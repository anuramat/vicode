pub mod r#move;
pub mod render;

use std::iter;
use std::ops::Range;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::IntoElement;
use crate::tui::widgets::container::element::RenderContext;

serde_plain::derive_display_from_serialize!(ScrollOp);
serde_plain::derive_fromstr_from_deserialize!(ScrollOp);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollOp {
    Bottom,
    Down,
    HalfPageDown,
    HalfPageUp,
    LineDown,
    LineUp,
    NextElement,
    PageDown,
    PageUp,
    PrevElement,
    Top,
    Up,
}

/// Element at the top of the view;
/// Invalid if the entire history fits on the screen.
#[derive(Default, Clone, Debug)]
struct StartLocation {
    idx: usize,
    /// Last height of the element
    height: u16,
    /// How many lines to skip, or equivalently, the index of the first line rendered;
    /// Invariant: (offset < height) || (height == offset == 0)
    offset: u16,
    /// Offset as a percentage of element height to maintain when resizing;
    /// None until we start resizing, reset to None when scrolling
    relative_offset: Option<f32>,
}

#[derive(Default, Clone, Debug)]
enum Mode {
    /// Specific line is at the top.
    Scrolling,
    /// Last line of the last element is at the bottom of the view.
    /// If history fits on screen, we're always forcing Tail.
    /// During render we update the `StartLocation` s.t. it looks the same if we switch to Scrolling
    #[default]
    Tail,
}

#[derive(Debug, Default)]
pub struct ScrollElements {
    ctx: RenderContext,
    dirty: Vec<bool>,
    elements: Vec<Element>,
    start: StartLocation,
    mode: Mode,
    width: u16,
    height: u16,
}

impl ScrollElements {
    pub fn scroll<U>(
        &mut self,
        data: &[U],
        op: ScrollOp,
    ) where
        U: IntoElement,
    {
        match op {
            ScrollOp::Bottom => self.bottom(),
            ScrollOp::Down => self.half_page_down(data),
            ScrollOp::HalfPageDown => self.half_page_down(data),
            ScrollOp::HalfPageUp => self.half_page_up(data),
            ScrollOp::LineDown => self.line_down(data),
            ScrollOp::LineUp => self.line_up(data),
            ScrollOp::NextElement => self.next_element(data),
            ScrollOp::PageDown => self.page_down(data),
            ScrollOp::PageUp => self.page_up(data),
            ScrollOp::PrevElement => self.prev_element(data),
            ScrollOp::Top => self.top(data),
            ScrollOp::Up => self.half_page_up(data),
        }
    }

    /// mirror a change to the data: the elements in `range` became `len` new ones
    pub fn splice(
        &mut self,
        range: Range<usize>,
        len: usize,
    ) {
        let (start, end) = (range.start, range.end);
        self.elements
            .splice(range.clone(), iter::repeat_with(Element::default).take(len));
        self.dirty.splice(range, iter::repeat_n(true, len));
        // keep the view on the same element, if it survived
        if self.start.idx >= end {
            self.start.idx = self.start.idx + len - (end - start);
        } else if self.start.idx >= start + len {
            self.start = StartLocation::default();
            self.mode = Mode::Tail;
        }
    }

    /// number of cached elements
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    /// empty cache for `len` elements
    pub fn new(len: usize) -> Self {
        Self {
            dirty: vec![true; len],
            elements: iter::repeat_with(Element::default).take(len).collect(),
            ..Default::default()
        }
    }

    pub fn set_start<U>(
        &mut self,
        data: &[U],
        idx: usize,
        offset: u16,
    ) where
        U: IntoElement,
    {
        if data.is_empty() {
            return Default::default();
        }
        self.start = StartLocation {
            idx,
            height: self.height(data, idx),
            offset,
            relative_offset: None,
        }
    }

    pub fn element<U>(
        &mut self,
        data: &[U],
        idx: usize,
    ) -> &mut Element
    where
        U: IntoElement,
    {
        if self.dirty[idx] {
            self.elements[idx] = data[idx].to_element();
            self.dirty[idx] = false;
        }
        &mut self.elements[idx]
    }

    pub fn height<U>(
        &mut self,
        data: &[U],
        idx: usize,
    ) -> u16
    where
        U: IntoElement,
    {
        if data.is_empty() {
            return 0;
        }
        let width = self.width;
        let ctx = self.ctx;
        self.element(data, idx).height(width, ctx)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use similar_asserts::assert_eq;

    use super::*;
    use crate::tui::widgets::container::element::HeightComputable;

    #[derive(Clone, Debug)]
    struct TestElement(u16);

    impl HeightComputable for TestElement {
        fn height(
            &mut self,
            _width: u16,
            _ctx: RenderContext,
        ) -> u16 {
            self.0
        }

        fn render(
            &mut self,
            _area: Rect,
            _buf: &mut Buffer,
            _ctx: RenderContext,
        ) {
        }
    }

    impl From<&TestElement> for Element {
        fn from(value: &TestElement) -> Self {
            Self::new(value.clone())
        }
    }

    /// (start idx, tail mode, dirty) after splicing, scrolled to element 3 of 5
    fn spliced(
        range: Range<usize>,
        len: usize,
    ) -> (usize, bool, Vec<bool>) {
        let mut scroll = ScrollElements::new(5);
        scroll.dirty = vec![false; 5];
        scroll.start.idx = 3;
        scroll.mode = Mode::Scrolling;
        scroll.splice(range, len);
        assert_eq!(scroll.elements.len(), scroll.dirty.len());
        (
            scroll.start.idx,
            matches!(scroll.mode, Mode::Tail),
            scroll.dirty,
        )
    }

    #[test]
    fn splice_after_start_keeps_the_view() {
        assert_eq!(
            spliced(5..5, 1),
            (3, false, vec![false, false, false, false, false, true])
        );
        assert_eq!(
            spliced(4..5, 1),
            (3, false, vec![false, false, false, false, true])
        );
        assert_eq!(spliced(4..5, 0), (3, false, vec![false; 4]));
    }

    #[test]
    fn splice_before_start_shifts_it() {
        // compaction: the first three become a summary
        assert_eq!(spliced(0..3, 1), (1, false, vec![true, false, false]));
        assert_eq!(
            spliced(1..2, 3),
            (5, false, vec![false, true, true, true, false, false, false])
        );
    }

    #[test]
    fn splice_in_place_keeps_the_view() {
        assert_eq!(
            spliced(3..5, 2),
            (3, false, vec![false, false, false, true, true])
        );
    }

    #[test]
    fn splice_dropping_start_tails() {
        assert_eq!(spliced(0..4, 1), (0, true, vec![true, false]));
        assert_eq!(spliced(3..5, 0), (0, true, vec![false; 3]));
    }

    #[test]
    fn scroll_ops_display_in_config_format() {
        assert_eq!(ScrollOp::HalfPageDown.to_string(), "half_page_down");
        assert_eq!(ScrollOp::Down.to_string(), "down");
    }
}
