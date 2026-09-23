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
        let model = cerebras::resolve_model(model).to_owned();
        let auto_effort = effort == Some(triage::AUTO_REASONING);
        let effort = effort
            .filter(|_| !auto_effort)
            .map(str::to_owned)
            .or_else(|| {
                cerebras::find_model(&model)
                    .and_then(|model| model.default_reasoning.map(str::to_owned))
            });
        let mut messages = Vec::new();
        if let Some(prompt) = instructions.prompt() {
            messages.push(Message::text(Role::System, prompt));
        }
        for turn in history {
            messages.push(Message::text(Role::User, turn.user.clone()));
            messages.push(Message::text(Role::Assistant, turn.assistant.clone()));
        }
        Ok(Self {
            client: Client::new()?,
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
    ) -> Result<String> {
        let checkpoint = self.messages.len();
        let result = self.ask_inner(question, root, on_delta);
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
    ) -> Result<String> {
        let plan = self.plan(question, root);
        let effort = self.turn_effort(plan.effort);
        self.messages.push(Message::text(Role::User, question));
        for _ in 0..MAX_TURNS {
            let tools = tools::catalog(plan.tools.union(tools_in_history(&self.messages)));
            let outcome = self.client.stream(
                &self.messages,
                &tools,
                &self.model,
                effort.as_deref(),
                on_delta,
            )?;
            if outcome.tool_calls.is_empty() {
                if outcome.content.is_empty() {
                    return Err(Error::new(
                        "the model completed without returning an answer",
                        "try again; if it keeps happening, lower the reasoning level",
                    ));
                }
                self.messages
                    .push(Message::text(Role::Assistant, outcome.content.clone()));
                return Ok(outcome.content);
            }
            self.apply_tool_turn(outcome, root)?;
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

    fn apply_tool_turn(&mut self, outcome: Outcome, root: &Path) -> Result<()> {
        let mut assistant = Message::text(Role::Assistant, outcome.content);
        assistant.tool_calls = outcome.tool_calls.clone();
        self.messages.push(assistant);
        for call in outcome.tool_calls {
            let arguments = parse_arguments(&call.arguments);
            let content = match tools::execute(&call.name, &arguments, root) {
                Ok(content) => content,
                Err(error) => error.message().to_owned(),
            };
            self.messages.push(Message::tool_result(call.id, content));
        }
        Ok(())
    }
}

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
    use super::{recent_exchanges, tools_in_history};
    use crate::cerebras::{Message, Role, ToolCall};
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
}
