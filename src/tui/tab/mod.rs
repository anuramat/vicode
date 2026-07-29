pub mod input;
pub mod update;

use std::collections::HashMap;
use std::fmt::Debug;

use anyhow::Result;

use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::agent::router::AgentRouterHandle;
use crate::forward;
use crate::llm::history::History;
use crate::llm::provider::assistant::ModelConfig;
use crate::project::Project;
use crate::tui::widgets::container::scroll::ScrollElements;
use crate::tui::widgets::container::scroll::ScrollOp;
use crate::tui::widgets::info::InfoWidget;
use crate::tui::widgets::input::CompletionSource;
use crate::tui::widgets::input::Input;
use crate::tui::widgets::input::InputOpts;
use crate::tui::widgets::message::MessageView;
use crate::tui::widgets::tab::input::MessageInput;

const FILE_COMPLETION_MAX_HEIGHT: u16 = 5;

/// the scroll's render data: each message paired with the live tool-output
/// buffers (only pending calls consult them)
pub fn message_views<'s>(
    state: &'s AgentState,
    live_output: &'s HashMap<String, String>,
) -> Vec<MessageView<'s>> {
    state
        .context
        .history
        .state()
        .messages
        .iter()
        .map(|msg| MessageView {
            msg,
            live: live_output,
        })
        .collect()
}

#[derive(Debug)]
pub struct Tab<'a> {
    pub router: Option<AgentRouterHandle>,
    pub aid: AgentId,
    pub state: AgentState,
    /// assistant cached for ui
    pub assistant_config: Option<ModelConfig>,
    pub project: Project,

    pub scroll: ScrollElements,
    pub input: MessageInput<'a>,
    pub info: InfoWidget,
    /// streamed-so-far output per in-flight tool call, rendered live into
    /// the pending call's widget; the finalized item replaces it (§2.5)
    pub live_output: HashMap<String, String>,
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
        let mut tab = Self {
            router,
            aid,
            state,
            assistant_config: None,
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
            live_output: HashMap::new(),
        };
        tab.refresh_assistant_config();
        tab
    }

    pub fn refresh_assistant_config(&mut self) {
        self.assistant_config = self
            .project
            .assistants()
            .assistant(&self.state.assistant)
            .ok()
            .map(|assistant| assistant.config);
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

    pub fn scroll(
        &mut self,
        op: ScrollOp,
    ) {
        let views = message_views(&self.state, &self.live_output);
        self.scroll.scroll(&views, op);
    }

    pub fn router(&self) -> Result<&AgentRouterHandle> {
        self.router
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("agent isn't attached (yet?)"))
    }
}
