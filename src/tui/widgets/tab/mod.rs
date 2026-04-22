pub mod input;

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::tui::tab::Tab;
use crate::tui::tab::TabEntry;
use crate::tui::widgets::container::element::RenderContext;

const INPUT_AREA_HEIGHT: u16 = 5;
const INFO_PANE_WIDTH: u16 = 32;

impl TabEntry<'_> {
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        match self {
            Self::Loading => render_loading(area, buf),
            Self::Ready(tab) => tab.render(area, buf, ctx),
        }
    }
}

impl Tab<'_> {
    #[tracing::instrument(skip(self, buf))]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let [body, info_area] = *Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Min(0),
                Constraint::Length(INFO_PANE_WIDTH),
            ])
            .split(area)
        else {
            unreachable!()
        };
        self.info.render(info_area, buf);

        let body = Rect {
            x: body.x + 1,
            height: body.height.saturating_sub(1),
            width: body.width.saturating_sub(2),
            ..body
        };

        let input_height = if self.input.visible() {
            INPUT_AREA_HEIGHT + 1
        } else {
            0
        };
        let [messages_area, input_area] = *Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Min(0), Constraint::Length(input_height)])
            .split(body)
        else {
            unreachable!()
        };

        self.scroll.render(
            &self.agent.state.context.history,
            messages_area.inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 0,
            }),
            buf,
            ctx,
        );
        self.input.render(input_area, buf);
    }
}

fn render_loading(
    area: Rect,
    buf: &mut Buffer,
) {
    let style = Style::default().add_modifier(Modifier::REVERSED);
    buf.set_style(area, style);

    let text = "LOADING".to_string();
    let area = Rect {
        x: area.x + ((area.width - text.len() as u16) / 2),
        y: area.y + area.height / 2,
        width: text.len() as u16,
        height: 1,
    };
    let widget = Paragraph::new(text);

    widget.render(area, buf);
}
