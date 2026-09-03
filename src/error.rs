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
            message: message.into(),
            help: help.into(),
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
        self.message = format!("{context}: {}", self.message);
        self
    }

    pub fn print(&self) {
        eprintln!("wut: {}", self.message);
        eprintln!("     {}", self.help);
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
