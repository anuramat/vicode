pub mod input;
pub mod scroll;
pub mod update;

use std::fmt::Debug;

use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::agent::AgentHandle;
use crate::agent::id::AgentId;
use crate::forward;
use crate::llm::history::History;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::tui::app::AppEvent;
use crate::tui::command::parse_arg;
use crate::tui::osc7::set_osc7;
use crate::tui::widgets::container::scroll::ScrollElements;
use crate::tui::widgets::info::InfoWidget;
use crate::tui::widgets::input::CompletionSource;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::input::InputOpts;
use crate::tui::widgets::tab::input::MessageInput;

const FILE_COMPLETION_MAX_HEIGHT: u16 = 5;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Body,
    Info,
}

#[derive(Debug)]
pub struct Tab<'a> {
    pub tx: Sender<AppEvent>, // TODO we can ALMOST drop this
    pub aid: AgentId,
    pub agent: AgentHandle,
    pub project: Project,

    pub scroll: ScrollElements,
    pub input: MessageInput<'a>,
    pub info: InfoWidget,
    pub focus: Focus,

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
            Self::Ready(tab) => tab.set_osc7(),
        }
    }
}

impl Tab<'_> {
    forward! {
        history: History = self.agent.state.context.history;
    }

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
            project: project.clone(),
            scroll: ScrollElements::default(),
            input: MessageInput {
                title: String::new(),
                input: Input::new(InputOpts {
                    source: CompletionSource::Freeform(vec![('@', Vec::new())]),
                    height: FILE_COMPLETION_MAX_HEIGHT,
                    clear_on_unfocus: false,
                }),
            },
            info: InfoWidget::default(),
            focus: Focus::default(),
            multiplier: 1,
        };
        tab.refresh_file_completion()?;
        Ok(tab)
    }

    pub async fn refresh_info(&mut self) -> Result<()> {
        self.info = InfoWidget::new(&self.project, &self.aid).await?;
        Ok(())
    }

    pub const fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Body => Focus::Info,
            Focus::Info => Focus::Body,
        };
    }

    pub fn set_multiplier(
        &mut self,
        value: Option<&str>,
    ) -> Result<()> {
        let value: u8 = parse_arg(value)?.unwrap_or(1);
        anyhow::ensure!(value > 0, "multiplier must be positive");
        self.multiplier = value as usize;
        self.update_input_title();
        Ok(())
    }
}
