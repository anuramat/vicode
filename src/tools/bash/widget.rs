use ansi_to_tui::IntoText;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use syntect::util::as_24_bit_terminal_escaped;

use super::*;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::message::toolcall::ToolCallWidget;

static SYNTAX_SET: std::sync::LazyLock<SyntaxSet> =
    std::sync::LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: std::sync::LazyLock<ThemeSet> = std::sync::LazyLock::new(ThemeSet::load_defaults);

impl From<&BashCall> for Element {
    fn from(value: &BashCall) -> Self {
        let widget = ToolCallWidget {
            name: "bash".to_string(),
            inner: ratatui::widgets::Paragraph::from(value)
                .wrap(ratatui::widgets::Wrap { trim: false }),
        };
        widget.into()
    }
}

impl From<&BashCall> for Paragraph<'_> {
    fn from(task: &BashCall) -> Self {
        let mut texts = Vec::new();
        let command = task
            .arguments
            .as_ref()
            .map(|x| x.command.trim())
            .unwrap_or_default();
        texts.push(bash_to_text(command));

        let BashResult {
            stdout,
            stderr,
            exit_status,
            signal,
        } = match &task.output {
            Some(output) => match output {
                Ok(result) => result,
                Err(err) => {
                    texts.push(format!("\nerror: {}", err).into());
                    return texts_to_paragraph(texts);
                }
            },
            None => {
                texts.push("\n<pending>".into());
                return texts_to_paragraph(texts);
            }
        };

        let stdout = stdout.trim();
        if !stdout.is_empty() {
            texts.push(format!("\n{}", stdout).into());
        }

        let stderr = stderr.trim();
        if !stderr.is_empty() {
            texts.push(format!("\nstderr: {}", stderr).into());
        }

        if let Some(status) = *exit_status
            && status != 0
        {
            texts.push(format!("\nstatus: {}", status).into());
        }

        if let Some(signal) = signal {
            texts.push(format!("\nsignal: {}", signal).into());
        }

        texts_to_paragraph(texts)
    }
}

fn bash_to_text(script: &str) -> Text<'static> {
    // TODO optimize, parameterize theme
    let syntax = SYNTAX_SET.find_syntax_by_token("bash").unwrap();
    let theme = &THEME_SET.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);
    LinesWithEndings::from(script)
        .filter_map(|line| highlighter.highlight_line(line, &SYNTAX_SET).ok())
        .filter_map(|parts| as_24_bit_terminal_escaped(&parts, false).into_text().ok())
        .flatten()
        .collect()
}

fn texts_to_paragraph(texts: Vec<Text<'_>>) -> Paragraph<'_> {
    let lines: Vec<_> = texts.into_iter().flat_map(|t| t.lines).collect();
    let text = Text::from(lines);
    Paragraph::new(text)
}
