use std::path::Path;

use serde_json::Value;

use crate::cerebras::{self, Client, Message, Outcome, Role};
use crate::error::{Error, Result};
use crate::instructions::Instructions;
use crate::state::Turn;
use crate::tools::{self, ToolSet};
use crate::triage::{self, Plan};
use crate::typesafe;

const MAX_TURNS: usize = 12;
const TRIAGE_EXCHANGES: usize = 2;
const WEB_SEARCH_BLOCKED: &str = "web_search is unavailable after workspace files have been read \
     in this question, so file contents cannot leave the machine; answer from what you have and \
     suggest the user ask about the web separately";

pub struct Agent {
    client: Client,
    model: String,
    effort: Option<String>,
    auto_effort: bool,
    triage: Option<typesafe::Client>,
    notice: Option<Error>,
    messages: Vec<Message>,
}

impl Agent {
    pub fn new(
        model: Option<&str>,
        effort: Option<&str>,
        instructions: &Instructions,
        history: &[Turn],
    ) -> Result<Self> {
        let selected = cerebras::require_model(model.unwrap_or(cerebras::default_model()))?;
        if let Some(level) = effort {
            selected.validate_reasoning(level)?;
        }
        let model = selected.id.to_owned();
        let auto_effort = effort == Some(triage::AUTO_REASONING);
        let effort = effort
            .filter(|_| !auto_effort)
            .map(str::to_owned)
            .or_else(|| {
                cerebras::find_model(&model)
                    .and_then(|model| model.default_reasoning.map(str::to_owned))
            });
        let mut messages = vec![Message::text(Role::System, system_prompt(instructions))];
        for turn in history {
            messages.push(Message::text(Role::User, turn.user.clone()));
            messages.push(Message::text(Role::Assistant, turn.assistant.clone()));
        }
        Ok(Self {
            client: Client::new(&model)?,
            model,
            effort,
            auto_effort,
            triage: typesafe::Client::from_env(),
            notice: None,
            messages,
        })
    }

    pub fn ask(
        &mut self,
        question: &str,
        root: &Path,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
        on_tool: &mut dyn FnMut(&str),
    ) -> Result<String> {
        let checkpoint = self.messages.len();
        let result = self.ask_inner(question, root, on_delta, on_tool);
        if result.is_err() {
            self.messages.truncate(checkpoint);
        }
        result
    }

    /// A one-time warning raised while answering, such as TypeSafe rejecting
    /// its API key. Callers print it once the answer is on screen.
    pub fn take_notice(&mut self) -> Option<Error> {
        self.notice.take()
    }

    fn ask_inner(
        &mut self,
        question: &str,
        root: &Path,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
        on_tool: &mut dyn FnMut(&str),
    ) -> Result<String> {
        let plan = self.plan(question, root);
        let effort = self.turn_effort(plan.effort);
        let start = self.messages.len();
        self.messages.push(Message::text(Role::User, question));
        let mut read_files = false;
        for _ in 0..MAX_TURNS {
            crate::cancel::check()?;
            let tools = tools::catalog(plan.tools.union(tools_in_history(&self.messages)));
            let outcome = self.client.stream(
                &self.messages,
                &tools,
                &self.model,
                effort.as_deref(),
                on_delta,
            )?;
            if outcome.tool_calls.is_empty() {
                if outcome.content.trim().is_empty() {
                    return Err(Error::new(
                        "the model completed without returning an answer",
                        "try again; if it keeps happening, lower the reasoning level",
                    ));
                }
                // Keep only the question and the answer. The tool calls and
                // their results would otherwise be sent again with every
                // later question, and a saved session drops them anyway.
                self.messages.truncate(start);
                self.messages.push(Message::text(Role::User, question));
                self.messages
                    .push(Message::text(Role::Assistant, outcome.content.clone()));
                return Ok(outcome.content);
            }
            self.apply_tool_turn(outcome, root, on_tool, &mut read_files)?;
        }
        Err(Error::new(
            format!("the model used tools more than {MAX_TURNS} times without answering"),
            "try a more specific question",
        ))
    }

