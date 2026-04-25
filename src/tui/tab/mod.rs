pub mod input;
pub mod scroll;
pub mod update;

use std::fmt::Debug;

use anyhow::Result;
use tokio::sync::mpsc::Sender;

use crate::agent::AgentHandle;
use crate::agent::id::AgentId;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::tui::app::AppEvent;
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
            Self::Ready(tab) => tab.set_osc7(project),
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
        tab.refresh_file_completion(project)?;
        Ok(tab)
    }

    pub async fn refresh_info(
        &mut self,
        project: &Project,
    ) -> Result<()> {
        self.info = InfoWidget::new(project, &self.aid).await?;
        Ok(())
    }

    pub fn label(&self) -> String {
        format!("[{}]{}", self.agent.state.status.label(), self.aid)
    }
}
