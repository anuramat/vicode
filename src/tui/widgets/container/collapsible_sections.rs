use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::symbols::line::HORIZONTAL;

use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::container::scroll::ScrollOp;

const MIN_COLLAPSED_CONTENT_HEIGHT: u16 = 40;

#[derive(Debug)]
pub struct CollapsibleSection {
    title: String,
    element: Element,
}

#[derive(Debug, Default)]
pub struct CollapsibleSections {
    sections: Vec<CollapsibleSection>,
    selected: usize,
}

impl CollapsibleSection {
    pub fn new(
        title: impl Into<String>,
        element: impl Into<Element>,
    ) -> Self {
        Self {
            title: title.into(),
            element: element.into(),
        }
    }

    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        self.element.height(width, ctx) + 1
    }

    fn render_title(
        &self,
        area: Rect,
        buf: &mut Buffer,
        selected: bool,
        suffix: Option<&str>,
    ) {
        let mut style = Style::default();
        if !selected {
            style = style.add_modifier(Modifier::DIM);
        }
        let line = HORIZONTAL.repeat(area.width.into());
        buf.set_string(area.x, area.y, line, style);
        if let Some(suffix) = suffix {
            buf.set_string(
                area.x + 1,
                area.y,
                format!(" {}{} ", self.title, suffix),
                style,
            );
        } else {
            buf.set_string(area.x + 1, area.y, format!(" {} ", self.title), style);
        }
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
        selected: bool,
        suffix: Option<&str>,
    ) {
        self.render_title(area, buf, selected, suffix);
        self.element.render(
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

impl CollapsibleSections {
    pub fn new(sections: impl IntoIterator<Item = CollapsibleSection>) -> Self {
        Self {
            sections: sections.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn scroll(
        &mut self,
        op: ScrollOp,
    ) {
        if self.sections.is_empty() {
            return;
        }
        match op {
            ScrollOp::Down => self.selected = (self.selected + 1).min(self.sections.len() - 1),
            ScrollOp::Up => self.selected = self.selected.saturating_sub(1),
            _ => {}
        }
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if self.sections.is_empty() || area.area() == 0 {
            return;
        }

        self.selected = self.selected.min(self.sections.len() - 1);
        if self.total_height(area.width, ctx) <= area.height {
            self.render_expanded(area, buf, ctx);
        } else if area.height.saturating_sub(self.sections.len() as u16)
            < MIN_COLLAPSED_CONTENT_HEIGHT
        {
            self.render_selected(area, buf, ctx);
        } else {
            self.render_collapsed(area, buf, ctx);
        }
    }

    fn total_height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        let mut result = 0;
        for section in &mut self.sections {
            result += section.height(width, ctx);
        }
        result
    }

    fn render_expanded(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let mut y = area.y;
        for (i, section) in self.sections.iter_mut().enumerate() {
            let height = section.height(area.width, ctx);
            section.render(
                Rect { y, height, ..area },
                buf,
                ctx,
                i == self.selected,
                None,
            );
            y += height;
        }
    }

    fn render_collapsed(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let mut y = area.y;
        let n_sections = self.sections.len() as u16;
        for (i, section) in self.sections.iter_mut().enumerate() {
            let selected = i == self.selected;
            if selected {
                let available_height = area.height - (n_sections - 1);
                section.render(
                    Rect {
                        y,
                        height: available_height,
                        ..area
                    },
                    buf,
                    ctx,
                    selected,
                    None,
                );
                y += available_height;
            } else {
                section.render_title(
                    Rect {
                        y,
                        height: 1,
                        ..area
                    },
                    buf,
                    selected,
                    None,
                );
                y += 1;
            }
        }
    }

    fn render_selected(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let suffix = format!(" [{}/{}]", self.selected + 1, self.sections.len());
        self.sections[self.selected].render(area, buf, ctx, true, Some(&suffix));
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::tui::widgets::container::element::HeightComputable;

    #[derive(Clone, Debug)]
    struct TestElement {
        height: u16,
        label: &'static str,
    }

    impl HeightComputable for TestElement {
        fn height(
            &mut self,
            _width: u16,
            _ctx: RenderContext,
        ) -> u16 {
            self.height
        }

        fn render(
            &mut self,
            area: Rect,
            buf: &mut Buffer,
            _ctx: RenderContext,
        ) {
            for y in area.y..area.bottom() {
                buf.set_string(area.x, y, self.label, Style::default());
            }
        }
    }

    fn sections() -> CollapsibleSections {
        CollapsibleSections::new(["one", "two", "three", "four", "five"].map(|title| {
            CollapsibleSection::new(
                title,
                TestElement {
                    height: 80,
                    label: title,
                },
            )
        }))
    }

    fn render(
        sections: &mut CollapsibleSections,
        width: u16,
        height: u16,
    ) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        sections.render(area, &mut buf, RenderContext::default());
        (0..height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..width {
                    line.push_str(buf[(x, y)].symbol());
                }
                line.replace(HORIZONTAL, "-").trim_end().to_string()
            })
            .collect()
    }

    fn expected<const N: usize>(lines: [&str; N]) -> Vec<String> {
        lines.into_iter().map(str::to_string).collect()
    }

    #[test]
    fn small_collapsed_pane_renders_only_selected_section_with_position() {
        let mut sections = sections();
        sections.scroll(ScrollOp::Down);

        assert_eq!(
            render(&mut sections, 18, 6),
            expected(["- two [2/5] ------", "two", "two", "two", "two", "two"])
        );
    }

    #[test]
    fn tiny_collapsed_pane_does_not_underflow() {
        let mut sections = sections();
        sections.scroll(ScrollOp::Down);
        sections.scroll(ScrollOp::Down);

        assert_eq!(
            render(&mut sections, 18, 2),
            expected(["- three [3/5] ----", "three"])
        );
    }

    #[test]
    fn large_collapsed_pane_keeps_other_section_titles() {
        let mut sections = sections();
        sections.scroll(ScrollOp::Down);
        let lines = render(&mut sections, 18, 50);

        assert_eq!(lines[0].contains("one"), true);
        assert_eq!(lines[1].contains("two"), true);
        assert_eq!(lines[47].contains("three"), true);
        assert_eq!(lines[48].contains("four"), true);
        assert_eq!(lines[49].contains("five"), true);
        assert_eq!(lines.iter().any(|line| line.contains("[2/5]")), false);
    }
}
