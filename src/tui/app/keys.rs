use anyhow::Result;
use crossterm::event::KeyCode::*;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;

use crate::tui::app::App;

impl<'a> App<'a> {
    pub async fn key(
        &mut self,
        event: KeyEvent,
    ) -> Result<()> {
        if event.kind != KeyEventKind::Press {
            // apparently windows sends key release events too; not that I care but just in case:
            return Ok(());
        }
        _ = self.key_global(event) || self.key_tab(event).await? || self.key_default(event).await?;
        Ok(())
    }

    fn key_global(
        &mut self,
        event: KeyEvent,
    ) -> bool {
        use crossterm::event::KeyCode::*;
        match event.code {
            Char('c') if event.modifiers == KeyModifiers::CONTROL => {
                self.should_exit = true;
                true
            }
            _ => false,
        }
    }

    async fn key_tab(
        &mut self,
        event: KeyEvent,
    ) -> Result<bool> {
        let handled = self
            .selected_tab()
            .and_then(|idx| self.tabs.get_index_mut(idx))
            .map(|(_, tab)| tab.key(event));
        if let Some(f) = handled {
            f.await
        } else {
            Ok(false)
        }
    }

    async fn key_default(
        &mut self,
        event: KeyEvent,
    ) -> Result<bool> {
        use super::NotificationKind::*;
        fn show_hide(hidden: bool) -> &'static str {
            if hidden { "hide" } else { "show" }
        }

        match event.code {
            Char('t') => {
                self.ctx.hide_tools = !self.ctx.hide_tools;
                self.notify(
                    Info,
                    format!("tool calls: {}", show_hide(self.ctx.hide_tools)),
                );
            }
            Char('r') => {
                self.ctx.hide_reasoning = !self.ctx.hide_reasoning;
                self.notify(
                    Info,
                    format!("reasoning: {}", show_hide(self.ctx.hide_reasoning)),
                );
            }
            Char('s') => {
                self.ctx.hide_developer = !self.ctx.hide_developer;
                self.notify(
                    Info,
                    format!("developer msg: {}", show_hide(self.ctx.hide_developer)),
                );
            }

            Char('j') => self.next_tab(),
            Char('k') => self.prev_tab(),
            Char('D') => self.delete_tab().await?,
            Char('Y') => self.duplicate_tab().await?,
            Char('o') => {
                self.new_tab().await?;
            }
            Char('q') => self.should_exit = true,
            _ => {
                return Ok(false);
            }
        };
        Ok(true)
    }
}
