use anyhow::Result;

use crate::tui::app::App;
use crate::tui::app::NotificationKind;
use crate::tui::command::Command;
use crate::tui::command::CommandName;
use crate::tui::command::parse_arg;
use crate::tui::widgets::input::Input;

impl<'a> App<'a> {
    pub async fn execute(
        &mut self,
        command: Command,
    ) -> Result<()> {
        match command.name {
            CommandName::AssistantNext => self.selected_tab()?.cycle_assistant(false).await?,
            CommandName::AssistantPrev => self.selected_tab()?.cycle_assistant(true).await?,
            CommandName::CmdlineEnter => self.cmdline.input.set_focus(true),
            CommandName::Compact => {
                self.selected_tab_mut()?
                    .compact(command.args.as_deref())
                    .await?;
            }
            CommandName::CompletionCancel => self.active_input()?.completion_cancel(),
            CommandName::CompletionNext => self.active_input()?.completion_next(),
            CommandName::CompletionPrev => self.active_input()?.completion_prev(),
            CommandName::InputExit => self.exit_input()?,
            CommandName::InputSubmit => self.submit().await?,
            CommandName::InsertEnter => self.selected_tab_mut()?.insert_mode(true),
            CommandName::InsertPaste => {
                self.selected_tab_mut()?
                    .paste(&command.args.unwrap_or_default());
            }
            CommandName::MsgUndo => self.selected_tab_mut()?.undo(1).await?,
            CommandName::MsgUndoUser => self.selected_tab_mut()?.undo_user().await?,
            CommandName::Quit => self.should_exit = true,
            CommandName::RefreshInfo => self.selected_tab_mut()?.refresh_info().await?,
            CommandName::ScrollBottom => self.selected_tab_mut()?.scroll_bottom(),
            CommandName::ScrollHalfPageDown => self.selected_tab_mut()?.scroll_half_page_down(),
            CommandName::ScrollHalfPageUp => self.selected_tab_mut()?.scroll_half_page_up(),
            CommandName::ScrollLineDown => self.selected_tab_mut()?.scroll_line_down(),
            CommandName::ScrollLineUp => self.selected_tab_mut()?.scroll_line_up(),
            CommandName::ScrollNextElement => self.selected_tab_mut()?.scroll_next_element(),
            CommandName::ScrollPageDown => self.selected_tab_mut()?.scroll_page_down(),
            CommandName::ScrollPageUp => self.selected_tab_mut()?.scroll_page_up(),
            CommandName::ScrollPrevElement => self.selected_tab_mut()?.scroll_prev_element(),
            CommandName::ScrollTop => self.selected_tab_mut()?.scroll_top(),
            CommandName::SetMultiplier => self
                .selected_tab_mut()?
                .set_multiplier(command.args.as_deref())?,
            CommandName::TabDelete => self.delete_tab().await?,
            CommandName::TabDuplicate => self.duplicate_tab().await?,
            CommandName::TabNew => self.new_tab().await?,
            CommandName::TabNext => self.next_tab(),
            CommandName::TabPrev => self.prev_tab(),
            CommandName::TabSelect => self.select_tab_by_str(command.args.as_deref())?,
            CommandName::ToggleDeveloper => {
                self.ctx.hide_developer = !self.ctx.hide_developer;
                self.notify_hide(self.ctx.hide_developer, "developer msg");
            }
            CommandName::ToggleInfo => self.selected_tab_mut()?.toggle_focus(),
            CommandName::ToggleMarkdown => {
                self.ctx.render_markdown = !self.ctx.render_markdown;
                self.notify(
                    NotificationKind::Info,
                    format!(
                        "markdown rendering: {}",
                        if self.ctx.render_markdown {
                            "on"
                        } else {
                            "off"
                        }
                    ),
                );
            }
            CommandName::ToggleReasoning => {
                self.ctx.hide_reasoning = !self.ctx.hide_reasoning;
                self.notify_hide(self.ctx.hide_reasoning, "reasoning");
            }
            CommandName::ToggleTabs => self.toggle_tabs(),
            CommandName::ToggleTools => {
                self.ctx.hide_tools = !self.ctx.hide_tools;
                self.notify_hide(self.ctx.hide_tools, "tool calls");
            }
            CommandName::TurnAbort => self.selected_tab_mut()?.abort().await?,
            CommandName::TurnRetry => self.selected_tab_mut()?.retry().await?,
            CommandName::None => {}
        }
        Ok(())
    }

    async fn submit(&mut self) -> Result<()> {
        if self.cmdline.input.focused() {
            let command = self.cmdline.take_command()?;
            // TODO can we avoid this somehow? recursive call requires pin; maybe make this a non-command?
            Box::pin(self.execute(command)).await
        } else {
            self.selected_tab_mut()?.submit().await
        }
    }

    fn exit_input(&mut self) -> Result<()> {
        if self.cmdline.input.focused() {
            self.cmdline.input.set_focus(false);
        } else {
            self.selected_tab_mut()?.insert_mode(false);
        }
        Ok(())
    }

    fn toggle_tabs(&mut self) {
        self.show_tabs = !self.show_tabs;
    }

    fn notify_hide(
        &mut self,
        hidden: bool,
        label: &str,
    ) {
        self.notify(
            NotificationKind::Info,
            format!("{label}: {}", if hidden { "hide" } else { "show" }),
        );
    }

    fn select_tab_by_str(
        &mut self,
        value: Option<&str>,
    ) -> Result<()> {
        let idx: Option<usize> = parse_arg(value)?;
        self.select_tab(idx);
        Ok(())
    }

    fn active_input(&mut self) -> Result<&mut Input<'a>> {
        if self.cmdline.input.focused() {
            return Ok(&mut self.cmdline.input);
        }
        let input: &mut Input<'_> = &mut self.selected_tab_mut()?.input;
        anyhow::ensure!(input.focused(), "no focused input");
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use futures::future::AbortHandle;
    use git2::Repository;
    use similar_asserts::assert_eq;
    use tokio::sync::mpsc::channel;

    use super::*;
    use crate::agent::AgentHandle;
    use crate::agent::AgentState;
    use crate::agent::AgentStatus;
    use crate::agent::AgentTopology;
    use crate::agent::handle::AgentEvent;
    use crate::agent::id::AgentId;
    use crate::config::Config;
    use crate::llm::history::History;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;
    use crate::project::layout::LayoutTrait;
    use crate::tui::tab::Tab;
    use crate::tui::tab::TabEntry;
    use crate::tui::widgets::input::CompletionItem;
    use crate::tui::widgets::input::CompletionSource;
    use crate::tui::widgets::input::Input;
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

                [providers.main]
                api = "responses"
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
        let mut app = App::new(project.clone());
        let (tx, _rx) = channel(1);
        let (agent_tx, _agent_rx) = channel::<AgentEvent>(1);
        let aid = AgentId::from("tab".to_string());
        Repository::init(project.agent_workdir(&aid)).unwrap();
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: assistant().await,
            topology: AgentTopology::default(),
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("".into()),
            },
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
                .key_insert(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }

        app.execute(Command {
            name: CommandName::CompletionNext,
            args: None,
        })
        .await
        .unwrap();

        assert_eq!(
            app.selected_tab().unwrap().input.textarea.lines(),
            ["open @src/main.rs"]
        );
    }
}
