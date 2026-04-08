// TODO split this file
use anyhow::Context;
use anyhow::Result;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;

use crate::tui::app::App;
use crate::tui::app::NotificationKind;
use crate::tui::command::Command;
use crate::tui::command::CommandName;
use crate::tui::tab::Tab;
use crate::tui::widgets::input::Input;

fn show_hide(hidden: bool) -> &'static str {
    if hidden { "hide" } else { "show" }
}

impl<'a> Tab<'a> {
    fn set_multiplier(
        &mut self,
        value: Option<&str>,
    ) -> Result<()> {
        let value: u8 = value
            .map(|s| {
                s.parse()
                    .with_context(|| format!("invalid multiplier: {s}"))
            })
            .transpose()?
            .unwrap_or(1);
        anyhow::ensure!(value > 0, "multiplier must be positive");
        self.multiplier = value as usize;
        self.update_input_title();
        Ok(())
    }
}

impl<'a> App<'a> {
    pub async fn key(
        &mut self,
        event: KeyEvent,
    ) -> Result<()> {
        if event.kind != KeyEventKind::Press {
            // apparently windows sends key release events too; not that I care but just in case:
            return Ok(());
        }
        // TODO add failsafe, so that there's always a way to exit the app even if the keymap is messed up (e.g. spam ctrl-c to quit)
        let keymap = &self.project.config().keymap;
        if self.cmdline.input.focused() {
            if let Some(command) = keymap.cmdline(event) {
                command.execute(self).await?;
            } else {
                self.cmdline.input.handle(event);
            }
        } else if self.selected_tab().is_ok_and(|tab| tab.input.focused()) {
            if let Some(command) = keymap.insert(event) {
                command.execute(self).await?;
            } else {
                self.selected_tab_mut()?.key_insert(event).await?;
            }
        } else if let Some(command) = keymap.normal(event) {
            command.execute(self).await?;
        }
        Ok(())
    }

    // TODO maybe show the entire rendercontext in the notification, not just the individual flags?

    fn toggle_markdown(&mut self) {
        self.ctx.render_markdown = !self.ctx.render_markdown;
        self.notify(
            NotificationKind::Info,
            format!("markdown: {}", show_hide(self.ctx.render_markdown)),
        );
    }

    fn toggle_tools(&mut self) {
        self.ctx.hide_tools = !self.ctx.hide_tools;
        self.notify(
            NotificationKind::Info,
            format!("tool calls: {}", show_hide(self.ctx.hide_tools)),
        );
    }

    fn toggle_reasoning(&mut self) {
        self.ctx.hide_reasoning = !self.ctx.hide_reasoning;
        self.notify(
            NotificationKind::Info,
            format!("reasoning: {}", show_hide(self.ctx.hide_reasoning)),
        );
    }

    fn toggle_developer(&mut self) {
        self.ctx.hide_developer = !self.ctx.hide_developer;
        self.notify(
            NotificationKind::Info,
            format!("developer msg: {}", show_hide(self.ctx.hide_developer)),
        );
    }

    async fn submit(&mut self) -> Result<()> {
        if self.cmdline.input.focused() {
            self.submit_cmdline().await?;
        } else {
            self.selected_tab_mut()?.submit().await?;
        }
        Ok(())
    }

    fn exit_input(&mut self) -> Result<()> {
        if self.cmdline.input.focused() {
            self.cmdline.input.set_focus(false);
        } else {
            self.selected_tab_mut()?.insert_mode(false);
        }
        Ok(())
    }

    async fn submit_cmdline(&mut self) -> Result<()> {
        let command = self.cmdline.take_command()?;
        // TODO can we avoid this somehow? recursive call requires pin
        Box::pin(command.execute(self)).await
    }

    fn toggle_tabs(&mut self) {
        self.show_tabs = !self.show_tabs;
    }

    fn select_tab_arg(
        &mut self,
        value: Option<&str>,
    ) -> Result<()> {
        let idx = value
            .map(|s| s.parse().with_context(|| format!("invalid tab index: {s}")))
            .transpose()?;
        if let Some(idx) = idx {
            anyhow::ensure!(idx < self.tabs.len(), "tab index out of bounds: {idx}");
        }
        self.select_tab(idx);
        Ok(())
    }

    fn input(&mut self) -> Result<&mut Input<'a>> {
        if self.cmdline.input.get_mut().is_ok() {
            self.cmdline.input.get_mut()
        } else {
            self.selected_tab_mut()?.input.get_mut()
        }
    }
}

