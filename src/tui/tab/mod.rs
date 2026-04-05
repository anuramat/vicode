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
use crate::agent::id::AgentId;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::tui::app::handle::AppEvent;
use crate::tui::osc7::set_osc7;
use crate::tui::textarea::CompletionSource;
use crate::tui::textarea::Input;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::container::scroll::ScrollElements;
use crate::tui::widgets::info::InfoWidget;

const INPUT_AREA_HEIGHT: u16 = 5;
const INFO_PANE_WIDTH: u16 = 32;
const FILE_COMPLETION_MAX_HEIGHT: u16 = 5;
pub const FILE_COMPLETION_SOURCE: &str = "files";

#[derive(Debug)]
pub struct Tab<'a> {
    pub tx: Sender<AppEvent>, // TODO we can ALMOST drop this
    pub aid: AgentId,
    pub agent: AgentHandle,

    pub scroll: ScrollElements,
    pub insert_mode: bool, // TODO use enum
    pub user_input: UserInput<'a>,
    pub info: InfoWidget,

    pub multiplier: usize,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum TabEntry<'a> {
    Loading,
    Ready(Tab<'a>),
}

impl<'a> TabEntry<'a> {
    pub fn label(
        &self,
        aid: &AgentId,
    ) -> String {
        let prefix = match self {
            Self::Loading => "*",
            Self::Ready(tab) => tab.agent.state.status.label(),
        };
        format!("[{}]{}", prefix, aid)
    }

    pub fn set_osc7(
        &self,
        project: &Project,
    ) {
        match self {
            Self::Loading => set_osc7(project.root()),
            Self::Ready(tab) => tab.set_osc7(project),
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
        aid: AgentId,
        agent: AgentHandle,
        project: &Project,
    ) -> Result<Self> {
        let mut tab = Self {
            tx,
            aid,
            agent,
            scroll: Default::default(),
            insert_mode: false,
            user_input: Default::default(),
            info: Default::default(),
            multiplier: 1,
        };
        tab.refresh_file_completion(project).await;
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
            &self.agent.state.context.history,
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
        format!("[{}]{}", self.agent.state.status.label(), self.aid)
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
        Self(Input::new(
            "",
            vec![CompletionSource::prefixed_word(
                FILE_COMPLETION_SOURCE,
                '@',
                vec![],
            )],
            FILE_COMPLETION_MAX_HEIGHT,
        ))
    }
}
