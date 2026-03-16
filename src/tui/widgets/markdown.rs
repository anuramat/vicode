use std::sync::Arc;

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::tui::widgets::container::element::*;

// TODO maybe we can drop self_cell?

self_cell::self_cell!(
    pub struct MarkdownWidgetCell {
        owner: String,
        #[covariant]
        dependent: Paragraph,
    }
    impl {Debug}
);

#[derive(Debug, Clone)]
pub struct MarkdownWidget(pub Arc<MarkdownWidgetCell>);

impl HeightComputable for MarkdownWidget {
    fn height(
        &mut self,
        width: u16,
        _ctx: RenderContext,
    ) -> u16 {
        self.0.borrow_dependent().line_count(width) as u16
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        _ctx: RenderContext,
    ) {
        self.0.borrow_dependent().clone().render(area, buf);
    }
}

impl From<String> for MarkdownWidget {
    fn from(value: String) -> Self {
        let cell = MarkdownWidgetCell::new(value, |owner| {
            Paragraph::new(tui_markdown::from_str(owner)).wrap(Wrap { trim: false })
        });
        MarkdownWidget(Arc::new(cell))
    }
}
