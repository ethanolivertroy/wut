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

    pub fn agent(command: &str, message: impl Into<String>) -> Self {
        Self::new(
            message,
            format!("run '{command}' directly to fix it, then try again"),
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

    pub fn detail(mut self, detail: impl Display) -> Self {
        self.message = format!("{}: {detail}", self.message);
        self
    }

    pub fn print(&self) {
        eprintln!("wut: {}", self.message);
        eprintln!("     {}", self.help);
    }

    pub fn message(&self) -> &str {
        &self.message
    }

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

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn errors_reference_only_the_wut_product_identity() {
        assert_eq!(
            Error::usage("bad input").help(),
            "run 'wut --help' for usage"
        );
        assert_eq!(
            Error::terminal("read input", "failed").help(),
            "restart wut in an interactive terminal and try again"
        );
        let internal = Error::internal("broken");
        assert!(
            internal
                .help()
                .contains("https://github.com/ethanolivertroy/wut/issues")
        );
        assert!(!internal.help().contains("benja/ask"));
    }
}
