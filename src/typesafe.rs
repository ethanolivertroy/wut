use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::error::Error;

pub const ENV_KEY: &str = "TYPESAFE_API_KEY";
const ENV_BASE_URL: &str = "TYPESAFE_BASE_URL";
const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
pub const MODEL: &str = "jev-latest";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_ATTEMPTS: u8 = 2;

/// A minimal client for TypeSafe's System One endpoint.
///
/// System One models such as Jev answer typed questions (Noul, Choice, Score)
/// about a `state`; they do not generate text. See https://docs.typesafe.ai/api.
pub struct Client {
    api_key: String,
    base_url: String,
}

/// A failed System One request. `permanent` marks failures that will repeat on
/// every request (bad credentials, malformed questions) so callers can stop
/// trying for the rest of the process.
#[derive(Debug)]
pub struct Failure {
    pub error: Error,
    pub permanent: bool,
}

#[derive(Debug)]
pub struct Answers {
    answers: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub choice: String,
    pub confidence: f64,
}

impl Client {
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var(ENV_KEY)
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())?;
        let base_url = std::env::var(ENV_BASE_URL)
            .ok()
            .map(|url| url.trim().trim_end_matches('/').to_owned())
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        Some(Self { api_key, base_url })
    }

    pub fn system_one(
        &self,
        state: &Value,
        questions: &Value,
    ) -> std::result::Result<Answers, Failure> {
        let body = request_body(state, questions).to_string();
        let url = format!("{}/v1/systemone", self.base_url);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(IO_TIMEOUT)
            .timeout_write(IO_TIMEOUT)
            .build();
        let mut attempt = 1;
        loop {
            let result = agent
                .post(&url)
                .set("Authorization", &format!("Bearer {}", self.api_key))
                .set("Content-Type", "application/json")
                .send_string(&body);
            match result {
                Ok(response) => {
                    let text = response.into_string().map_err(|error| Failure {
                        error: Error::new(
                            format!("could not read the TypeSafe response: {error}"),
                            "check your connection and try again",
                        ),
                        permanent: false,
                    })?;
                    return parse_response(&text);
                }
                // 429 and 529 ask for a retry after a short backoff.
                Err(ureq::Error::Status(429 | 529, _)) if attempt < MAX_ATTEMPTS => {
                    thread::sleep(RETRY_DELAY * u32::from(attempt));
                    attempt += 1;
                }
                Err(error) => return Err(request_failure(error)),
            }
        }
    }
}

pub fn request_body(state: &Value, questions: &Value) -> Value {
    json!({
        "state": state,
        "model": MODEL,
        "questions": questions,
    })
}

fn parse_response(text: &str) -> std::result::Result<Answers, Failure> {
    let value: Value = serde_json::from_str(text).map_err(|error| Failure {
        error: Error::new(
            format!("TypeSafe returned an invalid response: {error}"),
            "try again shortly",
        ),
        permanent: false,
    })?;
    let answers = value
        .get("answers")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| Failure {
            error: Error::new("TypeSafe returned no answers", "try again shortly"),
            permanent: false,
        })?;
    Ok(Answers { answers })
}

impl Answers {
    #[cfg(test)]
    pub fn from_map(answers: Map<String, Value>) -> Self {
        Self { answers }
    }

    /// The probability that the answer to a Noul question is yes.
    pub fn noul(&self, id: &str) -> Option<f64> {
        let answer = self.answers.get(id)?;
        if answer["type"] != "noul" {
            return None;
        }
        answer["noul"].as_f64()
    }

    pub fn choice(&self, id: &str) -> Option<Choice> {
        let answer = self.answers.get(id)?;
        if answer["type"] != "choice" {
            return None;
        }
        Some(Choice {
            choice: answer["choice"].as_str()?.to_owned(),
            confidence: answer["confidence"].as_f64()?,
        })
    }
}

fn request_failure(error: ureq::Error) -> Failure {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| {
                    value["detail"]
                        .as_str()
                        .or_else(|| value["error"]["message"].as_str())
                        .or_else(|| value["message"].as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| body.trim().to_owned());
            let (help, permanent) = match status {
                401 | 403 => (
                    format!("check {ENV_KEY} or unset it to skip TypeSafe"),
                    true,
                ),
                422 => (
                    "the request was rejected; report it at \
                     https://github.com/ethanolivertroy/wut/issues"
                        .to_owned(),
                    true,
                ),
                429 => (
                    "you hit a TypeSafe rate limit; wait a moment".to_owned(),
                    false,
                ),
                _ => ("TypeSafe may be temporarily unavailable".to_owned(), false),
            };
            let message = if detail.is_empty() {
                format!("TypeSafe returned HTTP {status}")
            } else {
                format!("TypeSafe returned HTTP {status}: {detail}")
            };
            Failure {
                error: Error::new(message, help),
                permanent,
            }
        }
        ureq::Error::Transport(error) => Failure {
            error: Error::new(
                format!("could not reach TypeSafe: {error}"),
                "check your connection and try again",
            ),
            permanent: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Choice, parse_response, request_body};

    #[test]
    fn builds_a_system_one_request() {
        let body = request_body(
            &json!({"question": "how do I exit vim?"}),
            &json!({"urgent": {"type": "noul", "instructions": "Is it urgent?"}}),
        );
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"]["question"], "how do I exit vim?");
        assert_eq!(body["questions"]["urgent"]["type"], "noul");
    }

    #[test]
    fn reads_typed_answers() {
        let answers = parse_response(
            r#"{
                "model": "jev-1.13.0",
                "answers": {
                    "is_urgent": {"type": "noul", "noul": 0.95},
                    "department": {
                        "type": "choice",
                        "choice": "billing",
                        "probabilities": {"billing": 0.88, "technical": 0.12},
                        "confidence": 0.81
                    }
                },
                "usage": {"input_tokens": 296, "output_tokens": 20}
            }"#,
        )
        .unwrap();
        assert_eq!(answers.noul("is_urgent"), Some(0.95));
        assert_eq!(
            answers.choice("department"),
            Some(Choice {
                choice: "billing".to_owned(),
                confidence: 0.81
            })
        );
        assert_eq!(answers.noul("department"), None);
        assert_eq!(answers.choice("is_urgent"), None);
        assert_eq!(answers.noul("missing"), None);
    }

    #[test]
    fn rejects_responses_without_answers() {
        let failure = parse_response(r#"{"model": "jev-1.13.0"}"#).unwrap_err();
        assert!(!failure.permanent);
        assert!(parse_response("not json").is_err());
    }
}
