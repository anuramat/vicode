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
        let insert_mode = self.selected_tab().is_ok_and(|tab| tab.insert_mode);
        if insert_mode {
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
}

// TODO move to an app method?
impl Command {
    pub async fn execute(
        self,
        app: &mut App<'_>,
    ) -> Result<()> {
        match self.name {
            CommandName::AppQuit => app.should_exit = true,
            CommandName::ToggleTools => app.toggle_tools(),
            CommandName::ToggleReasoning => app.toggle_reasoning(),
            CommandName::ToggleDeveloper => app.toggle_developer(),
            CommandName::TabNext => app.next_tab(),
            CommandName::TabPrev => app.prev_tab(),
            CommandName::TabDelete => app.delete_tab().await?,
            CommandName::TabDuplicate => app.duplicate_tab().await?,
            CommandName::TabNew => app.new_tab().await?,
            CommandName::Submit => app.selected_tab_mut()?.submit().await?,
            CommandName::Retry => app.selected_tab_mut()?.retry().await?,
            CommandName::Abort => app.selected_tab_mut()?.abort().await?,
            CommandName::EnterInsert => app.selected_tab_mut()?.insert_mode(true),
            CommandName::ExitInsert => app.selected_tab_mut()?.insert_mode(false),
            CommandName::AssistantNext => app.selected_tab_mut()?.next_assistant().await?,
            CommandName::ScrollLineUp => app.selected_tab_mut()?.scroll_line_up(),
            CommandName::ScrollLineDown => app.selected_tab_mut()?.scroll_line_down(),
            CommandName::ScrollHalfPageUp => app.selected_tab_mut()?.scroll_half_page_up(),
            CommandName::ScrollHalfPageDown => app.selected_tab_mut()?.scroll_half_page_down(),
            CommandName::ScrollPageUp => app.selected_tab_mut()?.scroll_page_up(),
            CommandName::ScrollPageDown => app.selected_tab_mut()?.scroll_page_down(),
            CommandName::ScrollPrevElement => app.selected_tab_mut()?.scroll_prev_element(),
            CommandName::ScrollNextElement => app.selected_tab_mut()?.scroll_next_element(),
            CommandName::ScrollTop => app.selected_tab_mut()?.scroll_top(),
            CommandName::ScrollBottom => app.selected_tab_mut()?.scroll_bottom(),
            CommandName::SetMultiplier => app
                .selected_tab_mut()?
                .set_multiplier(self.args.as_deref())?,
        }
        Ok(())
    }
}
