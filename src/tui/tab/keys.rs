use anyhow::Result;
use crossterm::event::KeyEvent;

use crate::tui::tab::Tab;

impl<'a> Tab<'a> {
    pub async fn key(
        &mut self,
        event: KeyEvent,
    ) -> Result<bool> {
        if self.insert_mode {
            self.key_insert(event).await;
            Ok(true)
        } else {
            return self.key_normal(event).await;
        }
    }

    pub async fn key_normal(
        &mut self,
        event: KeyEvent,
    ) -> Result<bool> {
        use crossterm::event::KeyCode::Char;
        use crossterm::event::KeyCode::*;
        use crossterm::event::KeyModifiers as Mods;
        match event.code {
            Enter => self.submit().await,
            Char('i') => self.insert_mode(true),

            Up => self.history.line_up(),
            Down => self.history.line_down(),

            Char('y') if event.modifiers == Mods::CONTROL => self.history.line_up(),
            Char('e') if event.modifiers == Mods::CONTROL => self.history.line_down(),

            Char('u') if event.modifiers == Mods::CONTROL => self.history.half_page_up(),
            Char('d') if event.modifiers == Mods::CONTROL => self.history.half_page_down(),

            Char('b') if event.modifiers == Mods::CONTROL => self.history.page_up(),
            Char('f') if event.modifiers == Mods::CONTROL => self.history.page_down(),

            Char('[') => self.history.prev_element(),
            Char(']') => self.history.next_element(),
            // TODO add {/} to move between *user* messages
            Char('g') => self.history.top(),
            Char('G') => self.history.bottom(),

            Char(c @ '1'..='9') => {
                self.multiplier = c.to_digit(10).unwrap() as usize;
                self.update_input_border();
            }

            _ => {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
