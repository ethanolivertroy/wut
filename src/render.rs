use std::io::{self, Write};

use unicode_width::UnicodeWidthStr;

use crate::config::ResponseColor;
use crate::error::{Error, Result};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const DARK_GRAY: &str = "\x1b[90m";

pub struct TerminalRenderer<W = io::Stdout> {
    output: W,
    pending: String,
    code: Option<CodeBlock>,
    wrote: bool,
    ends_with_newline: bool,
    list_active: bool,
    list_has_children: bool,
    response_color: ResponseColor,
}

#[derive(Default)]
struct CodeBlock {
    language: String,
    lines: Vec<String>,
}

impl TerminalRenderer<io::Stdout> {
    pub fn new(response_color: ResponseColor) -> Self {
        Self::with_writer(io::stdout(), response_color)
    }
}

impl<W: Write> TerminalRenderer<W> {
    fn with_writer(output: W, response_color: ResponseColor) -> Self {
        Self {
            output,
            pending: String::new(),
            code: None,
            wrote: false,
            ends_with_newline: false,
            list_active: false,
            list_has_children: false,
            response_color,
        }
    }

    pub fn push(&mut self, text: &str) -> Result<()> {
        self.pending.push_str(text);

        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending[..newline].to_owned();
            self.pending.drain(..=newline);
            self.line(&line)?;
        }

        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line)?;
        }

        if let Some(block) = self.code.take() {
            self.draw_code(block)?;
        }

        if self.wrote && !self.ends_with_newline {
            self.write("\n")?;
        }
        Ok(())
    }

    fn line(&mut self, line: &str) -> Result<()> {
        if let Some(block) = &mut self.code {
            if line.trim_start().starts_with("```") {
                let block = self.code.take().expect("code block exists");
                self.draw_code(block)
            } else {
                block.lines.push(line.to_owned());
                Ok(())
            }
        } else if let Some(language) = line.trim_start().strip_prefix("```") {
            self.list_active = false;
            self.list_has_children = false;
            self.code = Some(CodeBlock {
                language: language.trim().to_owned(),
                lines: Vec::new(),
            });
            Ok(())
        } else {
            self.draw_markdown_line(line)
        }
    }

    fn draw_markdown_line(&mut self, line: &str) -> Result<()> {
        if let Some(heading) = heading(line) {
            self.list_active = false;
            self.list_has_children = false;
            return self.write(&format!(
                "{BOLD}{CYAN}{}{RESET}\n",
                render_inline(heading, "")
            ));
        }

        if let Some(quote) = line.trim_start().strip_prefix("> ") {
            self.list_active = false;
            self.list_has_children = false;
            let color = self.response_color.ansi();
            let rendered = render_inline(quote, color);
            let line = if color.is_empty() {
                format!("{DARK_GRAY}│{RESET} {rendered}\n")
            } else {
                format!("{DARK_GRAY}│{RESET} {color}{rendered}{RESET}\n")
            };
            return self.write(&line);
        }

        if is_rule(line) {
            self.list_active = false;
            self.list_has_children = false;
            return self.write(&format!("{DARK_GRAY}────────────────────────{RESET}\n"));
        }

        if let Some((nested, content)) = list_item(line) {
            if nested {
                self.list_active = true;
                self.list_has_children = true;
                return self.write(&self.response_line(&format!(
                    "    - {}",
                    render_inline(content, self.response_color.ansi())
                )));
            }

            if self.list_active && (self.list_has_children || content.ends_with(':')) {
                self.write("\n")?;
            }
            self.list_active = true;
            self.list_has_children = false;
            return self.write(&self.response_line(&format!(
                "  • {}",
                render_inline(content, self.response_color.ansi())
            )));
        }

        if let Some((number, content)) = ordered_list_item(line) {
            self.list_active = true;
            self.list_has_children = false;
            let color = self.response_color.ansi();
            let rendered = render_inline(content, color);
            let line = if color.is_empty() {
                format!("  {CYAN}{number}.{RESET} {rendered}\n")
            } else {
                format!("{color}  {BOLD}{number}.{RESET}{color} {rendered}{RESET}\n")
            };
            return self.write(&line);
        }

        self.list_active = false;
        self.list_has_children = false;
        self.write(&self.response_line(&render_inline(line, self.response_color.ansi())))
    }

    fn response_line(&self, content: &str) -> String {
        let color = self.response_color.ansi();
        if color.is_empty() {
            format!("{content}\n")
        } else {
            format!("{color}{content}{RESET}\n")
        }
    }

    fn draw_code(&mut self, block: CodeBlock) -> Result<()> {
        let content_width = block
            .lines
            .iter()
            .map(|line| UnicodeWidthStr::width(line.as_str()))
            .max()
            .unwrap_or(0);
        let label_width = if block.language.is_empty() {
            0
        } else {
            UnicodeWidthStr::width(block.language.as_str()) + 1
        };
        let width = content_width.max(label_width).max(1);

        if block.language.is_empty() {
            self.write(&format!("{DARK_GRAY}┌{}┐{RESET}\n", "─".repeat(width + 2)))?;
        } else {
            self.write(&format!(
                "{DARK_GRAY}┌ {RESET}{CYAN}{}{RESET} {DARK_GRAY}{}┐{RESET}\n",
                block.language,
                "─".repeat(width - UnicodeWidthStr::width(block.language.as_str()))
            ))?;
        }

        for line in block.lines {
            let padding = width - UnicodeWidthStr::width(line.as_str());
            self.write(&format!(
                "{DARK_GRAY}│{RESET} {line}{} {DARK_GRAY}│{RESET}\n",
                " ".repeat(padding)
            ))?;
        }
        self.write(&format!("{DARK_GRAY}└{}┘{RESET}\n", "─".repeat(width + 2)))
    }

    fn write(&mut self, text: &str) -> Result<()> {
        match self
            .output
            .write_all(text.as_bytes())
            .and_then(|()| self.output.flush())
        {
            Ok(()) => {
                self.wrote = true;
                self.ends_with_newline = text.ends_with('\n');
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(Error::new(
                format!("could not write output: {error}"),
                "check the output destination and try again",
            )),
        }
    }
}

