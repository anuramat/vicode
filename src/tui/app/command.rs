use anyhow::Result;

use crate::tui::app::App;
use crate::tui::app::AppFocus;
use crate::tui::app::NotificationKind;
use crate::tui::command::Command;
use crate::tui::command::CommandName;
use crate::tui::command::ScrollOp;
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
            CommandName::InsertEnter => {
                self.focus = AppFocus::Body;
                self.selected_tab_mut()?.insert_mode(true);
            }
            CommandName::InsertPaste => {
                self.selected_tab_mut()?
                    .paste(&command.args.unwrap_or_default());
            }
            CommandName::MsgUndo => self.selected_tab_mut()?.undo(1).await?,
            CommandName::MsgUndoUser => self.selected_tab_mut()?.undo_user().await?,
            CommandName::Quit => self.should_exit = true,
            CommandName::RefreshInfo => self.selected_tab_mut()?.refresh_info().await?,
            CommandName::Scroll => {
                let op: ScrollOp = parse_arg(command.args.as_deref())?
                    .ok_or_else(|| anyhow::anyhow!("missing argument"))?;
                self.scroll(op)?;
            }
            CommandName::SetMultiplier => self
                .selected_tab_mut()?
                .set_multiplier(command.args.as_deref())?,
            CommandName::TabArchive => self.archive_tab().await?,
            CommandName::TabDuplicate => self.duplicate_tab().await?,
            CommandName::TabNew => self.new_tab().await?,
            CommandName::TabNext => self.next_tab(),
            CommandName::TabPrev => self.prev_tab(),
            CommandName::TabSelect => self.select_tab_by_str(command.args.as_deref())?,
            CommandName::ToggleDeveloper => {
                self.ctx.hide_developer = !self.ctx.hide_developer;
                self.notify_hide(self.ctx.hide_developer, "developer msg");
            }
            CommandName::ToggleInfo => {
                self.selected_tab()?;
                self.toggle_focus(AppFocus::Info);
            }
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
            CommandName::ToggleTabs => self.toggle_focus(AppFocus::Tabs),
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

    fn toggle_focus(
        &mut self,
        focus: AppFocus,
    ) {
        self.focus = if self.focus == focus {
            AppFocus::Body
        } else {
            focus
        };
    }

    fn scroll(
        &mut self,
        op: ScrollOp,
    ) -> Result<()> {
        match self.focus {
            AppFocus::Body => self.selected_tab_mut()?.scroll(op),
            AppFocus::Tabs => self.scroll_tabs(op),
            AppFocus::Info => self.selected_tab_mut()?.info.scroll(op),
        }
        Ok(())
    }

    fn scroll_tabs(
        &mut self,
        op: ScrollOp,
    ) {
        match op {
            ScrollOp::HalfPageUp
            | ScrollOp::LineUp
            | ScrollOp::PageUp
            | ScrollOp::PrevElement
            | ScrollOp::Up => self.prev_tab(),
            ScrollOp::HalfPageDown
            | ScrollOp::LineDown
            | ScrollOp::NextElement
            | ScrollOp::PageDown
            | ScrollOp::Down => self.next_tab(),
            ScrollOp::Top => {
                self.select_tab(Some(0));
            }
            ScrollOp::Bottom => {
                self.select_tab(self.tabs.len().checked_sub(1));
            }
        }
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
    use git2::Repository;
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::id::AgentId;
    use crate::project::layout::LayoutTrait;
    use crate::tui::tab::Tab;
    use crate::tui::widgets::input::CompletionItem;
    use crate::tui::widgets::input::CompletionSource;
    use crate::tui::widgets::input::Input;
    use crate::tui::widgets::input::InputOpts;

    #[tokio::test]
    async fn completion_commands_target_tab_in_insert_mode() {
        let project = crate::project::Project::new_test().unwrap().0;
        let mut app = App::new(project.clone(), Default::default());
        let aid = AgentId::from("tab".to_string());
        Repository::init(project.agent_workdir(&aid)).unwrap();
        let state = AgentState::fake(&project);
        let mut tab = Tab::new(
            Some(crate::agent::router::AgentRouter::test_handle()),
            aid.clone(),
            state,
            &project,
        );
        tab.input.input = Input::new(InputOpts {
            source: CompletionSource::Freeform(vec![(
                '@',
                vec![CompletionItem::new("@src/main.rs".into())],
            )]),
            height: 5,
            clear_on_unfocus: false,
        });
        tab.insert_mode(true);

        app.tabs.insert(aid, tab);
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

    #[tokio::test]
    async fn tab_focus_owns_contextual_scroll_commands() {
        let mut app = App::new(
            crate::project::Project::new_test().unwrap().0,
            Default::default(),
        );
        let project = app.project.clone();
        let state = AgentState::fake(&project);
        app.tabs = ["a", "b"]
            .into_iter()
            .map(|id| {
                let aid = AgentId::from(id.to_string());
                (aid.clone(), Tab::new(None, aid, state.clone(), &project))
            })
            .collect();
        app.focus = AppFocus::Body;
        app.rebuild_tablist();
        app.select_tab(Some(0));

        app.execute(Command {
            name: CommandName::ToggleTabs,
            args: None,
        })
        .await
        .unwrap();
        app.execute(Command {
            name: CommandName::Scroll,
            args: Some("down".into()),
        })
        .await
        .unwrap();
        assert_eq!(app.selected_tab_idx(), Some(1));

        app.execute(Command {
            name: CommandName::Scroll,
            args: Some("up".into()),
        })
        .await
        .unwrap();

        assert_eq!(app.focus, AppFocus::Tabs);
        assert_eq!(app.selected_tab_idx(), Some(0));

        app.execute(Command {
            name: CommandName::ToggleTabs,
            args: None,
        })
        .await
        .unwrap();

        assert_eq!(app.focus, AppFocus::Body);
    }
}
