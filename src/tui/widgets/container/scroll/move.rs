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

        self.start = StartLocation {
            idx,
            ..Default::default()
        };
    }

    // put the current message at the top if it's not; previous otherwise
    pub fn prev_element(&mut self) {
        self.mode = Mode::Scrolling;

        if self.start.offset != 0 {
            self.start = StartLocation {
                idx: self.start.idx,
                ..Default::default()
            };
        }

        let mut idx = self.start.idx.saturating_sub(1);
        while idx > 0 && self.height(idx) == 0 {
            idx -= 1;
        }
        self.start = StartLocation {
            idx,
            ..Default::default()
        };
    }

    pub fn top(&mut self) {
        self.start = StartLocation::default();
        self.mode = Mode::Scrolling;
    }

    pub fn bottom(&mut self) {
        self.start = Default::default();
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

    // TODO check and refactor below

    #[tracing::instrument(skip(self))]
    fn add_offset_down(
        &mut self,
        mut delta: u16,
    ) {
        self.mode = Mode::Scrolling;
        self.start.relative_offset = None;

        let visible = self.height(self.start.idx) - self.start.offset;
        // NOTE "<" specifically, because if delta == element, we need to skip to the next element
        if delta < visible {
            self.start.offset += delta;
            return;
        }
        delta -= visible;
        self.start.offset = 0;

        let mut idx = self.start.idx + 1;
        while delta > 0 && idx < self.elements.len() {
            let height = self.height(idx);
            if delta < height {
                self.start.offset = delta;
                break;
            }
            delta -= height;
            idx += 1;
        }
        if idx >= self.len() {
            return self.bottom();
        }
        self.start.idx = idx;
    }

    #[tracing::instrument(skip(self))]
    fn add_offset_up(
        &mut self,
        mut delta: u16,
    ) {
        self.mode = Mode::Scrolling;
        self.start.relative_offset = None;

        // NOTE "<=" or "<" doesn't matter because of delta > 0 check
        if delta <= self.start.offset {
            self.start.offset -= delta;
            return;
        }
        delta -= self.start.offset;
        self.start.offset = 0;

        while delta > 0 && self.start.idx > 0 {
            self.start.idx -= 1;
            let height = self.height(self.start.idx);
            if delta <= height {
                self.start.offset = height - delta;
                break;
            }
            delta -= height;
        }
    }
}
