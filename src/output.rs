use std::io::{self, Write};

use crate::config::ResponseColor;
use crate::error::{Error, Result};
use crate::render::TerminalRenderer;

#[derive(Default)]
struct RawStream {
    wrote: bool,
    ends_with_newline: bool,
}

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
                TurnOutputKind::Raw(RawStream::default())
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
    fn push(&mut self, delta: &str) -> Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        self.wrote = true;
        self.ends_with_newline = delta.ends_with('\n');
        write_stdout(&[delta.as_bytes()])
    }

    fn finish(&self, fallback: &str) -> Result<()> {
        if !self.wrote {
            print_answer(fallback)
        } else if !self.ends_with_newline {
            write_stdout(&[b"\n"])
        } else {
            Ok(())
        }
    }
}

fn print_answer(answer: &str) -> Result<()> {
    if answer.ends_with('\n') {
        write_stdout(&[answer.as_bytes()])
    } else {
        write_stdout(&[answer.as_bytes(), b"\n"])
    }
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
