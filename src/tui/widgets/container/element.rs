use std::fmt::Debug;

use ratatui::prelude::*;
use ratatui::widgets::Block;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use smart_default::SmartDefault;

use crate::tui::widgets::container::empty::EmptyElement;

// TODO rename the trait

pub trait HeightComputable: Debug {
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

#[derive(Debug)]
pub struct Element {
    widget: Box<dyn HeightComputable>,
    width: u16,
    height: u16,
    ctx: RenderContext,
}

#[allow(clippy::struct_excessive_bools)] // fine for a config
#[derive(Debug, Clone, PartialEq, Eq, Copy, Serialize, Deserialize, SmartDefault, JsonSchema)]
#[serde(default)]
pub struct RenderContext {
    #[default(true)]
    pub hide_reasoning: bool,
    #[default(true)]
    pub hide_tools: bool,
    #[default(true)]
    pub hide_developer: bool,
    #[default(true)]
    pub render_markdown: bool,
}

pub trait IntoElement {
    fn to_element(&self) -> Element;
}

impl Element {
    pub fn new<T>(widget: T) -> Self
    where T: HeightComputable + 'static {
        Self {
            widget: Box::new(widget),
            width: 0,
            height: 0,
            ctx: RenderContext::default(),
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
        let drop_cells: u32 = (offset * area.width).into();
        let take_cells = area.area();
        let width: u32 = area.width.into();
        for i in 0..take_cells {
            let src_idx: u32 = i + drop_cells;
            let x: u32 = i % width;
            let y: u32 = i / width;
            let dest_x: u32 = x + u32::from(area.x);
            let dest_y: u32 = y + u32::from(area.y);

            let dest_x: u16 = dest_x.try_into().expect("area width overflow");
            let dest_y: u16 = dest_y.try_into().expect("area height overflow");
            buf[(dest_x, dest_y)] = tmp_buf.content[src_idx as usize].clone();
        }
    }
}

impl<T> From<T> for Element
where T: HeightComputable + 'static
{
    fn from(p: T) -> Self {
        Self::new(p)
    }
}

impl Default for Element {
    fn default() -> Self {
        Self::from(EmptyElement)
    }
}

impl<T> IntoElement for T
where for<'a> &'a T: Into<Element>
{
    fn to_element(&self) -> Element {
        self.into()
    }
}
