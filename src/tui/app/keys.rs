// TODO split this file
use anyhow::Context;
use anyhow::Result;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;

use crate::config::CONFIG;
use crate::tui::app::App;
use crate::tui::app::NotificationKind;
use crate::tui::command::Command;
use crate::tui::command::CommandName;
use crate::tui::tab::Tab;

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
        self.update_input_border();
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
        if self.cmdline.input.focus {
            if let Some(command) = CONFIG.keymap.cmdline(event) {
                command.execute(self).await?;
            } else {
                self.cmdline.input.handle(event);
            }
        } else if self.selected_tab().is_ok_and(|tab| tab.insert_mode) {
            if let Some(command) = CONFIG.keymap.insert(event) {
                command.execute(self).await?;
            } else {
                self.selected_tab_mut()?.key_insert(event).await?
            }
        } else if let Some(command) = CONFIG.keymap.normal(event) {
            command.execute(self).await?;
        }
        Ok(())
    }

    // TODO maybe show the entire rendercontext in the notification, not just the individual flags?

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
        if self.cmdline.input.focus {
            self.submit_cmdline().await?
        } else {
            self.selected_tab_mut()?.submit().await?
        }
        Ok(())
    }

    fn exit_input(&mut self) -> Result<()> {
        if self.cmdline.input.focus {
            self.cmdline.input.focus(false);
        } else {
            self.selected_tab_mut()?.insert_mode(false)
        }
        Ok(())
    }

    async fn submit_cmdline(&mut self) -> Result<()> {
        let text = self.cmdline.input.textarea.lines().join("\n");
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        // TODO can we avoid this somehow? recursive call requires pin
        Box::pin(text.parse::<Command>()?.execute(self)).await
    }

    fn enter_cmdline(&mut self) {
        self.cmdline.input.focus = true;
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
            CommandName::CmdlineEnter => app.enter_cmdline(),
            CommandName::CompletionCancel => app.cmdline.input.completion_cancel(),
            CommandName::CompletionNext => app.cmdline.input.completion_next(),
            CommandName::CompletionPrev => app.cmdline.input.completion_prev(),
            CommandName::InputExit => app.exit_input()?,
            CommandName::InputSubmit => app.submit().await?,
            CommandName::InsertEnter => app.selected_tab_mut()?.insert_mode(true),
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
            CommandName::ToggleDeveloper => app.toggle_developer(),
            CommandName::ToggleReasoning => app.toggle_reasoning(),
            CommandName::ToggleTools => app.toggle_tools(),
            CommandName::TurnAbort => app.selected_tab_mut()?.abort().await?,
            CommandName::TurnRetry => app.selected_tab_mut()?.retry().await?,
        }
        Ok(())
    }
}