// TODO move to an app method?
impl Command {
    pub async fn execute(
        self,
        app: &mut App<'_>,
    ) -> Result<()> {
        match self.name {
            CommandName::AssistantNext => app.selected_tab_mut()?.next_assistant().await?,
            CommandName::AssistantPrev => app.selected_tab_mut()?.prev_assistant().await?,
            CommandName::CmdlineEnter => app.cmdline.input.set_focus(true),
            CommandName::Compact => {
                app.selected_tab_mut()?
                    .compact(self.args.as_deref())
                    .await?;
            }
            CommandName::CompletionCancel => {
                app.input()?.completion_cancel();
            }
            CommandName::CompletionNext => {
                app.input()?.completion_next();
            }
            CommandName::CompletionPrev => {
                app.input()?.completion_prev();
            }
            CommandName::InputExit => app.exit_input()?,
            CommandName::InputSubmit => app.submit().await?,
            CommandName::InsertEnter => app.selected_tab_mut()?.insert_mode(true),
            CommandName::InsertPaste => {
                app.selected_tab_mut()?
                    .paste(&self.args.unwrap_or_default())
                    .await;
            }
            CommandName::MsgUndo => app.selected_tab_mut()?.undo(1).await?,
            CommandName::MsgUndoUser => app.selected_tab_mut()?.undo_user().await?,
            CommandName::Quit => app.should_exit = true,
            CommandName::ScrollBottom => app.selected_tab_mut()?.scroll_bottom(),
            CommandName::ScrollHalfPageDown => app.selected_tab_mut()?.scroll_half_page_down(),
            CommandName::ScrollHalfPageUp => app.selected_tab_mut()?.scroll_half_page_up(),
            CommandName::ScrollLineDown => app.selected_tab_mut()?.scroll_line_down(),
            CommandName::ScrollLineUp => app.selected_tab_mut()?.scroll_line_up(),
            CommandName::ScrollNextElement => app.selected_tab_mut()?.scroll_next_element(),
            CommandName::ScrollPageDown => app.selected_tab_mut()?.scroll_page_down(),
            CommandName::ScrollPageUp => app.selected_tab_mut()?.scroll_page_up(),
            CommandName::ScrollPrevElement => app.selected_tab_mut()?.scroll_prev_element(),
            CommandName::ScrollTop => app.selected_tab_mut()?.scroll_top(),
            CommandName::SetMultiplier => app
                .selected_tab_mut()?
                .set_multiplier(self.args.as_deref())?,
            CommandName::TabDelete => app.delete_tab().await?,
            CommandName::TabDuplicate => app.duplicate_tab().await?,
            CommandName::TabNew => app.new_tab().await?,
            CommandName::TabNext => app.next_tab(),
            CommandName::TabPrev => app.prev_tab(),
            CommandName::TabSelect => app.select_tab_arg(self.args.as_deref())?,
            CommandName::ToggleDeveloper => app.toggle_developer(),
            CommandName::ToggleMarkdown => app.toggle_markdown(),
            CommandName::ToggleReasoning => app.toggle_reasoning(),
            CommandName::ToggleTabs => app.toggle_tabs(),
            CommandName::ToggleTools => app.toggle_tools(),
            CommandName::TurnAbort => app.selected_tab_mut()?.abort().await?,
            CommandName::TurnRetry => app.selected_tab_mut()?.retry().await?,
            CommandName::None => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyModifiers;
    use futures::future::AbortHandle;
    use git2::Repository;
    use similar_asserts::assert_eq;
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::AgentContext;
    use crate::agent::AgentHandle;
    use crate::agent::AgentState;
    use crate::agent::AgentStatus;
    use crate::agent::AgentTopology;
    use crate::agent::handle::AgentEvent;
    use crate::agent::id::AgentId;
    use crate::config::Config;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::layout::LayoutTrait;
    use crate::tui::tab::Tab;
    use crate::tui::tab::TabEntry;
    use crate::tui::widgets::input::CompletionItem;
    use crate::tui::widgets::input::CompletionSource;
    use crate::tui::widgets::input::InputOpts;

    async fn assistant() -> Assistant {
        AssistantPool::from_config(
            &Config::parse_with_defaults(
                r#"
                primary_assistant = ["test"]
                shell_cmd = ["bash", "-c"]

                [sandbox]
                kind = "bwrap"
                bin = "bwrap"
                args = []
                stages = []

                [keymap.cmdline]

                [keymap.normal]

                [keymap.insert]

                [providers.main]
                base_url = "https://api.example.com/v1"

                [assistants.test]
                provider = "main"
                model = "gpt-test"
                "#,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .assistant("test")
        .unwrap()
    }

    #[tokio::test]
    async fn completion_commands_target_tab_in_insert_mode() {
        let project = crate::project::Project::new_test().unwrap();
        let mut app = App::new(project.clone()).await.unwrap();
        let (tx, _rx) = channel(1);
        let (agent_tx, _agent_rx) = channel::<AgentEvent>(1);
        let aid = AgentId::from("tab".to_string());
        Repository::init(project.agent_workdir(&aid)).unwrap();
        let state = AgentState {
            status: AgentStatus::Idle,
            assistant: assistant().await,
            topology: AgentTopology::default(),
            context: AgentContext::default(),
        };
        let mut tab = Tab::new(
            tx,
            aid.clone(),
            AgentHandle {
                tx: agent_tx,
                state,
                abort: AbortHandle::new_pair().0,
            },
            &project,
        )
        .await
        .unwrap();
        tab.input.input = Input::new(InputOpts {
            source: CompletionSource::Freeform(vec![(
                '@',
                vec![CompletionItem::new("@src/main.rs".into())],
            )]),
            height: 5,
            clear_on_unfocus: false,
        });
        tab.insert_mode(true);

        app.tabs.insert(aid, TabEntry::Ready(tab));
        app.rebuild_tablist();
        app.select_tab(Some(0));
        for ch in "open @sr".chars() {
            app.selected_tab_mut()
                .unwrap()
                .key_insert(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))
                .await
                .unwrap();
        }

        Command {
            name: CommandName::CompletionNext,
            args: None,
        }
        .execute(&mut app)
        .await
        .unwrap();

        assert_eq!(
            app.selected_tab().unwrap().input.textarea.lines(),
            ["open @src/main.rs"]
        );
    }
}
