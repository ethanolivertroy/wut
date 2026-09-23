//! Pre-flight triage of a question with TypeSafe's Jev.
//!
//! Before wut asks the Cerebras model, one System One request judges what the
//! question needs: workspace tools, web search, and how much reasoning. Code
//! owns every decision here; Jev only supplies the probabilities.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::tools;
use crate::typesafe::{Answers, Client, Failure};

/// Reasoning setting that lets triage pick the effort level per question.
pub const AUTO_REASONING: &str = "auto";

/// The longest triage may hold up an answer, retries included. Jev usually
/// answers in a few hundred milliseconds; past this wut answers without a plan.
pub const BUDGET: Duration = Duration::from_secs(2);

const RECENT_EXCHANGES: usize = 2;
const EXCHANGE_CHARS: usize = 300;
const QUESTION_CHARS: usize = 2_000;
const MAX_ENTRIES: usize = 40;

// Checked against `typesafe::MODEL` with the eval in evals/triage; re-run it
// before changing these, the questions, or the model. Tools are only dropped
// when Jev is confident they are unnecessary, because keeping an unused tool
// costs a few tokens while removing a needed one costs the answer.
pub const TOOL_UNNEEDED_MAX: f64 = 0.2;
pub const MIN_EFFORT_CONFIDENCE: f64 = 0.5;

pub const NEEDS_WORKSPACE: &str = "needs_workspace_files";
pub const NEEDS_WEB: &str = "needs_current_web_information";
pub const THINKING: &str = "thinking_required";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effort {
    Recall,
    Explain,
    Investigate,
}

impl Effort {
    fn parse(id: &str) -> Option<Self> {
        match id {
            "recall" => Some(Self::Recall),
            "explain" => Some(Self::Explain),
            "investigate" => Some(Self::Investigate),
            _ => None,
        }
    }

    /// Maps the effort onto a model's reasoning levels, ordered lowest first.
    pub fn level<'a>(self, levels: &[&'a str]) -> Option<&'a str> {
        match self {
            Self::Recall => levels.first(),
            Self::Explain => levels.get(levels.len() / 2),
            Self::Investigate => levels.last(),
        }
        .copied()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Plan {
    pub tools: tools::ToolSet,
    pub effort: Option<Effort>,
}

impl Plan {
    pub fn from_answers(answers: &Answers) -> Self {
        let mut plan = Self::default();
        if let Some(probability) = answers.noul(NEEDS_WORKSPACE) {
            plan.tools.workspace = probability > TOOL_UNNEEDED_MAX;
        }
        if let Some(probability) = answers.noul(NEEDS_WEB) {
            plan.tools.web_search = probability > TOOL_UNNEEDED_MAX;
        }
        if let Some(answer) = answers.choice(THINKING)
            && answer.confidence >= MIN_EFFORT_CONFIDENCE
        {
            plan.effort = Effort::parse(&answer.choice);
        }
        plan
    }
}

/// Asks Jev about `question` and returns the plan. Errors are returned so the
/// caller can decide whether to keep trying; the fallback is always
/// `Plan::default()`.
pub fn plan(
    client: &Client,
    question: &str,
    exchanges: &[(String, String)],
    root: &Path,
) -> std::result::Result<Plan, Failure> {
    let state = state(question, exchanges, root);
    let answers = client.system_one(state, questions(), BUDGET)?;
    Ok(Plan::from_answers(&answers))
}

pub fn state(question: &str, exchanges: &[(String, String)], root: &Path) -> Value {
    let mut state = json!({
        "question": clip(question, QUESTION_CHARS),
        "workspace": {
            "folder": root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "entries": workspace_entries(root),
        },
    });
    let recent: Vec<Value> = exchanges
        .iter()
        .rev()
        .take(RECENT_EXCHANGES)
        .rev()
        .map(|(user, assistant)| {
            json!({
                "user": clip(user, EXCHANGE_CHARS),
                "assistant": clip(assistant, EXCHANGE_CHARS),
            })
        })
        .collect();
    if !recent.is_empty() {
        state["conversation"] = Value::Array(recent);
    }
    state
}

