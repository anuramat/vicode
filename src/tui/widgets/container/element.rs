use std::fmt::Debug;

use dyn_clone::DynClone;
use dyn_clone::clone_trait_object;
use ratatui::prelude::*;
use ratatui::widgets::Block;

use crate::tui::widgets::container::empty::EmptyElement;

// TODO rename the trait

clone_trait_object!(HeightComputable);
pub trait HeightComputable: Debug + DynClone {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16;

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    );

    fn block(
        &self,
        _ctx: RenderContext,
    ) -> Option<Block<'_>> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct Element {
    widget: Box<dyn HeightComputable>,
    width: u16,
    height: u16,
    ctx: RenderContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct RenderContext {
    pub hide_reasoning: bool,
    pub hide_tools: bool,
    pub hide_developer: bool,
}

impl Default for RenderContext {
    fn default() -> Self {
        Self {
            hide_reasoning: true,
            hide_tools: true,
            hide_developer: true,
        }
    }
}

pub trait IntoElement {
    fn to_element(&self) -> Element;
}

impl Element {
    pub fn new<T>(widget: T) -> Self
    where T: HeightComputable + 'static {
        Element {
            widget: Box::new(widget),
            width: 0,
            height: 0,
            ctx: Default::default(),
        }
    }

    pub fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        if self.width != width || self.ctx != ctx {
            self.width = width;
            self.ctx = ctx;
            self.height = self.compute_height(width, ctx);
        }
        self.height
    }

    fn compute_height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        if width == 0 {
            return 0;
        }
        let block = match self.widget.block(ctx) {
            Some(block) => block,
            _ => {
                return self.widget.height(width, ctx);
            }
        };
        let outer = Rect {
            x: 0,
            y: 0,
            width: u16::MAX / 2,
            height: u16::MAX / 2,
        };
        let inner = block.inner(outer);
        let v_thickness = outer.height - inner.height;
        let h_thickness = outer.width - inner.width;
        if width < h_thickness {
            v_thickness
        } else {
            self.widget.height(width - h_thickness, ctx) + v_thickness
        }
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        match &self.widget.block(ctx) {
            Some(block) => {
                block.render(area, buf);
                self.widget.render(block.inner(area), buf, ctx);
            }
            None => {
                self.widget.render(area, buf, ctx);
            }
        }
    }

    pub fn partial_render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        offset: u16, // n of lines to skip at the top
        ctx: RenderContext,
    ) {
        // render the entire widget
        let height = self.height(area.width, ctx);
        let mut tmp_buf = ratatui::buffer::Buffer::empty(Rect {
            x: 0,
            y: 0,
            width: area.width,
            height,
        });
        self.render(tmp_buf.area, &mut tmp_buf, ctx);
        // then copy the visible part to the destination buffer
        let drop_cells = offset * area.width;
        let take_cells = area.area();
        let width = area.width as u32;
        for i in 0..take_cells {
            let src_idx = drop_cells + (i as u16);
            let x = (i % width) as u16;
            let y = (i / width) as u16;
            let dest_x: u16 = area.x + x;
            let dest_y: u16 = area.y + y;
            buf[(dest_x, dest_y)] = tmp_buf.content[src_idx as usize].clone();
        }
    }
}

impl<T> From<T> for Element
where T: HeightComputable + 'static
{
    default fn from(p: T) -> Self {
        Element::new(p)
    }
}

impl Default for Element {
    fn default() -> Self {
        Element::from(EmptyElement)
    }
}

impl<T> IntoElement for T
where for<'a> &'a T: Into<Element>
{
    fn to_element(&self) -> Element {
        self.into()
    }
}
