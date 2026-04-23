use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols;
use ratatui::symbols::line::HORIZONTAL;
use ratatui::widgets::Block;
use ratatui::widgets::WidgetRef;

use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::HeightComputable;
use crate::tui::widgets::container::element::RenderContext;

#[derive(Debug)]
pub struct Section {
    title: String,
    inner: Element,
    style: Style,
}

#[derive(Debug)]
pub struct SectionList {
    pub sections: Vec<Section>,
    /// width of the first section:
    /// when set, the body of the first section is a single line,
    /// and if it fits in the area, it replaces the title
    pub promote_at_width: Option<u16>,
    /// when true, the first section is rendered without its title; acts after the promotion logic
    pub skip_first_header: bool,
    pub title: String,
    pub _right_title: Option<String>, // TODO use this: check if it fits with the render_effective_title and use same/similar logic on the right
    pub style: Style,
}

impl Section {
    pub fn new(
        title: impl Into<String>,
        inner: impl Into<Element>,
        style: Style,
    ) -> Self {
        Self {
            title: title.into(),
            inner: inner.into(),
            style,
        }
    }

    fn render_divider(
        &self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        // NOTE we render the line outside of our area so that right/left symbols connect to the border of the parent block
        let line = format!(
            "{}{}{}",
            symbols::line::VERTICAL_RIGHT,
            HORIZONTAL.repeat(area.width.into()),
            symbols::line::VERTICAL_LEFT
        );
        buf.set_string(area.x - 1, area.y, line, self.style);
        buf.set_string(area.x + 1, area.y, format!(" {} ", self.title), self.style);
    }

    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        self.inner.height(width, ctx) + 1
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        self.render_divider(area, buf);
        self.inner.render(
            Rect {
                y: area.y + 1,
                height: area.height.saturating_sub(1),
                ..area
            },
            buf,
            ctx,
        );
    }
}

impl SectionList {
    const fn should_promote(
        &self,
        width: u16,
    ) -> bool {
        if let Some(w) = self.promote_at_width {
            w <= width.saturating_sub(4) // 2 from borders, 2 from padding
        } else {
            false
        }
    }

    /// renders title/promoted section;
    /// returns the width of the rendered title
    pub fn render_title(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) -> u16 {
        if self.should_promote(area.width)
            && let Some(section) = self.sections.first_mut()
        {
            section.inner.render(area, buf, ctx);
            self.promote_at_width.unwrap_or(0)
        } else {
            buf.set_string(area.x, area.y, &self.title, self.style);
            self.title.len() as u16
        }
    }

    fn body_sections(
        &mut self,
        width: u16,
    ) -> impl Iterator<Item = &mut Section> {
        let skip = self.should_promote(width).into();
        let iterator = self.sections.iter_mut();
        iterator.skip(skip)
    }

    fn heights(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> (u16, Vec<Constraint>) {
        let skip = self.skip_first_header;
        let inner_width = width.saturating_sub(2);

        let mut heights = Vec::with_capacity(self.sections.len());
        let mut total: u16 = 0;
        for (i, e) in self.body_sections(width).enumerate() {
            let h = if i == 0 && skip {
                e.inner.height(inner_width, ctx)
            } else {
                e.height(inner_width, ctx)
            };
            heights.push(Constraint::Length(h));
            total += h;
        }
        (total, heights)
    }
}

impl HeightComputable for SectionList {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        self.heights(width, ctx).0 + 2
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let width = area.width;

        let block = Block::bordered()
            .border_set(ratatui::symbols::border::PLAIN)
            .style(self.style);
        block.render_ref(area, buf);

        {
            let area = Rect {
                x: area.x + 2,
                height: 1,
                ..area
            };
            let title_width = self.render_title(area, buf, ctx);
            buf.set_string(area.x - 1, area.y, " ", self.style);
            buf.set_string(area.x + title_width, area.y, " ", self.style);
        }

        let area = block.inner(area);
        let constraints = self.heights(width, ctx).1;
        let areas = Layout::vertical(constraints).split(area);
        let skip = self.skip_first_header;
        for (i, e) in self.body_sections(width).enumerate() {
            if i == 0 && skip {
                e.inner.render(areas[i], buf, ctx);
            } else {
                e.render(areas[i], buf, ctx);
            }
        }
    }
}
