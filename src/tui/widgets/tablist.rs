use derive_more::From;
use indexmap::IndexMap;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::widgets::*;

use crate::agent::AgentId;
use crate::tui::tab::Tab;

lazy_static::lazy_static! {
    static ref SELECTION_STYLE:
        Style = Style::default().add_modifier(Modifier::REVERSED);
}

#[derive(Clone, Default)]
pub struct TabList<'a> {
    pub widget: TabListWidget<'a>,
    pub state: TabListState,
}

#[derive(Clone, Default, From)]
pub struct TabListWidget<'a>(List<'a>);

#[derive(Clone, Default, From)]
pub struct TabListState(ListState);

impl<'a> TabList<'a> {
    pub fn rebuild(
        &mut self,
        tabs: &IndexMap<AgentId, Tab<'a>>,
    ) {
        let items = tabs
            .iter()
            .map(|i| ListItem::new(i.0.0.to_string()))
            .collect::<Vec<_>>();
        self.widget = List::new(items).highlight_style(*SELECTION_STYLE).into();
    }

    pub fn selected(&self) -> Option<usize> {
        self.state.0.selected()
    }

    pub fn select(
        &mut self,
        idx: Option<usize>,
    ) {
        self.state.0.select(idx);
    }
}

impl StatefulWidget for &TabListWidget<'_> {
    type State = TabListState;

    fn render(
        self,
        area: ratatui::prelude::Rect,
        buf: &mut ratatui::prelude::Buffer,
        state: &mut Self::State,
    ) {
        StatefulWidget::render(&self.0, area, buf, &mut state.0);
    }
}
