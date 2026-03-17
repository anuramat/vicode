use ratatui::buffer::Buffer;
use ratatui::prelude::*;
use ratatui::text::Line;
use ratatui::widgets::WidgetRef;

#[derive(Clone)]
pub struct StatusLine<'a> {
    pub line: Line<'a>,
}

impl<'a> StatusLine<'a> {
    pub fn new(
        project_name: String,
        tab_name: Option<String>,
    ) -> Self {
        let mut line = Line::raw("");
        line.push_span(Span::styled(project_name, Style::new().dark_gray()));
        if let Some(tab_name) = tab_name {
            line.push_span(Span::styled("/", Style::new().dark_gray()));
            line.push_span(Span::raw(tab_name));
        };
        Self { line }
    }
}

impl<'a> WidgetRef for StatusLine<'a> {
    fn render_ref(
        &self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        self.line.render_ref(area, buf);
    }
}
