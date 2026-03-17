use ratatui::prelude::*;
use ratatui::widgets::Clear;
use tracing::debug;

use super::*;

impl<U> ScrollElements<U>
where U: IntoElement
{
    #[tracing::instrument(skip(self, data, buf))]
    pub fn render(
        &mut self,
        data: &[U],
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        self.width = area.width;
        self.height = area.height;
        self.ctx = ctx;
        self.set_len(data.len());

        if data.is_empty() || area.area() == 0 {
            return;
        }

        self.track_resize(data, area);

        debug!("starting render, mode: {:?}", self.mode);
        if matches!(self.mode, Mode::Tail) || self.render_from_line(data, area, buf) > 0 {
            // two renders will only happen when history fits on screen && the user tries to scroll
            self.bottom();
            self.render_tail(data, area, buf);
        }
    }

    // TODO check and refactor below

    #[tracing::instrument(skip(self, data, buf))]
    fn render_from_line(
        &mut self,
        data: &[U],
        area: Rect,
        buf: &mut Buffer,
    ) -> u16 {
        Clear.render(area, buf);
        let mut remaining = self.render_first(data, area, buf);
        let mut idx = self.start.idx;
        while remaining > 0 && idx < data.len() - 1 {
            idx += 1;
            remaining = self.render_down(data, area, buf, idx, remaining);
        }
        debug!("render done; {remaining} empty lines");
        remaining
    }

    #[tracing::instrument(skip(self, data, buf))]
    fn render_tail(
        &mut self,
        data: &[U],
        area: Rect,
        buf: &mut Buffer,
    ) {
        debug!("rendering tail");
        Clear.render(area, buf);
        let mut offset = 0;
        let mut remaining = area.height;
        let mut idx = data.len();
        while remaining > 0 && idx > 0 {
            idx -= 1;
            (remaining, offset) = self.render_up(data, area, buf, idx, remaining);
        }

        self.start = StartLocation {
            idx,
            offset,
            relative_offset: None,
            ..self.start
        }
    }

    /// keeps the same relative offset when the height of the widget changes
    #[tracing::instrument(skip(self, data))]
    fn track_resize(
        &mut self,
        data: &[U],
        area: Rect,
    ) {
        let new_height = self.height(data, self.start.idx);
        if new_height == self.start.height {
            return;
        }
        debug!(
            "element height changed to {}; old values: {:?}",
            new_height, self.start
        );
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

    #[tracing::instrument(skip(self, data, buf))]
    fn render_first(
        &mut self,
        data: &[U],
        area: Rect,
        buf: &mut Buffer,
    ) -> u16 {
        let visible = (self.height(data, self.start.idx) - self.start.offset).min(area.height);
        let offset = self.start.offset;
        let ctx = self.ctx;
        self.element(data, self.start.idx).partial_render(
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

    #[tracing::instrument(skip(self, data, buf))]
    fn render_down(
        &mut self,
        data: &[U],
        area: Rect,
        buf: &mut Buffer,
        idx: usize,
        remaining: u16,
    ) -> u16 {
        let visible = self.height(data, idx).min(remaining);
        let ctx = self.ctx;
        self.element(data, idx).partial_render(
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

    #[tracing::instrument(skip(self, data, buf))]
    fn render_up(
        &mut self,
        data: &[U],
        area: Rect,
        buf: &mut Buffer,
        idx: usize,
        remaining: u16,
    ) -> (u16, u16) {
        let element = self.height(data, idx);
        let visible = element.min(remaining);
        let offset = element - visible;
        let ctx = self.ctx;
        self.element(data, idx).partial_render(
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