    /// Asks Jev what this question needs. Any failure falls back to offering
    /// every tool with the configured effort, so triage can only speed wut up.
    fn plan(&mut self, question: &str, root: &Path) -> Plan {
        let Some(client) = &self.triage else {
            return Plan::default();
        };
        let exchanges = recent_exchanges(&self.messages, TRIAGE_EXCHANGES);
        match triage::plan(client, question, &exchanges, root) {
            Ok(plan) => plan,
            Err(failure) => {
                if failure.permanent {
                    self.triage = None;
                    self.notice = Some(failure.error.context("TypeSafe triage disabled"));
                }
                Plan::default()
            }
        }
    }

    fn turn_effort(&self, suggested: Option<triage::Effort>) -> Option<String> {
        if !self.auto_effort {
            return self.effort.clone();
        }
        suggested
            .and_then(|effort| {
                let model = cerebras::find_model(&self.model)?;
                effort.level(model.levels).map(str::to_owned)
            })
            .or_else(|| self.effort.clone())
    }

    fn apply_tool_turn(
        &mut self,
        outcome: Outcome,
        root: &Path,
        on_tool: &mut dyn FnMut(&str),
        read_files: &mut bool,
    ) -> Result<()> {
        let mut assistant = Message::text(Role::Assistant, outcome.content);
        assistant.tool_calls = outcome.tool_calls.clone();
        self.messages.push(assistant);
        for call in outcome.tool_calls {
            crate::cancel::check()?;
            let arguments = parse_arguments(&call.arguments);
            let description = tools::describe_call(&call.name, &arguments);
            // A search query leaves the machine, so once this question has
            // read file contents a query could carry them out, for example
            // when a file holds a prompt injection. Refuse the search instead.
            if call.name == "web_search" && *read_files {
                on_tool(&format!("{description} (blocked: files were read)"));
                self.messages
                    .push(Message::tool_result(call.id, WEB_SEARCH_BLOCKED.to_owned()));
                continue;
            }
            on_tool(&description);
            if tools::reads_contents(&call.name) {
                *read_files = true;
            }
            let content = match tools::execute(&call.name, &arguments, root) {
                Ok(content) => content,
                Err(error) => error.message().to_owned(),
            };
            self.messages.push(Message::tool_result(call.id, content));
        }
        Ok(())
    }
}

/// The configured instructions plus today's date. Without the date the model
/// assumes it is still the year its training data ends in.
fn system_prompt(instructions: &Instructions) -> String {
    let mut context = crate::clock::today();
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) && crate::diagram::images_supported() {
        context.push_str(DIAGRAMS);
    }
    match instructions.prompt() {
        Some(prompt) => format!("{prompt}\n\n{context}"),
        None => context,
    }
}

/// Added when the terminal can show the diagrams wut draws from ```mermaid
/// blocks, so piped answers and other terminals don't get diagram source.
const DIAGRAMS: &str = " This terminal draws ```mermaid code blocks as diagrams. \
     When a diagram explains something better than prose, such as a flow, a sequence of \
     messages, an architecture or a state machine, include one as a ```mermaid block, \
     keep it small, and still answer in words.";

fn parse_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(serde_json::Map::new());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

/// Tool groups the conversation has already called. They stay on offer so a
/// request never carries a call to a tool it does not define.
fn tools_in_history(messages: &[Message]) -> ToolSet {
    ToolSet::offering(
        messages
            .iter()
            .flat_map(|message| &message.tool_calls)
            .map(|call| call.name.as_str()),
    )
}

