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
    /// Plain text written since the model's current round began, so the
    /// round can be redrawn as narration if it ends in tool calls.
    round: String,
    /// Terminal (columns, rows); `None` asks the terminal.
    size: Option<(usize, usize)>,
    /// Draw ```mermaid blocks as images.
    images: bool,
    /// The round drew an image, whose rows can't be counted to redraw it.
    round_has_image: bool,
}

#[derive(Default)]
struct CodeBlock {
    language: String,
    lines: Vec<String>,
}

impl TerminalRenderer<io::Stdout> {
    pub fn new(response_color: ResponseColor) -> Self {
        let mut renderer = Self::with_writer(io::stdout(), response_color);
        renderer.images = crate::diagram::images_supported();
        renderer
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
            round: String::new(),
            size: None,
            images: false,
            round_has_image: false,
        }
    }

    /// Redraws everything streamed in this round in gray, because the round
    /// ended in tool calls and so was narration ("Let me check the config.")
    /// rather than the answer. Blank lines are dropped. Output taller than the
    /// screen is left as it is, since the cursor can't reach it.
    pub fn dim_round(&mut self) -> Result<()> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line)?;
        }
        if let Some(block) = self.code.take() {
            self.draw_code(block)?;
        }
        self.list_active = false;
        self.list_has_children = false;
        let written = std::mem::take(&mut self.round);
        if std::mem::take(&mut self.round_has_image) || written.is_empty() {
            return Ok(());
        }
        let Some((columns, rows)) = self.size.or_else(terminal_size) else {
            return Ok(());
        };
        let used = rows_used(&written, columns);
        if used == 0 || used >= rows {
            return Ok(());
        }
        let mut redraw = format!("\x1b[{used}F\x1b[J");
        for line in written.lines().filter(|line| !line.trim().is_empty()) {
            redraw.push_str(&format!("{DARK_GRAY}{}{RESET}\n", line.trim_end()));
        }
        self.write(&redraw)?;
        self.round.clear();
        Ok(())
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
        let line: &str = &printable(line);
        // Models often open a round with blank lines; skip them so the text
        // starts right under the prompt or the last tool line.
        if self.code.is_none() && self.round.is_empty() && line.trim().is_empty() {
            return Ok(());
        }
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
        if self.images && block.language.eq_ignore_ascii_case("mermaid") {
            let columns = self
                .size
                .or_else(terminal_size)
                .map_or(80, |(columns, _)| columns)
                .saturating_sub(1);
            match crate::diagram::draw(&block.lines.join("\n"), columns) {
                Ok(escapes) => return self.write_image(&escapes),
                Err(reason) => {
                    self.draw_box(block)?;
                    return self.write(&format!(
                        "{DARK_GRAY}· diagram couldn't be drawn: {}{RESET}\n",
                        printable(&reason)
                    ));
                }
            }
        }
        self.draw_box(block)
    }

    /// Writes an image's escapes, then ends the line under it. The image isn't
    /// text, so it stays out of the round's text and marks the round as one
    /// that can't be redrawn.
    fn write_image(&mut self, escapes: &str) -> Result<()> {
        match self
            .output
            .write_all(escapes.as_bytes())
            .and_then(|()| self.output.write_all(b"\n"))
            .and_then(|()| self.output.flush())
        {
            Ok(()) => {
                self.wrote = true;
                self.ends_with_newline = true;
                self.round_has_image = true;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(Error::new(
                format!("could not write output: {error}"),
                "check the output destination and try again",
            )),
        }
    }

    fn draw_box(&mut self, block: CodeBlock) -> Result<()> {
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
                self.round.push_str(&without_colors(text));
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

fn terminal_size() -> Option<(usize, usize)> {
    let (columns, rows) = crossterm::terminal::size().ok()?;
    (columns > 0 && rows > 0).then_some((usize::from(columns), usize::from(rows)))
}

/// Terminal rows taken by `text`, counting lines that wrap.
fn rows_used(text: &str, columns: usize) -> usize {
    text.split_terminator('\n')
        .map(|line| UnicodeWidthStr::width(line).div_ceil(columns).max(1))
        .sum()
}

/// `text` without the color codes this renderer writes. Model output has
/// had its control characters removed, so these are the only escapes.
fn without_colors(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            characters.by_ref().find(|character| *character == 'm');
        } else {
            plain.push(character);
        }
    }
    plain
}

