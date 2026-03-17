use super::*;
use crate::tui::widgets::container::element::IntoElement;

impl<T, U> ScrollElements<T, U>
where
    T: AsRef<[U]>,
    U: IntoElement,
{
    /// put the next message at the top
    pub fn next_element(&mut self) {
        self.mode = Mode::Scrolling;

        let mut idx = self.start.idx + 1;
        while idx < self.len() && self.height(idx) == 0 {
            idx += 1;
        }

        if idx >= self.len() {
            return self.bottom();
        }

        self.set_start(idx, 0);
    }

    // put the current message at the top if it's not; previous otherwise
    pub fn prev_element(&mut self) {
        self.mode = Mode::Scrolling;

        let idx = if self.start.offset != 0 {
            self.start.idx
        } else {
            let mut idx = self.start.idx.saturating_sub(1);
            while idx > 0 && self.height(idx) == 0 {
                idx -= 1;
            }
            idx
        };

        self.set_start(idx, 0);
    }

    pub fn top(&mut self) {
        self.set_start(0, 0);
        self.mode = Mode::Scrolling;
    }

    pub fn bottom(&mut self) {
        tracing::debug!("scrolling to bottom");
        self.set_start(0, 0);
        self.mode = Mode::Tail;
    }

    pub fn half_page_down(&mut self) {
        let delta = (self.height / 2).max(1);
        self.add_offset_down(delta)
    }

    pub fn half_page_up(&mut self) {
        let delta = (self.height / 2).max(1);
        self.add_offset_up(delta)
    }

    pub fn page_down(&mut self) {
        self.add_offset_down(self.height)
    }

    pub fn page_up(&mut self) {
        self.add_offset_up(self.height)
    }

    pub fn line_down(&mut self) {
        self.add_offset_down(1)
    }

    pub fn line_up(&mut self) {
        self.add_offset_up(1)
    }

    #[tracing::instrument(skip(self))]
    fn add_offset_down(
        &mut self,
        mut delta: u16,
    ) {
        self.mode = Mode::Scrolling;

        let visible = self.height(self.start.idx) - self.start.offset;
        // NOTE "<" specifically, because if delta == element, we need to skip to the next element
        if delta < visible {
            return self.set_start(self.start.idx, self.start.offset + delta);
        }
        delta -= visible;

        let mut idx = self.start.idx + 1;
        let mut offset = 0;

        while delta > 0 && idx < self.len() {
            let height = self.height(idx);
            if delta < height {
                offset = delta;
                break;
            }
            delta -= height;
            idx += 1;
        }
        if idx >= self.len() {
            return self.bottom();
        }
        self.set_start(idx, offset);
    }

    #[tracing::instrument(skip(self))]
    fn add_offset_up(
        &mut self,
        mut delta: u16,
    ) {
        self.mode = Mode::Scrolling;

        // NOTE "<=" or "<" doesn't matter because of delta > 0 check
        if delta <= self.start.offset {
            return self.set_start(self.start.idx, self.start.offset - delta);
        }
        delta -= self.start.offset;

        let mut idx = self.start.idx;
        let mut offset = 0;

        while delta > 0 && idx > 0 {
            idx -= 1;
            let height = self.height(idx);
            if delta <= height {
                offset = height - delta;
                break;
            }
            delta -= height;
        }
        self.set_start(idx, offset);
    }
}
