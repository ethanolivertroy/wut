use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::Error;

pub const ENV_KEY: &str = "TYPESAFE_API_KEY";
const ENV_BASE_URL: &str = "TYPESAFE_BASE_URL";
const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// Pinned because the triage thresholds were checked against this version,
/// while `jev-latest` moves with each release. Re-run evals/triage before
/// changing it.
pub const MODEL: &str = "jev-1.13.0";
const RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_ATTEMPTS: u8 = 2;

/// A minimal client for TypeSafe's System One endpoint.
///
/// System One models such as Jev answer typed questions (Noul, Choice, Score)
/// about a `state`; they do not generate text. See https://docs.typesafe.ai/api.
#[derive(Clone)]
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
    response: Value,
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

    #[cfg(test)]
    pub fn for_test(base_url: &str) -> Self {
        Self {
            api_key: "test-key".to_owned(),
            base_url: base_url.to_owned(),
        }
    }

    /// Asks `questions` about `state`, waiting at most `budget` for the
    /// answers, retries included.
    pub fn system_one(
        &self,
        state: Value,
        questions: Value,
        budget: Duration,
    ) -> std::result::Result<Answers, Failure> {
        let client = self.clone();
        within(budget, move |deadline| {
            client.send(&state, &questions, deadline)
        })
    }

    fn send(
        &self,
        state: &Value,
        questions: &Value,
        deadline: Instant,
    ) -> std::result::Result<Answers, Failure> {
        let body = request_body(state, questions).to_string();
        let url = format!("{}/v1/systemone", self.base_url);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(deadline.saturating_duration_since(Instant::now()))
            .build();
        let mut attempt = 1;
        loop {
            let result = agent
                .post(&url)
                .timeout(deadline.saturating_duration_since(Instant::now()))
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
                // 429 and 529 ask for a retry after a short backoff, which is
                // only worth waiting for when the budget still covers it.
                Err(ureq::Error::Status(status @ (429 | 529), response))
                    if attempt < MAX_ATTEMPTS =>
                {
                    let delay = retry_delay(&response, attempt);
                    if Instant::now() + delay >= deadline {
                        return Err(request_failure(ureq::Error::Status(status, response)));
                    }
                    thread::sleep(delay);
                    attempt += 1;
                }
                Err(error) => return Err(request_failure(error)),
            }
        }
    }
}

/// Runs `work` on its own thread and waits for it until `budget` has passed.
/// ureq cannot interrupt a slow DNS lookup, so only a separate thread makes
/// the budget a hard limit; a result that arrives later is dropped.
fn within<T: Send + 'static>(
    budget: Duration,
    work: impl FnOnce(Instant) -> std::result::Result<T, Failure> + Send + 'static,
) -> std::result::Result<T, Failure> {
    let deadline = Instant::now() + budget;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work(deadline));
    });
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_else(|_| Err(out_of_time(budget)))
}

pub fn request_body(state: &Value, questions: &Value) -> Value {
    json!({
        "state": state,
        "model": MODEL,
        "questions": questions,
    })
}

fn parse_response(text: &str) -> std::result::Result<Answers, Failure> {
    let response: Value = serde_json::from_str(text).map_err(|error| Failure {
        error: Error::new(
            format!("TypeSafe returned an invalid response: {error}"),
            "try again shortly",
        ),
        permanent: false,
    })?;
    if !response["answers"].is_object() {
        return Err(Failure {
            error: Error::new("TypeSafe returned no answers", "try again shortly"),
            permanent: false,
        });
    }
    Ok(Answers { response })
}

impl Answers {
    #[cfg(test)]
    pub fn from_response(response: Value) -> Self {
        Self { response }
    }

    /// The whole response body, including the versioned `model` that
    /// answered and the token `usage`.
    #[cfg(test)]
    pub fn response(&self) -> &Value {
        &self.response
    }

    /// The probability that the answer to a Noul question is yes.
    pub fn noul(&self, id: &str) -> Option<f64> {
        self.answer(id, "noul")?["noul"].as_f64()
    }

    pub fn choice(&self, id: &str) -> Option<Choice> {
        let answer = self.answer(id, "choice")?;
        Some(Choice {
            choice: answer["choice"].as_str()?.to_owned(),
            confidence: answer["confidence"].as_f64()?,
        })
    }

    fn answer(&self, id: &str, kind: &str) -> Option<&Value> {
        self.response["answers"]
            .get(id)
            .filter(|answer| answer["type"] == kind)
    }
}

/// The wait before retrying, honoring `retry-after-ms` or `retry-after`
/// (in seconds) when the response asks for longer than the backoff.
fn retry_delay(response: &ureq::Response, attempt: u8) -> Duration {
    let backoff = RETRY_DELAY * u32::from(attempt);
    let seconds = |name: &str, scale: f64| {
        response
            .header(name)
            .and_then(|value| value.trim().parse::<f64>().ok())
            .and_then(|value| Duration::try_from_secs_f64(value / scale).ok())
    };
    seconds("retry-after-ms", 1_000.0)
        .or_else(|| seconds("retry-after", 1.0))
        .map_or(backoff, |requested| requested.max(backoff))
}

