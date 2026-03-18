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
const MAX_ONELINER_LENGTH: usize = 20; // TODO screen size?

impl From<&BashCall> for Element {
    fn from(value: &BashCall) -> Self {
        let mut name = "bash".to_string();
        let paragraph = value.arguments.as_ref().map(|args| {
            let mut texts: Vec<Text<'_>> = Vec::new();

            let command = args.command.trim();
            if command.len() <= MAX_ONELINER_LENGTH {
                name = format!("bash: {}", command);
            }
            texts.push(bash_to_text(command));

            if let Some(output) = &value.output {
                texts.push("\n".into());
                match output {
                    Ok(result) => texts.extend(result_to_texts(result)),
                    Err(err) => {
                        texts.push(format!("app error:\n{}", err).into());
                    }
                }
            };
            texts_to_paragraph(texts)
        });

        let widget = ToolCallWidget {
            name: "bash".to_string(),
            inner: paragraph,
        };
        widget.into()
    }
}

fn result_to_texts(result: &BashResult) -> Vec<Text<'static>> {
    let mut texts = Vec::new();

    let BashResult {
        stdout,
        stderr,
        exit_status,
        signal,
    } = result;

    if let Some(status) = *exit_status
        && status != 0
    {
        texts.push(format!("status: {}", status).into());
    }

    if let Some(signal) = signal {
        texts.push(format!("signal: {}", signal).into());
    }

    let stderr = stderr.trim();
    if !stderr.is_empty() {
        texts.push(format!("stderr:\n{}", stderr).into());
    }

    // TODO: better visual separation, ideally -- blocks

    let stdout = stdout.trim();
    if !stdout.is_empty() {
        texts.push(format!("stdout:\n{}", stdout).into());
    }

    texts
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

fn texts_to_paragraph(texts: Vec<Text<'static>>) -> Paragraph<'static> {
    let lines: Vec<_> = texts.into_iter().flat_map(|t| t.lines).collect();
    let text = Text::from(lines);
    Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: false })
}
