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
pub struct ScrollElements<T, U>
where
    T: AsRef<[U]>,
    U: IntoElement,
{
    pub data: T,
    ctx: RenderContext,
    dirty: Vec<bool>,
    elements: Vec<Element>,
    start: StartLocation,
    mode: Mode,
    width: u16,
    height: u16,
    phantom: PhantomData<U>,
}

impl<T, U> ScrollElements<T, U>
where
    T: AsRef<[U]>,
    U: IntoElement,
{
    pub fn new(data: T) -> Self {
        Self {
            ctx: Default::default(),
            data,
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

    pub fn element(
        &mut self,
        idx: usize,
    ) -> &mut Element {
        self.elements.resize_with(self.len(), Default::default);
        if idx >= self.dirty.len() || self.dirty[idx] {
            self.dirty.resize(self.len(), true);
            self.elements[idx] = self.data.as_ref()[idx].to_element();
            self.dirty[idx] = false;
        }
        &mut self.elements[idx]
    }

    pub fn len(&self) -> usize {
        self.data.as_ref().len()
    }

    pub fn height(
        &mut self,
        idx: usize,
    ) -> u16 {
        let width = self.width;
        let ctx = self.ctx;
        self.element(idx).height(width, ctx)
    }
}
