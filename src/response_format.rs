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
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return Some(trimmed.to_owned());
    }
    let body = trimmed
        .strip_prefix("```json\n")
        .or_else(|| trimmed.strip_prefix("```\n"))?
        .strip_suffix("\n```")?
        .trim();
    serde_json::from_str::<serde_json::Value>(body).ok()?;
    Some(body.to_owned())
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
}
