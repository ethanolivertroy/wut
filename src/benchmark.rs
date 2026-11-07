//! Direct inference measurements: no triage, workspace reads, or saved sessions.
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::cerebras::{self, Client, Message, Model, Role};
use crate::error::{Error, Result};
use crate::output::write_stdout;

const SAMPLES: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PROMPT: &str = "In one short sentence, explain what a terminal is. Do not use tools.";

pub fn run(selected: Option<&str>, reasoning: Option<&str>) -> Result<()> {
    let models: Vec<&Model> = match selected {
        Some(id) => vec![cerebras::require_model(id)?],
        None => cerebras::MODELS.iter().collect(),
    };
    if reasoning == Some(crate::triage::AUTO_REASONING) {
        return Err(Error::usage(
            "--benchmark measures direct inference; choose an explicit reasoning level instead of auto",
        ));
    }
    let mut succeeded = 0;
    let mut failed = 0;
    for model in models {
        let effort = reasoning.or(model.default_reasoning);
        if let Some(effort) = effort {
            model.validate_reasoning(effort)?;
        }
        let client = match Client::new(model.id) {
            Ok(client) => client.with_timeout(REQUEST_TIMEOUT),
            Err(error) => {
                emit(
                    &json!({"model": model.id, "provider": model.provider.name(), "status": "unavailable", "error": error.message()}),
                )?;
                failed += 1;
                continue;
            }
        };
        for sample in 1..=SAMPLES {
            match measure(&client, model, effort) {
                Ok(mut row) => {
                    row["sample"] = json!(sample);
                    emit(&row)?;
                    succeeded += 1;
                }
                Err(error) => {
                    emit(
                        &json!({"model": model.id, "provider": model.provider.name(), "sample": sample, "status": "error", "error": error.message()}),
                    )?;
                    failed += 1;
                    break;
                }
            }
        }
    }
    if succeeded == 0 || failed > 0 {
        return Err(Error::new(
            format!(
                "benchmark recorded {succeeded} successful samples and {failed} unavailable or failed models"
            ),
            "set the required API key variables, or select an available model with --model; results are JSON lines on stdout",
        ));
    }
    Ok(())
}

fn measure(client: &Client, model: &Model, effort: Option<&str>) -> Result<Value> {
    let start = Instant::now();
    let mut first_text = None;
    let outcome = client.stream(
        &[Message::text(Role::User, PROMPT)],
        &[],
        model.id,
        effort,
        &mut |delta| {
            if !delta.is_empty() && first_text.is_none() {
                first_text = Some(start.elapsed());
            }
            Ok(())
        },
    )?;
    if outcome.content.trim().is_empty() || !outcome.tool_calls.is_empty() {
        return Err(Error::new(
            "benchmark model did not return a plain-text answer",
            "try a different model or reasoning level",
        ));
    }
    Ok(json!({
        "model": model.id, "provider": model.provider.name(), "status": "ok",
        "reasoning": effort, "first_text_ms": first_text.map(|time| time.as_secs_f64() * 1000.0),
        "total_ms": start.elapsed().as_secs_f64() * 1000.0,
        "output_chars": outcome.content.chars().count(),
    }))
}

fn emit(row: &Value) -> Result<()> {
    write_stdout(&[row.to_string().as_bytes(), b"\n"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurements_report_first_text_separately_from_completion() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(socket);
            let mut length = None;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut request = vec![0; length.unwrap()];
            reader.read_exact(&mut request).unwrap();
            let request: Value = serde_json::from_slice(&request).unwrap();
            assert!(request.get("tools").is_none());
            assert_eq!(request["messages"][0]["content"], PROMPT);
            let first = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
            let done = "data: [DONE]\n\n";
            let socket = reader.get_mut();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{first}", first.len() + done.len()).unwrap();
            socket.flush().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            socket.write_all(done.as_bytes()).unwrap();
        });
        let client = Client::for_test(&base).with_timeout(Duration::from_secs(2));
        let model = cerebras::require_model("gpt-oss-120b").unwrap();
        let row = measure(&client, model, Some("low")).unwrap();
        assert_eq!(row["output_chars"], 2);
        assert_eq!(row["status"], "ok");
        assert_eq!(row["model"], model.id);
        assert!(row["first_text_ms"].as_f64().unwrap() <= row["total_ms"].as_f64().unwrap());
        server.join().unwrap();
    }
}
