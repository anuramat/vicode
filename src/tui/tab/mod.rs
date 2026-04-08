pub mod input;
pub mod scroll;
pub mod update;

use std::fmt::Debug;

use anyhow::Result;
use derive_more::Deref;
use derive_more::DerefMut;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::symbols::line::HORIZONTAL;
use ratatui::symbols::line::THICK_HORIZONTAL;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use tokio::sync::mpsc::Sender;

use crate::agent::AgentHandle;
use crate::agent::id::AgentId;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::tui::app::AppEvent;
use crate::tui::colors::INPUT_ACTIVE_COLOR;
use crate::tui::colors::INPUT_INACTIVE_COLOR;
use crate::tui::osc7::set_osc7;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::container::scroll::ScrollElements;
use crate::tui::widgets::info::InfoWidget;
use crate::tui::widgets::input::CompletionSource;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::input::InputOpts;

const INPUT_AREA_HEIGHT: u16 = 5;
const INFO_PANE_WIDTH: u16 = 32;
const FILE_COMPLETION_MAX_HEIGHT: u16 = 5;

#[derive(Debug)]
pub struct Tab<'a> {
    pub tx: Sender<AppEvent>, // TODO we can ALMOST drop this
    pub aid: AgentId,
    pub agent: AgentHandle,

    pub scroll: ScrollElements,
    pub input: MessageInput<'a>,
    pub info: InfoWidget,

    pub multiplier: usize,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum TabEntry<'a> {
    Loading,
    Ready(Tab<'a>),
}

impl TabEntry<'_> {
    pub fn label(
        &self,
        aid: &AgentId,
    ) -> String {
        let prefix = match self {
            Self::Loading => "*",
            Self::Ready(tab) => tab.agent.state.status.label(),
        };
        format!("[{prefix}]{aid}")
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

impl Tab<'_> {
    pub fn new(
        tx: Sender<AppEvent>,
        aid: AgentId,
        agent: AgentHandle,
        project: &Project,
    ) -> Result<Self> {
        let mut tab = Self {
            tx,
            aid,
            agent,
            scroll: ScrollElements::default(),
            input: MessageInput {
                title: Default::default(),
                input: Input::new(InputOpts {
                    source: CompletionSource::Freeform(vec![('@', Vec::new())]),
                    height: FILE_COMPLETION_MAX_HEIGHT,
                    clear_on_unfocus: false,
                }),
            },
            info: Default::default(),
            multiplier: 1,
        };
        tab.refresh_file_completion(project)?;
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

        let body = Rect {
            x: body.x + 1,
            height: body.height.saturating_sub(1),
            width: body.width.saturating_sub(2),
            ..body
        };

        let input_height = if self.input.visible() {
            INPUT_AREA_HEIGHT + 1
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
        self.input.render(input_area, buf);
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

#[derive(Debug, Clone, Deref, DerefMut)]
pub struct MessageInput<'a> {
    #[deref]
    #[deref_mut]
    pub input: Input<'a>,
    // border between input and messages
    pub title: String,
}

impl MessageInput<'_> {
    pub fn visible(&self) -> bool {
        self.focused() || !self.empty()
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if !self.visible() {
            return;
        }
        let (symbol, color) = if self.focused() {
            (THICK_HORIZONTAL, INPUT_ACTIVE_COLOR)
        } else {
            (HORIZONTAL, INPUT_INACTIVE_COLOR)
        };
        buf.set_string(
            area.x,
            area.y,
            symbol.repeat(area.width.into()),
            Style::default().fg(color),
        );
        buf.set_string(area.x + 1, area.y, &self.title, Style::default());
        self.input.render(
            Rect {
                y: area.y + 1,
                height: area.height.saturating_sub(1),
                ..area
            },
            buf,
        );
    }
}
