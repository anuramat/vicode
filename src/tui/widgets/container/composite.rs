use std::fmt::Debug;

use derive_more::From;
use derive_more::Into;
use ratatui::prelude::*;

use crate::tui::widgets::container::element::*;

#[derive(Debug, Clone, Default, From, Into)]
pub struct CompositeElement(pub Vec<Element>);

// TODO cache the layout

impl HeightComputable for CompositeElement {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        self.0
            .iter_mut()
            .fold(0, |acc, x| acc + x.height(width, ctx))
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let constraints: Vec<Constraint> = self
            .0
            .iter_mut()
            .map(|element| {
                let height = element.height(area.width, ctx);
                Constraint::Length(height)
            })
            .collect();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);
        for (i, element) in self.0.iter_mut().enumerate() {
            element.render(layout[i], buf, ctx);
        }
    }
}
