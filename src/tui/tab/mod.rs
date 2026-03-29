pub mod input;
pub mod scroll;
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

use crate::agent::AgentHandle;
use crate::agent::AgentState;
use crate::agent::handle::AgentStarted;
use crate::agent::id::AgentId;
use crate::config::AssistantConfig;
use crate::config::CONFIG;
use crate::llm::message::AssistantMessageStatus;
use crate::llm::message::HistoryEntry;
use crate::llm::message::Message;
use crate::llm::tokens::count_text_tokens;
use crate::project::PROJECT;
use crate::project::layout::LayoutTrait;
use crate::tui::app::handle::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::textarea::Input;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::container::scroll::ScrollElements;
use crate::tui::widgets::info::InfoWidget;

const INPUT_AREA_HEIGHT: u16 = 5;
const INFO_PANE_WIDTH: u16 = 32;

#[derive(Debug)]
pub struct Tab<'a> {
    pub tx: Sender<AppEvent>, // TODO we can ALMOST drop this
    pub agent: AgentHandle,
    pub aid: AgentId,
    pub agent_state: AgentState,
    pub instructions_tokens: usize,
    pub context_tokens: usize,

    /// cache for presentation purposes
    pub assistant_config: AssistantConfig,

    pub scroll: ScrollElements,
    pub insert_mode: bool, // TODO use enum
    pub user_input: UserInput<'a>,
    pub info: InfoWidget,

    pub multiplier: usize,
    // TODO rename to status or smth
    pub state: TabState,
}

#[derive(Debug)]
pub enum TabEntry<'a> {
    Loading,
    Ready(Tab<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabState {
    Running(AssistantState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantState {
    Generating,
    Idle,
    AbortedByUser,
    Error, // TODO Error(String)
}

impl AssistantState {
    pub fn from_history(history: &[HistoryEntry]) -> Self {
        match history.last() {
            Some(HistoryEntry {
                message: Message::Assistant(msg),
                ..
            }) => (&msg.finish_reason).into(),
            _ => Self::Idle,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Generating => "+",
            Self::Idle => ".",
            Self::AbortedByUser => "?",
            Self::Error => "!",
        }
    }
}

impl From<&AssistantMessageStatus> for AssistantState {
    fn from(value: &AssistantMessageStatus) -> Self {
        match value {
            AssistantMessageStatus::InProgress => Self::Generating,
            AssistantMessageStatus::Success => Self::Idle,
            AssistantMessageStatus::AbortedByUser => Self::AbortedByUser,
            AssistantMessageStatus::Error(_) => Self::Error,
        }
    }
}

impl TabState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running(state) => state.label(),
        }
    }

    pub fn idle(&self) -> bool {
        matches!(self, Self::Running(AssistantState::Idle))
    }
}

impl<'a> TabEntry<'a> {
    pub fn label(
        &self,
        aid: &AgentId,
    ) -> String {
        let prefix = match self {
            Self::Loading => "*",
            Self::Ready(tab) => tab.state.label(),
        };
        format!("[{}]{}", prefix, aid)
    }

    pub fn set_osc7(&self) {
        match self {
            Self::Loading => set_osc7(&PROJECT.root()),
            Self::Ready(tab) => tab.set_osc7(),
        }
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        match self {
            Self::Loading => render_loading(area, buf),
            Self::Ready(tab) => tab.render(area, buf, ctx),
        }
    }
}

impl<'a> Tab<'a> {
    pub async fn new(
        tx: Sender<AppEvent>,
        agent: AgentStarted,
    ) -> Result<Self> {
        let state = TabState::Running(AssistantState::from_history(
            agent.state.context.history.as_ref(),
        ));
        let tab = Self {
            tx,
            aid: agent.aid, // TODO I think we can drop this
            agent: agent.handle,
            assistant_config: CONFIG.assistants[&agent.state.context.assistant_id].clone(),
            instructions_tokens: count_text_tokens(&agent.state.context.instructions),
            context_tokens: agent.state.context.history.total_tokens(),
            agent_state: agent.state,
            scroll: Default::default(),
            insert_mode: false,
            user_input: Default::default(),
            info: Default::default(),
            multiplier: 1,
            state,
        };
        Ok(tab)
    }

    #[tracing::instrument(skip(self, buf))]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        let [body, info_area] = *Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Min(0),
                Constraint::Length(INFO_PANE_WIDTH),
            ])
            .split(area)
        else {
            unreachable!()
        };
        self.info.render(info_area, buf);

        let block = Block::new()
            .borders(Borders::LEFT | Borders::RIGHT)
            .border_type(BorderType::Plain);
        block.render_ref(body, buf);
        let body = block.inner(body);

        let input_height = if self.user_input.visible(self.insert_mode) {
            INPUT_AREA_HEIGHT
        } else {
            0
        };
        let [messages_area, input_area] = *Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Min(0), Constraint::Length(input_height)])
            .split(body)
        else {
            unreachable!()
        };

        self.scroll.render(
            self.agent_state.context.history.as_ref(),
            messages_area.inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 0,
            }),
            buf,
            ctx,
        );
        self.user_input.0.render(input_area, buf);
    }

    pub fn label(&self) -> String {
        format!("[{}]{}", self.state.label(), self.aid)
    }
}

fn render_loading(
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

#[derive(Debug, Clone)]
pub struct UserInput<'a>(pub Input<'a>);

impl<'a> UserInput<'a> {
    pub fn empty(&self) -> bool {
        let lines = self.0.textarea.lines();
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
        Self(Input::new("", vec![], 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::message::AssistantMessage;
    use crate::llm::message::DeveloperMessage;
    use crate::llm::message::MessageMeta;
    use crate::llm::message::UserMessage;

    fn entry(message: Message) -> HistoryEntry {
        HistoryEntry {
            meta: MessageMeta::default(),
            message,
        }
    }

    #[test]
    fn assistant_state_comes_from_last_message() {
        let history = vec![entry(Message::Assistant(AssistantMessage {
            finish_reason: AssistantMessageStatus::Error("oops".into()),
            content: Default::default(),
        }))];
        assert_eq!(
            AssistantState::from_history(&history),
            AssistantState::Error
        );
    }

    #[test]
    fn trailing_user_message_is_idle() {
        let history = vec![entry(Message::User(UserMessage { text: "hi".into() }))];
        assert_eq!(AssistantState::from_history(&history), AssistantState::Idle);
    }

    #[test]
    fn trailing_developer_message_is_idle() {
        let history = vec![entry(Message::Developer(DeveloperMessage {
            text: "note".into(),
        }))];
        assert_eq!(AssistantState::from_history(&history), AssistantState::Idle);
    }

    #[test]
    fn tab_state_suffixes_are_short_and_stable() {
        assert_eq!(
            TabState::Running(AssistantState::AbortedByUser).label(),
            "?"
        );
    }
}
