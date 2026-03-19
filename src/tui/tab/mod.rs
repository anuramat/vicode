pub mod input;
pub mod keys;
pub mod update;

use std::fmt::Debug;

use anyhow::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;
use tokio::sync::mpsc::Sender;
use tui_textarea::TextArea;

use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::llm::message::HistoryEntry;
use crate::llm::tokens::count_text_tokens;
use crate::tui::app::handle::AppEvent;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::container::scroll::ScrollElements;
use crate::tui::widgets::info::InfoWidget;

const INPUT_AREA_HEIGHT: u16 = 5;
const INFO_PANE_WIDTH: u16 = 32;

#[derive(Debug, Clone)]
pub struct Tab<'a> {
    pub tx: Sender<AppEvent>,
    pub aid: AgentId,
    pub agent_state: AgentState,
    pub instructions_tokens: usize,
    pub context_tokens: usize,

    pub scroll: ScrollElements<HistoryEntry>,
    pub insert_mode: bool, // TODO use enum
    pub user_input: UserInput<'a>,
    pub info: InfoWidget,

    pub multiplier: usize,
    pub state: TabState,
}

#[derive(Debug, Clone)]
pub enum TabState {
    Loading,
    InProgress,
    Idle,
}

impl<'a> Tab<'a> {
    pub async fn new(
        tx: Sender<AppEvent>,
        aid: AgentId,
        agent_state: AgentState,
    ) -> Result<Self> {
        let tab = Self {
            instructions_tokens: count_text_tokens(&agent_state.context.instructions),
            context_tokens: agent_state.context.history.total_tokens(),
            tx,
            aid,
            agent_state,
            scroll: ScrollElements::<HistoryEntry>::new(),
            insert_mode: false,
            user_input: Default::default(),
            info: Default::default(),
            multiplier: 1,
            state: TabState::Idle,
        };
        Ok(tab)
    }

    pub fn loading_tab(
        tx: Sender<AppEvent>,
        aid: AgentId,
        agent_state: AgentState,
    ) -> Self {
        Self {
            instructions_tokens: 0,
            context_tokens: 0,
            tx,
            aid,
            agent_state,
            scroll: ScrollElements::<HistoryEntry>::new(),
            insert_mode: false,
            user_input: Default::default(),
            info: Default::default(),
            multiplier: 1,
            state: TabState::Loading,
        }
    }

    #[tracing::instrument(skip(self, buf))]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let outer = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Min(0),
                Constraint::Length(INFO_PANE_WIDTH),
            ])
            .split(area);
        self.info.render(outer[1], buf);

        let block = Block::new()
            .borders(Borders::LEFT | Borders::RIGHT)
            .border_type(BorderType::Plain);
        block.render_ref(outer[0], buf);
        let area = block.inner(outer[0]);

        if matches!(self.state, TabState::Loading) {
            self.render_loading(area, buf);
            return;
        }

        let input_height = if self.user_input.visible(self.insert_mode) {
            INPUT_AREA_HEIGHT
        } else {
            0
        };
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Min(0), Constraint::Length(input_height)])
            .split(area);

        self.scroll.render(
            self.agent_state.context.history.as_ref(),
            layout[0].inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 0,
            }),
            buf,
            ctx,
        );
        self.user_input.0.render(layout[1], buf);
    }

    fn render_loading(
        &self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let style = Style::default().add_modifier(Modifier::REVERSED);
        buf.set_style(area, style);

        let text = "LOADING".to_string();
        let area = Rect {
            x: area.x + ((area.width - text.len() as u16) / 2),
            y: area.y + area.height / 2,
            width: text.len() as u16,
            height: 1,
        };
        let widget = Paragraph::new(text);

        widget.render(area, buf);
    }
}

#[derive(Debug, Clone)]
pub struct UserInput<'a>(pub TextArea<'a>);

impl<'a> UserInput<'a> {
    pub fn empty(&self) -> bool {
        let lines = self.0.lines();
        lines.len() == 1 && lines[0].is_empty()
    }

    pub fn visible(
        &self,
        insert_mode: bool,
    ) -> bool {
        insert_mode || !self.empty()
    }
}

impl Default for UserInput<'_> {
    fn default() -> Self {
        let mut text_area = TextArea::default();
        text_area.set_cursor_line_style(Default::default());
        Self(text_area)
    }
}