fn list_item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start_matches(' ');
    let content = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))?;
    Some((trimmed.len() != line.len(), content))
}

fn heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    ["### ", "## ", "# "]
        .into_iter()
        .find_map(|marker| trimmed.strip_prefix(marker))
}

fn ordered_list_item(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let dot = trimmed.find('.')?;
    let number = &trimmed[..dot];
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((number, trimmed[dot + 1..].strip_prefix(' ')?))
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 3
        && (trimmed.bytes().all(|byte| byte == b'-') || trimmed.bytes().all(|byte| byte == b'*'))
}

fn render_inline(line: &str, base_color: &str) -> String {
    let mut rendered = String::with_capacity(line.len());
    let mut remaining = line;

    while !remaining.is_empty() {
        let code = remaining.find('`').map(|index| (index, "`"));
        let bold = remaining.find("**").map(|index| (index, "**"));
        let Some((open, marker)) = [code, bold].into_iter().flatten().min_by_key(|item| item.0)
        else {
            rendered.push_str(remaining);
            break;
        };

        rendered.push_str(&remaining[..open]);
        let after_open = &remaining[open + marker.len()..];
        let Some(close) = after_open.find(marker) else {
            rendered.push_str(&remaining[open..]);
            break;
        };

        rendered.push_str(if marker == "`" && base_color.is_empty() {
            CYAN
        } else {
            BOLD
        });
        rendered.push_str(&after_open[..close]);
        rendered.push_str(RESET);
        rendered.push_str(base_color);
        remaining = &after_open[close + marker.len()..];
    }
    rendered
}