/// The last `limit` completed question/answer pairs, skipping tool traffic.
fn recent_exchanges(messages: &[Message], limit: usize) -> Vec<(String, String)> {
    let mut exchanges = Vec::new();
    let mut pending: Option<&str> = None;
    for message in messages {
        match message.role {
            Role::User => pending = Some(&message.content),
            Role::Assistant if message.tool_calls.is_empty() && !message.content.is_empty() => {
                if let Some(user) = pending.take() {
                    exchanges.push((user.to_owned(), message.content.clone()));
                }
            }
            Role::Assistant | Role::System | Role::Tool => {}
        }
    }
    let skip = exchanges.len().saturating_sub(limit);
    exchanges.drain(..skip);
    exchanges
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    use super::{Agent, recent_exchanges, system_prompt, tools_in_history};
    use crate::instructions::Instructions;

    #[test]
    fn the_system_prompt_always_carries_the_date() {
        let concise = system_prompt(&Instructions::Concise);
        assert!(concise.starts_with("Be friendly"), "{concise}");
        assert!(concise.contains("Today is "), "{concise}");
        let bare = system_prompt(&Instructions::AgentDefault);
        assert!(bare.starts_with("Today is "), "{bare}");
    }
    use crate::cerebras::{Client, Message, Role, ToolCall};
    use crate::tools::ToolSet;

    fn calling(names: &[&str]) -> Message {
        let mut message = Message::text(Role::Assistant, "");
        for (index, name) in names.iter().enumerate() {
            message.tool_calls.push(ToolCall {
                id: format!("call_{index}"),
                name: (*name).to_owned(),
                arguments: "{}".to_owned(),
            });
        }
        message
    }

    #[test]
    fn keeps_offering_tools_the_conversation_already_called() {
        let plain = vec![
            Message::text(Role::User, "how do I exit vim?"),
            Message::text(Role::Assistant, "press esc, then type :q"),
        ];
        assert_eq!(
            tools_in_history(&plain),
            ToolSet {
                workspace: false,
                web_search: false,
            }
        );

        let mut searched = plain.clone();
        searched.push(Message::text(Role::User, "where is the key read?"));
        searched.push(calling(&["grep"]));
        searched.push(Message::tool_result("call_0", "src/cerebras.rs:12"));
        assert_eq!(
            tools_in_history(&searched),
            ToolSet {
                workspace: true,
                web_search: false,
            }
        );

        searched.push(calling(&["web_search", "read"]));
        assert_eq!(tools_in_history(&searched), ToolSet::default());
    }

    #[test]
    fn collects_completed_exchanges_without_tool_traffic() {
        let mut searching = Message::text(Role::Assistant, "");
        searching.tool_calls.push(ToolCall {
            id: "call_1".to_owned(),
            name: "grep".to_owned(),
            arguments: "{}".to_owned(),
        });
        let messages = vec![
            Message::text(Role::System, "be brief"),
            Message::text(Role::User, "first"),
            Message::text(Role::Assistant, "one"),
            Message::text(Role::User, "second"),
            searching,
            Message::tool_result("call_1", "no matches"),
            Message::text(Role::Assistant, "two"),
            Message::text(Role::User, "third"),
            Message::text(Role::Assistant, "three"),
            Message::text(Role::User, "unanswered"),
        ];
        assert_eq!(
            recent_exchanges(&messages, 2),
            vec![
                ("second".to_owned(), "two".to_owned()),
                ("third".to_owned(), "three".to_owned()),
            ]
        );
        assert!(recent_exchanges(&messages[..1], 2).is_empty());
    }

    /// Serves one canned SSE body per request, in order, and returns the
    /// base URL plus a handle yielding the request bodies it saw.
    fn serve(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for body in responses {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut request = vec![0; length];
                reader.read_exact(&mut request).unwrap();
                requests.push(String::from_utf8(request).unwrap());
                let mut stream = reader.into_inner();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        (base, handle)
    }

    fn sse(events: &[&str]) -> String {
        let mut body: String = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect();
        body.push_str("data: [DONE]\n\n");
        body
    }

    #[test]
    fn tool_traffic_is_dropped_once_the_turn_is_answered() {
        let (base, server) = serve(vec![
            sse(&[
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"ls","arguments":"{}"}}]}}]}"#,
            ]),
            sse(&[r#"{"choices":[{"delta":{"content":"two files"}}]}"#]),
            sse(&[r#"{"choices":[{"delta":{"content":"yes"}}]}"#]),
        ]);
        let mut agent = Agent {
            client: Client::for_test(&base),
            model: "gpt-oss-120b".to_owned(),
            effort: None,
            auto_effort: false,
            triage: None,
            notice: None,
            messages: Vec::new(),
        };
        let root = std::env::temp_dir();
        let mut tools = Vec::new();
        let answer = agent
            .ask("what is here?", &root, &mut |_| Ok(()), &mut |call| {
                tools.push(call.to_owned())
            })
            .unwrap();
        assert_eq!(answer, "two files");
        assert_eq!(tools, ["ls"]);
        let roles: Vec<_> = agent.messages.iter().map(|message| message.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant]);

        agent
            .ask("sure?", &root, &mut |_| Ok(()), &mut |_| {})
            .unwrap();
        let requests = server.join().unwrap();
        assert!(
            requests[1].contains("\"tool\""),
            "the tool result is sent within its turn"
        );
        assert!(!requests[2].contains("\"tool\""), "{}", requests[2]);
        assert!(requests[2].contains("two files"));
    }

    fn test_agent(base: &str) -> Agent {
        Agent {
            client: Client::for_test(base),
            model: "gpt-oss-120b".to_owned(),
            effort: None,
            auto_effort: false,
            triage: None,
            notice: None,
            messages: Vec::new(),
        }
    }

    #[test]
    fn groq_can_read_a_workspace_file_and_stream_the_answer() {
        let root = tempdir();
        std::fs::write(root.join("notes.txt"), "the build command is cargo test").unwrap();
        let (base, server) = serve(vec![
            tool_call("read", r#"{"path":"notes.txt"}"#),
            sse(&[r#"{"choices":[{"delta":{"content":"Run cargo test."}}]}"#]),
        ]);
        let mut agent = test_agent(&base);
        agent.client = Client::for_provider_test(&base, crate::cerebras::Provider::Groq);
        agent.model = "groq:openai/gpt-oss-20b".to_owned();
        agent.effort = Some("low".to_owned());
        let mut streamed = String::new();
        let answer = agent
            .ask(
                "how do I test this?",
                &root,
                &mut |delta| {
                    streamed.push_str(delta);
                    Ok(())
                },
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(answer, "Run cargo test.");
        assert_eq!(streamed, answer);
        let requests = server.join().unwrap();
        for request in &requests {
            let value: serde_json::Value = serde_json::from_str(request).unwrap();
            assert_eq!(value["model"], "openai/gpt-oss-20b");
            assert_eq!(value["reasoning_effort"], "low");
            assert_eq!(value["include_reasoning"], false);
        }
        assert!(requests[1].contains("the build command is cargo test"));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn tool_call(name: &str, arguments: &str) -> String {
        let call = serde_json::json!({"choices":[{"delta":{"tool_calls":[
            {"index":0,"id":"c1","function":{"name":name,"arguments":arguments}}
        ]}}]});
        sse(&[&call.to_string()])
    }

    #[test]
    fn web_search_is_refused_after_files_are_read() {
        let root = tempdir();
        std::fs::write(root.join("notes.txt"), "secret plan").unwrap();
        let (base, server) = serve(vec![
            tool_call("read", r#"{"path":"notes.txt"}"#),
            tool_call("web_search", r#"{"query":"secret plan"}"#),
            sse(&[r#"{"choices":[{"delta":{"content":"done"}}]}"#]),
        ]);
        let mut agent = test_agent(&base);
        let mut tools = Vec::new();
        let answer = agent
            .ask("summarize notes", &root, &mut |_| Ok(()), &mut |call| {
                tools.push(call.to_owned())
            })
            .unwrap();
        assert_eq!(answer, "done");
        assert_eq!(
            tools,
            [
                "read notes.txt",
                "web search \"secret plan\" (blocked: files were read)"
            ]
        );
        let requests = server.join().unwrap();
        assert!(
            requests[2].contains("web_search is unavailable"),
            "{}",
            requests[2]
        );
    }

    #[test]
    fn a_blank_answer_is_an_error() {
        let (base, server) = serve(vec![sse(&[
            r#"{"choices":[{"delta":{"content":"\n\n  \n"}}]}"#,
        ])]);
        let mut agent = test_agent(&base);
        let error = agent
            .ask("hi", &std::env::temp_dir(), &mut |_| Ok(()), &mut |_| {})
            .unwrap_err();
        assert!(error.message().contains("without returning an answer"));
        assert!(agent.messages.is_empty(), "a failed turn is not kept");
        server.join().unwrap();
    }

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wut-agent-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }
}