pub fn questions() -> Value {
    json!({
        NEEDS_WORKSPACE: {
            "type": "noul",
            "instructions": {
                "question": "Does answering `question` require opening, searching, or listing files in `workspace`?",
                "context": "`workspace` is the directory the user runs the assistant from. `conversation` holds the most recent earlier exchanges; a follow-up question may refer back to them.",
            },
            "criteria": {
                "true": {
                    "what": "The answer depends on the contents of this particular project: its code, configuration, dependencies, structure, output, or an error it produces. Includes questions that point at the project with words like this, here, my, or our.",
                    "examples": [
                        "why is this broken?",
                        "what does the agent module do?",
                        "where is the API key read?",
                        "how do I run the tests here?",
                    ],
                },
                "false": {
                    "what": "The answer is general knowledge, a command or syntax to recall, or a concept that can be explained without opening any file in `workspace`.",
                    "examples": [
                        "how do I exit vim?",
                        "what does chmod 755 mean?",
                        "explain rust lifetimes",
                        "tar command to extract a .tar.gz",
                    ],
                },
            },
        },
        NEEDS_WEB: {
            "type": "noul",
            "instructions": {
                "question": "Does answering `question` require information from the web that may have changed recently, such as news, the latest release of some software, current prices or status, or the contents of a specific web page?",
            },
            "criteria": {
                "true": {
                    "what": "The correct answer depends on recent events, the current version or status of something, or the live contents of a website.",
                    "examples": [
                        "what's new in rust 1.90?",
                        "is github down right now?",
                        "latest version of node",
                        "summarize https://example.com/post",
                    ],
                },
                "false": {
                    "what": "The answer is stable knowledge, or concerns only the user's own files and conversation.",
                    "examples": [
                        "how do I exit vim?",
                        "what does this function do?",
                        "explain the tcp handshake",
                    ],
                },
            },
        },
        THINKING: {
            "type": "choice",
            "instructions": {
                "question": "How much deliberate thinking does a correct answer to `question` require?",
                "focus": "Judge how hard the answer is to work out once any files or web pages it needs are open, not how long the answer will be or how much reading and searching it takes.",
            },
            "criteria": {
                "recall": {
                    "what": "A quick fact, a command, syntax, or a one-step lookup, including finding, reading, or listing one thing in the project or on the web.",
                    "not_for": "Anything that must be worked out or diagnosed.",
                    "examples": [
                        "how do I exit vim?",
                        "git command to undo the last commit",
                        "what port does postgres use?",
                    ],
                },
                "explain": {
                    "what": "A concept, comparison, or how-something-works explanation with a well-known answer, including describing or summarizing what code, a project, or a page does.",
                    "not_for": "One-line recall, or diagnosing a specific problem.",
                    "examples": [
                        "explain rust lifetimes",
                        "difference between tcp and udp",
                        "how does git rebase work?",
                    ],
                },
                "investigate": {
                    "what": "Debugging, diagnosing an error, reviewing code or dependencies for problems, planning a change, weighing tradeoffs, or a multi-step problem where the answer must be worked out from evidence.",
                    "not_for": "Answers that only need to be looked up, listed, summarized, or stated, even when finding them means opening files or searching the web.",
                    "examples": [
                        "why is this broken?",
                        "why does my build fail with linker errors?",
                        "should this project use tokio or async-std?",
                    ],
                },
            },
        },
    })
}

