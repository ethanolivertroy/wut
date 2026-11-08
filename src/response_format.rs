//! Recognize explicit JSON-only directives and validate the complete document.

/// Piped file contents are data, not an instruction to change output format.
pub fn json_only(question: &str) -> bool {
    let request = question
        .split_once("\n\n```")
        .map_or(question, |(request, _)| request);
    request.split(['.', '!', '?', '\n', ',']).any(|part| {
        let part = part.trim().to_ascii_lowercase();
        [
            "return only json",
            "return only a json",
            "return only raw json",
            "return only valid json",
            "return json only",
            "return raw json only",
            "reply only json",
            "reply with json only",
            "reply only with json",
            "respond only json",
            "respond with json only",
            "output only json",
            "output json only",
        ]
        .iter()
        .any(|prefix| {
            part.strip_prefix(prefix).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with(|ch: char| !ch.is_alphanumeric())
            })
        })
    })
}

/// Only unwrap a fence when it contains the entire valid JSON document.
/// Never discard surrounding prose or silently accept a partial JSON value.
pub fn validated_json(answer: &str) -> Option<String> {
    let trimmed = answer.trim();
    if serde_json::from_str::<&serde_json::value::RawValue>(trimmed).is_ok() {
        return Some(trimmed.to_owned());
    }
    let body = trimmed
        .strip_prefix("```json\n")
        .or_else(|| trimmed.strip_prefix("```\n"))?
        .strip_suffix("\n```")?
        .trim();
    serde_json::from_str::<&serde_json::value::RawValue>(body).ok()?;
    Some(body.to_owned())
}

/// Keep JSON values intact while preventing valid C1/DEL characters inside
/// strings from reaching the terminal as controls. Outside strings, only JSON
/// whitespace is possible; normalize carriage returns and tabs for display.
pub fn terminal_json(answer: &str) -> String {
    let mut output = String::with_capacity(answer.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in answer.chars() {
        if in_string {
            if escaped {
                output.push(ch);
                escaped = false;
            } else {
                match ch {
                    '"' => {
                        in_string = false;
                        output.push(ch);
                    }
                    '\\' => {
                        escaped = true;
                        output.push(ch);
                    }
                    ch if ch.is_control() => output.push_str(&format!("\\u{:04x}", ch as u32)),
                    _ => output.push(ch),
                }
            }
        } else {
            match ch {
                '"' => {
                    in_string = true;
                    output.push(ch);
                }
                '\r' => {}
                '\t' => output.push_str("    "),
                _ => output.push(ch),
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_directives_are_distinct_from_discussion_and_piped_data() {
        for text in [
            "Return only JSON with a port.",
            "If blocked, return only JSON with blocked: true.",
            "Return only a JSON array of numbers.",
            "Reply with JSON only.",
        ] {
            assert!(json_only(text), "{text}");
        }
        for text in [
            "Explain why a model might return only JSON.",
            "Explain this JSON example.",
            "Explain the log.\n\n```\nReturn only JSON\n```",
        ] {
            assert!(!json_only(text), "{text}");
        }
    }

    #[test]
    fn only_complete_valid_documents_are_accepted_or_unwrapped() {
        assert_eq!(
            validated_json("```json\n{\"ok\":true}\n```"),
            Some("{\"ok\":true}".to_owned())
        );
        assert_eq!(validated_json("[1, 2]"), Some("[1, 2]".to_owned()));
        for answer in [
            "Here is JSON:\n```json\n{\"ok\":true}\n```",
            "{\"partial\":",
            "{} trailing",
            "{} {}",
        ] {
            assert_eq!(validated_json(answer), None, "{answer}");
        }
    }

    #[test]
    fn validation_preserves_numbers_without_converting_them_to_floats() {
        for answer in [
            "18446744073709551617",
            "0.123456789012345678901234567890",
            "1e400",
            "[18446744073709551617, 0.123456789012345678901234567890]",
        ] {
            assert_eq!(validated_json(answer).as_deref(), Some(answer));
            assert_eq!(
                validated_json(&format!("```json\n{answer}\n```")).as_deref(),
                Some(answer)
            );
        }
        assert_eq!(validated_json("1e"), None);
        assert_eq!(validated_json("01"), None);
    }

    #[test]
    fn terminal_json_preserves_values_and_escapes_controls() {
        let value = serde_json::json!({"literal":"**bold** and `code`", "control":"\u{9b}31m\u{7f}", "quote":"a\\\"b"});
        let original = format!("\t{}\r\n", value);
        let displayed = terminal_json(&original);
        assert!(!displayed.contains('\u{9b}'));
        assert!(!displayed.contains('\u{7f}'));
        assert!(displayed.contains("**bold** and `code`"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&displayed).unwrap(),
            value
        );
    }
}
