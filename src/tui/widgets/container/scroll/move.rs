use super::*;
use crate::tui::widgets::container::element::IntoElement;

impl ScrollElements {
    /// put the next message at the top
    pub fn next_element<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        self.mode = Mode::Scrolling;

        let mut idx = self.start.idx + 1;
        while idx < data.len() && self.height(data, idx) == 0 {
            idx += 1;
        }

        if idx >= data.len() {
            return self.bottom();
        }

        self.set_start(data, idx, 0);
    }

    // put the current message at the top if it's not; previous otherwise
    pub fn prev_element<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        self.mode = Mode::Scrolling;

        let idx = if self.start.offset != 0 {
            self.start.idx
        } else {
            let mut idx = self.start.idx.saturating_sub(1);
            while idx > 0 && self.height(data, idx) == 0 {
                idx -= 1;
            }
            idx
        };

        self.set_start(data, idx, 0);
    }

    pub fn top<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        self.set_start(data, 0, 0);
        self.mode = Mode::Scrolling;
    }

    pub fn bottom(&mut self) {
        tracing::debug!("scrolling to bottom");
        self.start = Default::default();
        self.mode = Mode::Tail;
    }

    pub fn half_page_down<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        let delta = (self.height / 2).max(1);
        self.add_offset_down(data, delta)
    }

    pub fn half_page_up<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        let delta = (self.height / 2).max(1);
        self.add_offset_up(data, delta)
    }

    pub fn page_down<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        self.add_offset_down(data, self.height)
    }

    pub fn page_up<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        self.add_offset_up(data, self.height)
    }

    pub fn line_down<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        self.add_offset_down(data, 1)
    }

    pub fn line_up<U>(
        &mut self,
        data: &[U],
    ) where
        U: IntoElement,
    {
        self.add_offset_up(data, 1)
    }

    #[tracing::instrument(skip(self, data))]
    fn add_offset_down<U>(
        &mut self,
        data: &[U],
        mut delta: u16,
    ) where
        U: IntoElement,
    {
        self.mode = Mode::Scrolling;

        let visible = self.height(data, self.start.idx) - self.start.offset;
        // NOTE "<" specifically, because if delta == element, we need to skip to the next element
        if delta < visible {
            return self.set_start(data, self.start.idx, self.start.offset + delta);
        }
        delta -= visible;

        let mut idx = self.start.idx + 1;
        let mut offset = 0;

        while delta > 0 && idx < data.len() {
            let height = self.height(data, idx);
            if delta < height {
                offset = delta;
                break;
            }
            delta -= height;
            idx += 1;
        }
        if idx >= data.len() {
            return self.bottom();
        }
        self.set_start(data, idx, offset);
    }

    #[tracing::instrument(skip(self, data))]
    fn add_offset_up<U>(
        &mut self,
        data: &[U],
        mut delta: u16,
    ) where
        U: IntoElement,
    {
        self.mode = Mode::Scrolling;

        // NOTE "<=" or "<" doesn't matter because of delta > 0 check
        if delta <= self.start.offset {
            return self.set_start(data, self.start.idx, self.start.offset - delta);
        }
        delta -= self.start.offset;

        let mut idx = self.start.idx;
        let mut offset = 0;

        while delta > 0 && idx > 0 {
            idx -= 1;
            let height = self.height(data, idx);
            if delta <= height {
                offset = height - delta;
                break;
            }
            delta -= height;
        }
        self.set_start(data, idx, offset);
    }
}
