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
        let history = self.agent_state.context.history.as_ref();
        match event.code {
            Enter => self.submit().await?,
            Char('R') => self.retry().await?,
            Char('X') => self.abort().await?,
            Char('i') => self.insert_mode(true),

            Up => self.scroll.line_up(history),
            Down => self.scroll.line_down(history),

            Tab => self.next_assistant().await?,

            Char('y') if event.modifiers == Mods::CONTROL => self.scroll.line_up(history),
            Char('e') if event.modifiers == Mods::CONTROL => self.scroll.line_down(history),

            Char('u') if event.modifiers == Mods::CONTROL => self.scroll.half_page_up(history),
            Char('d') if event.modifiers == Mods::CONTROL => self.scroll.half_page_down(history),

            Char('b') if event.modifiers == Mods::CONTROL => self.scroll.page_up(history),
            Char('f') if event.modifiers == Mods::CONTROL => self.scroll.page_down(history),

            Char('[') => self.scroll.prev_element(history),
            Char(']') => self.scroll.next_element(history),
            // TODO add {/} to move between *user* messages
            Char('g') => self.scroll.top(history),
            Char('G') => self.scroll.bottom(),

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
