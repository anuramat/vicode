use ratatui::prelude::*;
use ratatui::widgets::Clear;

use super::*;

impl<T, U> ScrollElements<T, U>
where
    T: AsRef<[U]>,
    U: IntoElement,
{
    #[tracing::instrument(skip(self, buf))]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        self.width = area.width;
        self.height = area.height;
        self.ctx = ctx;

        if self.len() == 0 || area.area() == 0 {
            return;
        }

        self.track_resize(area);

        if matches!(self.mode, Mode::Tail) || self.render_from_line(area, buf) > 0 {
            // two renders will only happen when history fits on screen && the user tries to scroll
            self.bottom();
            self.render_tail(area, buf);
        }
    }

    // TODO check and refactor below

    #[tracing::instrument(skip(self, buf))]
    fn render_from_line(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) -> u16 {
        Clear.render(area, buf);
        let mut remaining = self.render_first(area, buf);
        let mut idx = self.start.idx;
        while remaining > 0 && idx < self.len() - 1 {
            idx += 1;
            remaining = self.render_down(area, buf, idx, remaining);
        }
        remaining
    }

    #[tracing::instrument(skip(self, buf))]
    fn render_tail(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        Clear.render(area, buf);
        let mut offset = 0;
        let mut remaining = area.height;
        let mut idx = self.len();
        while remaining > 0 && idx > 0 {
            idx -= 1;
            (remaining, offset) = self.render_up(area, buf, idx, remaining);
        }

        self.start = StartLocation {
            idx,
            offset,
            relative_offset: None,
            ..self.start
        }
    }

    /// keeps the same relative offset when the height of the widget changes
    #[tracing::instrument(skip(self))]
    fn track_resize(
        &mut self,
        area: Rect,
    ) {
        let new_height = self.height(self.start.idx);
        if new_height == self.start.height {
            return;
        }
        let relative = if let Some(relative) = self.start.relative_offset {
            relative
        } else {
            let relative = (self.start.offset as f32) / (self.start.height as f32);
            self.start.relative_offset = Some(relative);
            relative
        };
        let max_offset = new_height.saturating_sub(1);
        let new_offset = (relative * (new_height as f32)) as u16;
        self.start = StartLocation {
            offset: new_offset.min(max_offset),
            height: new_height,
            ..self.start
        }
    }

    #[tracing::instrument(skip(self, buf))]
    fn render_first(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) -> u16 {
        let visible = (self.height(self.start.idx) - self.start.offset).min(area.height);
        let offset = self.start.offset;
        let ctx = self.ctx;
        self.element(self.start.idx).partial_render(
            Rect {
                height: visible,
                ..area
            },
            buf,
            offset,
            ctx,
        );
        area.height - visible
    }

    #[tracing::instrument(skip(self, buf))]
    fn render_down(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        idx: usize,
        remaining: u16,
    ) -> u16 {
        let visible = self.height(idx).min(remaining);
        let ctx = self.ctx;
        self.element(idx).partial_render(
            Rect {
                y: area.y + area.height - remaining,
                height: visible,
                ..area
            },
            buf,
            0,
            ctx,
        );
        remaining - visible
    }

    #[tracing::instrument(skip(self, buf))]
    fn render_up(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        idx: usize,
        remaining: u16,
    ) -> (u16, u16) {
        let element = self.height(idx);
        let visible = element.min(remaining);
        let offset = element - visible;
        let ctx = self.ctx;
        self.element(idx).partial_render(
            Rect {
                y: area.y + remaining - visible,
                height: visible,
                ..area
            },
            buf,
            offset,
            ctx,
        );
        (remaining - visible, offset)
    }
}
