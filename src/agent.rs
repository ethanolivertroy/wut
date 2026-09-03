use std::path::Path;

use serde_json::Value;

use crate::cerebras::{self, Client, Message, Outcome, Role};
use crate::error::{Error, Result};
use crate::instructions::Instructions;
use crate::state::Turn;
use crate::tools;

const MAX_TURNS: usize = 12;

pub struct Agent {
    client: Client,
    model: String,
    effort: Option<String>,
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
        let effort = effort.map(str::to_owned).or_else(|| {
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

    fn ask_inner(
        &mut self,
        question: &str,
        root: &Path,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<String> {
        self.messages.push(Message::text(Role::User, question));
        let tools = tools::catalog();
        for _ in 0..MAX_TURNS {
            let outcome = self.client.stream(
                &self.messages,
                &tools,
                &self.model,
                self.effort.as_deref(),
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
