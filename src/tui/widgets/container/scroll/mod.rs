pub mod r#move;
pub mod render;

use std::marker::PhantomData;

use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::container::element::IntoElement;
use crate::tui::widgets::container::element::RenderContext;

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
    /// During render we update the StartLocation s.t. it looks the same if we switch to Scrolling
    #[default]
    Tail,
}

#[derive(Clone, Debug)]
pub struct ScrollElements<U>
where U: IntoElement
{
    ctx: RenderContext,
    dirty: Vec<bool>,
    elements: Vec<Element>,
    start: StartLocation,
    mode: Mode,
    width: u16,
    height: u16,
    phantom: PhantomData<U>,
}

impl<U> ScrollElements<U>
where U: IntoElement
{
    pub fn new() -> Self {
        Self {
            ctx: Default::default(),
            dirty: Vec::new(),
            elements: Vec::new(),
            start: Default::default(),
            mode: Default::default(),
            width: 0,
            height: 0,
            phantom: Default::default(),
        }
    }

    pub fn set_dirty(
        &mut self,
        idx: usize,
    ) {
        if idx < self.dirty.len() {
            self.dirty[idx] = true;
        }
    }

    pub fn set_len(
        &mut self,
        len: usize,
    ) {
        if len < self.elements.len() {
            self.dirty = Vec::new();
        }
        self.dirty.resize(len, true);
        self.elements.resize_with(len, Default::default);
        if len == 0 || self.start.idx >= len {
            self.start = Default::default();
            self.mode = Mode::Tail;
        }
    }

    pub fn set_start(
        &mut self,
        data: &[U],
        idx: usize,
        offset: u16,
    ) {
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

    pub fn element(
        &mut self,
        data: &[U],
        idx: usize,
    ) -> &mut Element {
        if self.dirty[idx] {
            self.elements[idx] = data[idx].to_element();
            self.dirty[idx] = false;
        }
        &mut self.elements[idx]
    }

    pub fn height(
        &mut self,
        data: &[U],
        idx: usize,
    ) -> u16 {
        if data.is_empty() {
            return 0;
        }
        let width = self.width;
        let ctx = self.ctx;
        self.element(data, idx).height(width, ctx)
    }
}
