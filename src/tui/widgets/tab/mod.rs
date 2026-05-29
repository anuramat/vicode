pub mod input;

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;

use crate::tui::tab::Tab;
use crate::tui::widgets::container::element::RenderContext;

const INPUT_AREA_HEIGHT: u16 = 5;

impl Tab<'_> {
    #[tracing::instrument(skip(self, buf))]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let input_height = if self.input.visible() {
            INPUT_AREA_HEIGHT + 1
        } else {
            0
        };

        let [messages_area, input_area] = *Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Min(0), Constraint::Length(input_height)])
            .split(area)
        else {
            unreachable!()
        };

        self.scroll.render(
            self.state.context.history.state().messages.as_slice(),
            messages_area,
            buf,
            ctx,
        );
        self.input.render(input_area, buf);
    }
}
