use ratatui::buffer::Buffer;
use ratatui::prelude::*;
use ratatui::text::Line;

#[derive(Clone)]
pub struct StatusLine {
    pub text: String,
}

impl StatusLine {
    pub fn new() -> Self {
        let status_text = crate::project::PROJECT
            .root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Self { text: status_text }
    }
}

impl Widget for &StatusLine {
    fn render(
        self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let line = Line::from(self.text.as_str());
        line.render(area, buf);
    }
}