/// The line with control characters removed, so model output, which can
/// echo text from a prompt injection, cannot move the cursor, change
/// colors, set the window title or write the clipboard. Tabs become four
/// spaces so code blocks keep their width.
fn printable(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.chars().any(char::is_control) {
        return line.into();
    }
    let mut clean = String::with_capacity(line.len());
    for character in line.chars() {
        if character == '\t' {
            clean.push_str("    ");
        } else if !character.is_control() {
            clean.push(character);
        }
    }
    clean.into()
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

#[cfg(test)]
mod tests {
    use super::{TerminalRenderer, printable, rows_used};
    use crate::config::ResponseColor;

    fn render(chunks: &[&str], color: ResponseColor) -> String {
        let mut output = Vec::new();
        let mut renderer = TerminalRenderer::with_writer(&mut output, color);
        for chunk in chunks {
            renderer.push(chunk).unwrap();
        }
        renderer.finish().unwrap();
        String::from_utf8(output).unwrap()
    }

    /// The rendered text with our own color codes removed.
    fn plain(text: &str) -> String {
        let mut plain = String::new();
        let mut chars = text.chars();
        while let Some(character) = chars.next() {
            if character == '\x1b' {
                chars.by_ref().find(|c| *c == 'm');
            } else {
                plain.push(character);
            }
        }
        plain
    }

    #[test]
    fn streamed_chunks_render_like_the_whole_answer() {
        let answer = "# Title\nSome **bold** and `code`.\n- one\n  - nested\n1. first\n";
        let whole = render(&[answer], ResponseColor::Default);
        let pieces: Vec<String> = answer.chars().map(String::from).collect();
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        assert_eq!(render(&pieces, ResponseColor::Default), whole);
        assert_eq!(
            plain(&whole),
            "Title\nSome bold and code.\n  • one\n    - nested\n  1. first\n"
        );
    }

    #[test]
    fn code_blocks_are_boxed_even_when_left_open() {
        let closed = plain(&render(&["```sh\nls -la\n```\n"], ResponseColor::Default));
        assert_eq!(closed, "┌ sh ────┐\n│ ls -la │\n└────────┘\n");
        let open = plain(&render(&["```\nab"], ResponseColor::Default));
        assert_eq!(open, "┌────┐\n│ ab │\n└────┘\n");
    }

    #[test]
    fn markdown_inside_code_blocks_is_left_alone() {
        let text = plain(&render(
            &["```\n# not a heading\n- not a list\n```\n"],
            ResponseColor::Default,
        ));
        assert!(text.contains("│ # not a heading │"), "{text}");
        assert!(text.contains("│ - not a list    │"), "{text}");
    }

    #[test]
    fn rules_quotes_and_unclosed_markers() {
        let text = plain(&render(
            &["---\n> quoted\nhalf **open\n"],
            ResponseColor::Default,
        ));
        assert_eq!(text, "────────────────────────\n│ quoted\nhalf **open\n");
    }

    #[test]
    fn a_response_color_wraps_each_line() {
        let red = ResponseColor::Red.ansi();
        let text = render(&["hello\n"], ResponseColor::Red);
        assert_eq!(text, format!("{red}hello\x1b[0m\n"));
    }

    #[test]
    fn ends_the_last_line() {
        assert_eq!(
            render(&["no newline"], ResponseColor::Default),
            "no newline\n"
        );
        assert_eq!(render(&[], ResponseColor::Default), "");
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let hostile = "a\x1b]52;c;ZXZpbA==\x07b\x1b[2Jc\u{9b}31md\r\n";
        let text = render(&[hostile, "```\n\tx\n```\n"], ResponseColor::Default);
        assert!(text.starts_with("a]52;c;ZXZpbA==b[2Jc31md\n"), "{text:?}");
        assert!(plain(&text).contains("│     x │"), "{text:?}");
        assert_eq!(printable("plain"), "plain");
    }

    fn sized(output: &mut Vec<u8>, columns: usize, rows: usize) -> TerminalRenderer<&mut Vec<u8>> {
        let mut renderer = TerminalRenderer::with_writer(output, ResponseColor::Default);
        renderer.size = Some((columns, rows));
        renderer
    }

    #[test]
    fn narration_is_redrawn_gray_and_the_answer_is_not() {
        let mut output = Vec::new();
        let mut renderer = sized(&mut output, 80, 24);
        renderer
            .push("\n\nLet me check `main.rs`.\n\nOne moment")
            .unwrap();
        renderer.dim_round().unwrap();
        renderer.push("\n\nIt's in **main**.\n").unwrap();
        renderer.finish().unwrap();
        let text = String::from_utf8(output).unwrap();
        let (streamed, rest) = text.split_once("\x1b[3F\x1b[J").expect("moves up 3 rows");
        assert!(streamed.contains("Let me check"), "{streamed:?}");
        assert!(
            rest.starts_with("\x1b[90mLet me check main.rs.\x1b[0m\n\x1b[90mOne moment\x1b[0m\n"),
            "{rest:?}"
        );
        assert!(rest.ends_with("It's in \x1b[1mmain\x1b[0m.\n"), "{rest:?}");
    }

    #[test]
    fn nothing_to_dim_writes_nothing() {
        let mut output = Vec::new();
        let mut renderer = sized(&mut output, 80, 24);
        renderer.dim_round().unwrap();
        renderer.finish().unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn narration_taller_than_the_screen_is_left_alone() {
        let mut output = Vec::new();
        let mut renderer = sized(&mut output, 80, 3);
        renderer.push("one\ntwo\nthree\n").unwrap();
        renderer.dim_round().unwrap();
        renderer.finish().unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "one\ntwo\nthree\n");
    }

    #[test]
    fn counts_wrapped_rows() {
        assert_eq!(rows_used("\n", 10), 1);
        assert_eq!(rows_used("abcdefghij\n", 10), 1);
        assert_eq!(rows_used("abcdefghijk\n\nxy\n", 10), 4);
        assert_eq!(rows_used("界界界界界界\n", 10), 2);
    }

    #[test]
    fn mermaid_blocks_become_images_when_supported() {
        let source = "```mermaid\nflowchart LR\n  A --> B\n```\n";
        let mut output = Vec::new();
        let mut renderer = sized(&mut output, 80, 24);
        renderer.images = true;
        renderer.push(source).unwrap();
        renderer.push("after\n").unwrap();
        renderer.finish().unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(
            text.starts_with("\x1b_Ga=T,f=100,q=2,"),
            "{:?}",
            &text[..40]
        );
        assert!(
            text.ends_with("\x1b\\\nafter\n"),
            "{:?}",
            &text[text.len() - 20..]
        );
        assert!(!text.contains("flowchart"), "the source isn't printed");

        let boxed = plain(&render(&[source], ResponseColor::Default));
        assert!(boxed.contains("│ flowchart LR │"), "{boxed}");
    }

    #[test]
    fn a_broken_diagram_shows_its_source_and_the_reason() {
        let mut output = Vec::new();
        let mut renderer = sized(&mut output, 80, 24);
        renderer.images = true;
        renderer
            .push("```mermaid\nflowchart LR\n  A --> \n```\n")
            .unwrap();
        renderer.finish().unwrap();
        let text = plain(&String::from_utf8(output).unwrap());
        assert!(text.contains("│ flowchart LR │"), "{text}");
        assert!(
            text.contains("· diagram couldn't be drawn: Parse error on line 2"),
            "{text}"
        );
    }

    #[test]
    fn a_round_with_an_image_is_not_redrawn() {
        let mut output = Vec::new();
        let mut renderer = sized(&mut output, 80, 24);
        renderer.images = true;
        renderer
            .push("Here:\n```mermaid\nflowchart LR\n  A --> B\n```\n")
            .unwrap();
        renderer.dim_round().unwrap();
        renderer.finish().unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains("\x1b[J"), "no redraw over an image");
    }
}
