/// WARN vibecode-quality handcode
/// TODO unfuck
use ansi_to_tui::IntoText;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;
use ratatui::widgets::block::Title;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use syntect::util::as_24_bit_terminal_escaped;

use super::*;
use crate::tui::widgets::container::composite::CompositeElement;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::HeightComputable;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::message::toolcall::style;

static SYNTAX_SET: std::sync::LazyLock<SyntaxSet> =
    std::sync::LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: std::sync::LazyLock<ThemeSet> = std::sync::LazyLock::new(ThemeSet::load_defaults);

impl From<&BashCall> for Element {
    fn from(value: &BashCall) -> Self {
        let mut line = None;
        let output = value.output.as_ref().map(|x| match x {
            Ok(result) => {
                let (x, y) = result_to_element(result);
                line = y;
                x
            }
            Err(err) => text_element("app error", err.to_string()),
        });
        let widget = BashWidget {
            // TODO push colored text to name
            command: value.arguments.as_ref().map(|x| {
                let str = x.command.trim().to_string();
                CommandWrapped {
                    no_newlines: !str.contains("\n"),
                    element: bash_to_element(&str),
                    str,
                }
            }),
            output,
            width: 0,
            status_line: line,
        };
        widget.into()
    }
}

#[derive(Debug, Clone)]
struct CommandWrapped {
    str: String,
    no_newlines: bool,
    element: Element,
}

#[derive(Debug, Clone)]
struct BashWidget {
    command: Option<CommandWrapped>,
    output: Option<Element>,
    width: u16,
    status_line: Option<Line<'static>>,
}

impl BashWidget {
    fn oneliner(
        &self,
        command: &Option<CommandWrapped>,
    ) -> bool {
        command
            .as_ref()
            .is_none_or(|x| x.no_newlines && (x.str.len() as u16) < self.width)
    }

    fn title(&self) -> Line<'static> {
        if self.oneliner(&self.command)
            && let Some(command) = self.command.as_ref()
        {
            bash_to_text(&command.str).lines[0].clone()
        } else {
            "bash".into()
        }
    }
}

impl HeightComputable for BashWidget {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        self.width = width;
        if ctx.hide_tools {
            return 1;
        }
        let mut height = 0;
        if let Some(inner) = &mut self.output {
            height += inner.height(width, ctx);
        }
        if !self.oneliner(&self.command)
            && let Some(command) = &mut self.command
        {
            height += command.element.height(width, ctx)
        }
        height.max(1)
    }

    fn render(
        &mut self,
        mut area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        self.width = area.width;
        if ctx.hide_tools || self.height(self.width, ctx) == 1 {
            return Paragraph::new(self.title()).render_ref(area, buf);
        }
        if !self.oneliner(&self.command)
            && let Some(command) = &mut self.command
        {
            let height = command.element.height(self.width, ctx);
            command.element.render(Rect { height, ..area }, buf, ctx);
            area.height -= height;
            area.y += height;
        };
        if let Some(element) = &mut self.output {
            element.render(area, buf, ctx)
        }
    }

    fn block(
        &self,
        ctx: RenderContext,
    ) -> Option<Block<'_>> {
        if ctx.hide_tools {
            return None;
        }
        let mut title = self.title();
        title.spans.insert(0, " ".into());
        title.spans.push(" ".into());
        let title = Title::from(title);
        let mut block = ratatui::widgets::Block::bordered()
            .border_set(ratatui::symbols::border::PLAIN)
            .style(style())
            .title(title);
        if let Some(status_line) = &self.status_line {
            let title =
                Title::from(status_line.clone()).alignment(ratatui::layout::Alignment::Right);
            block = block.title(title);
        }
        block.into()
    }
}

// TODO rename these, maybe move to widgets

#[derive(Debug, Clone)]
struct SectionElement<T>
where T: HeightComputable + Clone
{
    title: String,
    inner: T,
}

impl<T> HeightComputable for SectionElement<T>
where T: HeightComputable + Clone
{
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        self.inner.height(width, ctx)
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        self.inner.render(area, buf, ctx);
    }

    fn block(
        &self,
        _ctx: RenderContext,
    ) -> Option<Block<'_>> {
        // TODO use vertical left/right symbols on the ends of the top line
        Block::new()
            .borders(Borders::TOP)
            .border_set(ratatui::symbols::border::PLAIN)
            .style(style())
            .title(format!(
                "{} {} ",
                ratatui::symbols::line::HORIZONTAL,
                self.title
            ))
            .into()
    }
}

fn text_element(
    title: impl Into<String>,
    text: impl Into<Text<'static>>,
) -> Element {
    let inner = Paragraph::new(text.into()).wrap(Wrap { trim: false });
    SectionElement {
        title: title.into(),
        inner,
    }
    .into()
}

fn result_to_element(result: &BashResult) -> (Element, Option<Line<'static>>) {
    let BashResult {
        stdout,
        stderr,
        exit_status,
        signal,
    } = result;

    let status_line = {
        let mut parts = Vec::new();
        if let Some(exit_status) = exit_status
            && *exit_status != 0
        {
            parts.push(format!("exit status {}", exit_status));
        }
        if let Some(signal) = signal {
            parts.push(format!("signal {}", signal));
        }

        (!parts.is_empty()).then(|| parts.join(",").into())
    };

    let stdout_element = text_element("stdout", stdout.trim_end().to_string());

    let stderr = stderr.trim_end();
    if stderr.trim().is_empty() {
        return (stdout_element, status_line);
    }

    (
        CompositeElement(vec![
            stdout_element,
            text_element("stderr", stderr.to_string()),
        ])
        .into(),
        status_line,
    )
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

fn bash_to_element(script: &str) -> Element {
    Paragraph::new(bash_to_text(script))
        .wrap(Wrap { trim: false })
        .into()
}
