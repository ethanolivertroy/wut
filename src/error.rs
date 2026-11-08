use std::fmt::{self, Display};

const ISSUE_URL: &str = "https://github.com/ethanolivertroy/wut/issues";

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Eq, PartialEq)]
pub struct Error {
    message: String,
    help: String,
}

impl Error {
    pub fn new(message: impl Into<String>, help: impl Into<String>) -> Self {
        Self {
            message: escape_controls(message.into()),
            help: escape_controls(help.into()),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(message, "run 'wut --help' for usage")
    }

    pub fn terminal(action: &str, error: impl Display) -> Self {
        Self::new(
            format!("{action}: {error}"),
            "restart wut in an interactive terminal and try again",
        )
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            message,
            format!("try again; if it keeps happening, report it at {ISSUE_URL}"),
        )
    }

    pub fn context(mut self, context: impl Display) -> Self {
        self.message = escape_controls(format!("{context}: {}", self.message));
        self
    }

    pub fn print(&self) {
        eprintln!("wut: {}", self.message);
        eprintln!("     {}", self.help);
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    #[cfg(test)]
    pub fn help(&self) -> &str {
        &self.help
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Diagnostics may contain provider messages or attacker-controlled file names.
/// Keep printable Unicode intact, but show controls as literal escape text.
fn escape_controls(text: String) -> String {
    if !text.chars().any(char::is_control) {
        return text;
    }
    let mut safe = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            safe.extend(character.escape_default());
        } else {
            safe.push(character);
        }
    }
    safe
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn diagnostics_escape_controls_in_messages_help_and_context() {
        let error = Error::new("provider: \x1b]52;c;ZmFrZQ==\x07", "héllo\n\u{9b}2J")
            .context("path\rspoofed");
        assert!(!error.message().chars().any(char::is_control));
        assert!(!error.help().chars().any(char::is_control));
        assert!(error.message().contains(r"\u{1b}"));
        assert!(error.message().contains(r"path\rspoofed"));
        assert!(error.help().starts_with("héllo\\n"));
        assert_eq!(Error::new("普通のエラー", "help").message(), "普通のエラー");
    }
}