fn workspace_entries(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| !tools::is_denied(&entry.path()))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    names.truncate(MAX_ENTRIES);
    names
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut clipped = text[..tools::char_floor(text, limit)].to_owned();
    clipped.push_str(" […]");
    clipped
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::{BUDGET, Effort, Plan, plan, state};
    use crate::typesafe::tests::{Server, http};
    use crate::typesafe::{Answers, Client};

    fn answers(value: serde_json::Value) -> Answers {
        Answers::from_response(json!({ "answers": value }))
    }

    #[test]
    fn keeps_every_tool_and_default_effort_without_answers() {
        let plan = Plan::from_answers(&answers(json!({})));
        assert_eq!(plan, Plan::default());
        assert!(plan.tools.workspace);
        assert!(plan.tools.web_search);
        assert_eq!(plan.effort, None);
    }

    #[test]
    fn drops_tools_only_when_confidently_unneeded() {
        let plan = Plan::from_answers(&answers(json!({
            "needs_workspace_files": {"type": "noul", "noul": 0.05},
            "needs_current_web_information": {"type": "noul", "noul": 0.35},
        })));
        assert!(!plan.tools.workspace);
        assert!(plan.tools.web_search);
    }

    #[test]
    fn picks_effort_only_with_enough_confidence() {
        let confident = Plan::from_answers(&answers(json!({
            "thinking_required": {"type": "choice", "choice": "investigate", "confidence": 0.9},
        })));
        assert_eq!(confident.effort, Some(Effort::Investigate));

        let unsure = Plan::from_answers(&answers(json!({
            "thinking_required": {"type": "choice", "choice": "recall", "confidence": 0.3},
        })));
        assert_eq!(unsure.effort, None);

        let unknown = Plan::from_answers(&answers(json!({
            "thinking_required": {"type": "choice", "choice": "other", "confidence": 0.9},
        })));
        assert_eq!(unknown.effort, None);
    }

    #[test]
    fn maps_effort_onto_model_levels() {
        let three = ["low", "medium", "high"];
        assert_eq!(Effort::Recall.level(&three), Some("low"));
        assert_eq!(Effort::Explain.level(&three), Some("medium"));
        assert_eq!(Effort::Investigate.level(&three), Some("high"));

        let four = ["none", "low", "medium", "high"];
        assert_eq!(Effort::Recall.level(&four), Some("none"));
        assert_eq!(Effort::Explain.level(&four), Some("medium"));
        assert_eq!(Effort::Investigate.level(&four), Some("high"));

        assert_eq!(Effort::Explain.level(&[]), None);
    }

    #[test]
    fn state_holds_question_workspace_and_recent_exchanges() {
        let exchanges = vec![
            ("first".to_owned(), "one".to_owned()),
            ("second".to_owned(), "two".to_owned()),
            ("third".to_owned(), "x".repeat(400)),
        ];
        let state = state("why is this broken?", &exchanges, Path::new("/tmp"));
        assert_eq!(state["question"], "why is this broken?");
        assert_eq!(state["workspace"]["folder"], "tmp");
        assert!(state["workspace"]["entries"].is_array());
        let conversation = state["conversation"].as_array().unwrap();
        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation[0]["user"], "second");
        assert_eq!(conversation[1]["user"], "third");
        let clipped = conversation[1]["assistant"].as_str().unwrap();
        assert!(clipped.ends_with(" […]"));
        assert!(clipped.len() < 400);
    }

    #[test]
    fn state_omits_conversation_when_empty() {
        let state = state("how do I exit vim?", &[], Path::new("/tmp"));
        assert!(state.get("conversation").is_none());
    }

    #[test]
    fn a_stalled_typesafe_costs_at_most_the_budget() {
        let server = Server::start(vec![(
            Duration::from_secs(30),
            http("200 OK", "", r#"{"answers": {}}"#),
        )]);
        let started = Instant::now();
        let failure =
            plan(&Client::for_test(&server.url), "q", &[], Path::new("/tmp")).unwrap_err();
        let waited = started.elapsed();
        let slack = Duration::from_millis(250);
        assert!(waited > BUDGET - slack && waited < BUDGET + slack);
        assert!(!failure.permanent, "a slow answer must not disable triage");
    }
}
