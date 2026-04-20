use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use super::BashCall;
use super::BashResult;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::HeightComputable;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::container::sections::Section;
use crate::tui::widgets::container::sections::SectionList;
use crate::tui::widgets::message::toolcall::style;
use crate::tui::widgets::syntax::HIGHLIGHTER;

#[derive(Debug)]
struct BashWidget(SectionList);

impl HeightComputable for BashWidget {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        if ctx.hide_tools {
            return 1;
        }
        self.0.height(width, ctx)
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if ctx.hide_tools {
            self.0.render_title(area, buf, ctx);
        } else {
            self.0.render(area, buf, ctx);
        }
    }
}

impl From<&BashCall> for Element {
    fn from(value: &BashCall) -> Self {
        let (cmd, cmd_width) = value
            .arguments
            .as_ref()
            .map(|x| command(&x.command))
            .unzip();

        let (output, status) = match value.output.as_ref() {
            Some(Ok(result)) => (output(result), status(result)),
            Some(Err(err)) => (vec![app_error(err.clone())], None),
            None => (Vec::new(), None), // TODO show elapsed time
        };

        BashWidget(SectionList {
            sections: cmd.into_iter().chain(output).collect(),
            promote_at_width: cmd_width.flatten(),
            skip_first_header: true,
            title: "bash".into(),
            _right_title: status,
            style: style(),
        })
        .into()
    }
}

fn command(cmd: &str) -> (Section, Option<u16>) {
    let highlighted = HIGHLIGHTER.highlight(cmd.trim(), &HIGHLIGHTER.bash);

    let width = if let [line] = highlighted.lines.as_slice() {
        #[allow(clippy::cast_possible_truncation)]
        Some(line.width() as u16)
    } else {
        None
    };

    let element = Section::new("command", wrapped(highlighted), style());

    (element, width)
}

fn output(result: &BashResult) -> Vec<Section> {
    let mut sections = Vec::new();

    let stdout = Section::new(
        "stdout",
        wrapped(result.stdout.trim_end().to_string()),
        style(),
    );
    sections.push(stdout);

    let stderr_trimmed = result.stderr.trim_end();
    if !stderr_trimmed.is_empty() {
        sections.push(Section::new(
            "stderr",
            wrapped(stderr_trimmed.to_string()),
            style(),
        ));
    }

    sections
}

fn app_error(message: String) -> Section {
    Section::new("app error", wrapped(message), style())
}

fn status(result: &BashResult) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(code) = result.exit_status
        && code != 0
    {
        parts.push(format!("exit status {code}"));
    }

    if let Some(sig) = result.signal {
        parts.push(format!("signal {sig}"));
    }

    (!parts.is_empty()).then(|| parts.join(","))
}

fn wrapped(content: impl Into<Text<'static>>) -> Element {
    Paragraph::new(content).wrap(Wrap { trim: false }).into()
}
