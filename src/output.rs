use std::io::{self, Write};

use crate::config::ResponseColor;
use crate::error::{Error, Result};
use crate::render::TerminalRenderer;

/// Output for a pipe or a file. Nothing is streamed: a turn that calls
/// tools streams the model's narration ("Let me search…") before the tool
/// calls, so only the final answer is written, matching what the session
/// saves.
struct RawStream;

pub struct TurnOutput {
    kind: TurnOutputKind,
    streamed: bool,
}

enum TurnOutputKind {
    Raw(RawStream),
    Rich(TerminalRenderer),
}

impl TurnOutput {
    pub fn new(rich: bool, response_color: ResponseColor) -> Self {
        Self {
            kind: if rich {
                TurnOutputKind::Rich(TerminalRenderer::new(response_color))
            } else {
                TurnOutputKind::Raw(RawStream)
            },
            streamed: false,
        }
    }

    pub fn push(&mut self, delta: &str) -> Result<()> {
        if !delta.is_empty() {
            self.streamed = true;
        }
        match &mut self.kind {
            TurnOutputKind::Raw(output) => output.push(delta),
            TurnOutputKind::Rich(output) => output.push(delta),
        }
    }

    /// The text streamed since the last tool call was narration, not the
    /// answer. A terminal redraws it in gray; a pipe never printed it.
    pub fn dim_round(&mut self) -> Result<()> {
        match &mut self.kind {
            TurnOutputKind::Raw(_) => Ok(()),
            TurnOutputKind::Rich(output) => output.dim_round(),
        }
    }

    pub fn finish(mut self, fallback: &str) -> Result<()> {
        if let TurnOutputKind::Rich(output) = &mut self.kind
            && !self.streamed
        {
            output.push(fallback)?;
        }

        match self.kind {
            TurnOutputKind::Raw(output) => output.finish(fallback),
            TurnOutputKind::Rich(output) => output.finish(),
        }
    }
}

impl RawStream {
    fn push(&mut self, _delta: &str) -> Result<()> {
        Ok(())
    }

    fn finish(&self, answer: &str) -> Result<()> {
        let answer = plain_answer(answer);
        if answer.is_empty() {
            return Ok(());
        }
        write_stdout(&[answer.as_bytes(), b"\n"])
    }
}

/// The answer without the blank lines models often put around it.
fn plain_answer(answer: &str) -> &str {
    answer
        .trim_matches(|character: char| character == '\n' || character == '\r')
        .trim_end()
}

pub fn write_stdout(parts: &[&[u8]]) -> Result<()> {
    let mut stdout = io::stdout().lock();
    for part in parts {
        match stdout.write_all(part) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(error) => {
                return Err(Error::new(
                    format!("could not write output: {error}"),
                    "check the output destination and try again",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::plain_answer;

    #[test]
    fn plain_answer_drops_surrounding_blank_lines_only() {
        assert_eq!(
            plain_answer("\n\n  indented\nline  \n\n"),
            "  indented\nline"
        );
        assert_eq!(plain_answer("\r\n\n"), "");
        assert_eq!(plain_answer("done"), "done");
    }
}