fn out_of_time(budget: Duration) -> Failure {
    Failure {
        error: Error::new(
            format!("TypeSafe did not answer within {budget:?}"),
            "check your connection and try again",
        ),
        permanent: false,
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
pub mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::{Choice, Client, parse_response, request_body, within};

    /// A local stand-in for the System One endpoint. Each connection gets the
    /// next scripted response after its delay; requests are passed back raw.
    pub struct Server {
        pub url: String,
        pub requests: Receiver<String>,
        connections: Arc<AtomicUsize>,
    }

    impl Server {
        pub fn start(script: Vec<(Duration, String)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let connections = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&connections);
            let (sender, requests) = mpsc::channel();
            thread::spawn(move || {
                for (delay, response) in script {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    counter.fetch_add(1, Ordering::SeqCst);
                    let _ = sender.send(read_request(&mut stream));
                    thread::sleep(delay);
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            Self {
                url,
                requests,
                connections,
            }
        }

        pub fn connections(&self) -> usize {
            self.connections.load(Ordering::SeqCst)
        }
    }

    pub fn http(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut data = Vec::new();
        let mut buffer = [0; 4096];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 {
                break;
            }
            data.extend_from_slice(&buffer[..read]);
            let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                break;
            }
        }
        String::from_utf8_lossy(&data).into_owned()
    }

    const ANSWERS: &str = r#"{
        "model": "jev-1.13.0",
        "answers": {"needs_web": {"type": "noul", "noul": 0.1}},
        "usage": {"input_tokens": 300, "output_tokens": 20}
    }"#;

    fn ask(server: &Server, budget: Duration) -> Result<super::Answers, super::Failure> {
        Client::for_test(&server.url).system_one(
            json!({"question": "q"}),
            json!({"needs_web": {"type": "noul", "instructions": "Needs the web?"}}),
            budget,
        )
    }

    #[test]
    fn builds_a_system_one_request() {
        let body = request_body(
            &json!({"question": "how do I exit vim?"}),
            &json!({"urgent": {"type": "noul", "instructions": "Is it urgent?"}}),
        );
        assert_eq!(body["model"], "jev-1.13.0");
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
        assert_eq!(answers.response()["model"], "jev-1.13.0");
    }

    #[test]
    fn rejects_responses_without_answers() {
        let failure = parse_response(r#"{"model": "jev-1.13.0"}"#).unwrap_err();
        assert!(!failure.permanent);
        assert!(parse_response("not json").is_err());
    }

    #[test]
    fn posts_questions_to_the_system_one_endpoint() {
        let server = Server::start(vec![(Duration::ZERO, http("200 OK", "", ANSWERS))]);
        let answers = ask(&server, Duration::from_secs(5)).unwrap();
        assert_eq!(answers.noul("needs_web"), Some(0.1));

        let request = server.requests.recv().unwrap();
        assert!(request.starts_with("POST /v1/systemone HTTP/1.1"));
        let lowered = request.to_lowercase();
        assert!(lowered.contains("authorization: bearer test-key"));
        assert!(lowered.contains("content-type: application/json"));
        let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["model"], "jev-1.13.0");
        assert_eq!(body["questions"]["needs_web"]["type"], "noul");
    }

    #[test]
    fn retries_a_rate_limit_within_the_budget() {
        let server = Server::start(vec![
            (
                Duration::ZERO,
                http("429 Too Many Requests", "Retry-After-Ms: 20\r\n", "{}"),
            ),
            (Duration::ZERO, http("200 OK", "", ANSWERS)),
        ]);
        let answers = ask(&server, Duration::from_secs(5)).unwrap();
        assert_eq!(answers.noul("needs_web"), Some(0.1));
        assert_eq!(server.connections(), 2);
    }

    #[test]
    fn skips_a_retry_the_budget_cannot_cover() {
        let server = Server::start(vec![
            (
                Duration::ZERO,
                http("529 Overloaded", "Retry-After: 30\r\n", "{}"),
            ),
            (Duration::ZERO, http("200 OK", "", ANSWERS)),
        ]);
        let started = Instant::now();
        let failure = ask(&server, Duration::from_secs(2)).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!failure.permanent);
        assert!(failure.error.message().contains("HTTP 529"));
        assert_eq!(server.connections(), 1);
    }

    #[test]
    fn stops_waiting_once_the_budget_is_spent() {
        let started = Instant::now();
        let failure = within(Duration::from_millis(100), |_| {
            thread::sleep(Duration::from_secs(5));
            Ok(())
        })
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!failure.permanent);
        assert_eq!(
            failure.error.message(),
            "TypeSafe did not answer within 100ms"
        );
    }

    #[test]
    fn abandons_a_stalled_server_within_the_budget() {
        let server = Server::start(vec![(Duration::from_secs(5), http("200 OK", "", ANSWERS))]);
        let started = Instant::now();
        let failure = ask(&server, Duration::from_millis(200)).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!failure.permanent, "a slow answer must not disable triage");
    }

    #[test]
    fn rejected_keys_are_permanent() {
        let server = Server::start(vec![(
            Duration::ZERO,
            http("401 Unauthorized", "", r#"{"detail": "Invalid API key"}"#),
        )]);
        let failure = ask(&server, Duration::from_secs(5)).unwrap_err();
        assert!(failure.permanent);
        assert_eq!(
            failure.error.message(),
            "TypeSafe returned HTTP 401: Invalid API key"
        );
    }
}
