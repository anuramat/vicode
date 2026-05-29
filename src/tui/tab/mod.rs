pub mod input;
pub mod update;

use std::fmt::Debug;

use anyhow::Result;

use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::agent::router::AgentRouterHandle;
use crate::forward;
use crate::llm::history::History;
use crate::project::Project;
use crate::tui::command::parse_arg;
use crate::tui::widgets::container::scroll::ScrollElements;
use crate::tui::widgets::container::scroll::ScrollOp;
use crate::tui::widgets::info::InfoWidget;
use crate::tui::widgets::input::CompletionSource;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::input::InputOpts;
use crate::tui::widgets::tab::input::MessageInput;

const FILE_COMPLETION_MAX_HEIGHT: u16 = 5;

#[derive(Debug)]
pub struct Tab<'a> {
    pub router: Option<AgentRouterHandle>,
    pub aid: AgentId,
    pub state: AgentState,
    pub project: Project,

    pub scroll: ScrollElements,
    pub input: MessageInput<'a>,
    pub info: InfoWidget,

    pub multiplier: usize,
}

impl Tab<'_> {
    forward! {
        history: History = self.state.context.history;
    }

    pub fn new(
        router: Option<AgentRouterHandle>,
        aid: AgentId,
        state: AgentState,
        project: &Project,
    ) -> Self {
        Self {
            router,
            aid,
            state,
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
            multiplier: 1,
        }
    }

    pub fn label(&self) -> String {
        let prefix = if self.router.is_none() {
            "*"
        } else {
            self.state.status.label()
        };
        format!("[{prefix}]{}", self.aid)
    }

    pub async fn refresh_info(&mut self) -> Result<()> {
        if self.router.is_none() {
            return Ok(());
        }
        self.info = InfoWidget::new(&self.project, &self.aid).await?;
        Ok(())
    }

    pub fn set_multiplier(
        &mut self,
        value: Option<&str>,
    ) -> Result<()> {
        if self.router.is_none() {
            return Ok(());
        }
        let value: u8 = parse_arg(value)?.unwrap_or(1);
        anyhow::ensure!(value > 0, "multiplier must be positive");
        self.multiplier = value as usize;
        self.update_input_title();
        Ok(())
    }

    pub fn scroll(
        &mut self,
        op: ScrollOp,
    ) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.scroll(messages, op);
    }

    pub fn router(&self) -> Result<&AgentRouterHandle> {
        self.router
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("agent isn't attached (yet?)"))
    }
}
