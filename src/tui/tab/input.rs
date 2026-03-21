use anyhow::Context;
use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Padding;

use crate::agent::handle::UserPrompt;
use crate::llm::message::Message;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::llm::tokens::count_text_tokens;
use crate::tui::app::handle::AppEvent;
use crate::tui::tab::AssistantState;
use crate::tui::tab::Tab;
use crate::tui::tab::TabState;

fn block() -> Block<'static> {
    Block::new().borders(Borders::TOP).padding(Padding {
        left: 1,
        right: 1,
        top: 0,
        bottom: 0,
    })
}

fn thick() -> Block<'static> {
    block().border_type(BorderType::Thick)
}

fn thin() -> Block<'static> {
    block().border_type(BorderType::Plain)
}

impl<'a> Tab<'a> {
    pub fn insert_mode(
        &mut self,
        active: bool,
    ) {
        self.insert_mode = active;
        self.update_input_border();
    }

    pub fn update_input_border(&mut self) {
        let block = if self.insert_mode { thick() } else { thin() };
        let text = self.user_input.0.lines().join("\n");
        let tokens = count_text_tokens(&text);

        let title = if self.multiplier > 1 {
            format!(" x{} | {} T ", self.multiplier, tokens)
        } else {
            format!(" {} T ", tokens)
        };

        self.user_input.0.set_block(block.title(title));
    }

    pub async fn submit(&mut self) -> Result<()> {
        // read user input
        let text = self.user_input.0.lines().join("\n").trim().to_string();

        // clear input area and exit insert mode
        self.user_input = Default::default();
        self.insert_mode = false;

        // drop empty messages
        if text.is_empty() {
            return Ok(());
        }

        // forward to agent
        let prompt = UserPrompt {
            text: Some(text.clone()),
            multiplier: self.multiplier,
            loc: self.agent_state.context.history.len(),
        };

        self.set_state(TabState::Running(AssistantState::Generating))
            .await?;
        self.tx
            .send(AppEvent::UserPrompt(self.aid.clone(), prompt))
            .await?;
        Ok(())
    }

    pub async fn retry(&mut self) -> Result<()> {
        if matches!(
            self.state,
            TabState::Loading | TabState::Running(AssistantState::Generating)
        ) {
            return Ok(());
        }
        self.set_state(TabState::Running(AssistantState::Generating))
            .await?;
        self.tx.send(AppEvent::RetryTurn(self.aid.clone())).await?;
        Ok(())
    }

    pub async fn abort(&mut self) -> Result<()> {
        self.tx
            .send(AppEvent::AbortTurn(
                self.agent_state.context.history.len() - 1,
                self.aid.clone(),
            ))
            .await?;
        Ok(())
    }

    pub async fn next_assistant(&mut self) -> Result<()> {
        if !self.state.idle() {
            return Ok(());
        }
        let id = ASSISTANT_POOL
            .get()
            .unwrap()
            .next_assistant(&self.agent_state.context.assistant_id)
            .with_context(|| "couldn't find the provided assistant id")?;
        self.agent_state.context.assistant_id = id.clone();
        self.tx
            .send(AppEvent::SetAssistant(self.aid.clone(), id))
            .await?;
        Ok(())
    }

    pub async fn key_insert(
        &mut self,
        input: KeyEvent,
    ) -> Result<()> {
        use crossterm::event::KeyCode::Char;
        use crossterm::event::KeyCode::{self};
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers as Mods;
        use tui_textarea::CursorMove::*;
        let area = &mut self.user_input.0;
        let KeyEvent {
            code,
            modifiers: mods,
            ..
        } = input;
        match code {
            KeyCode::Enter => self.submit().await?,
            KeyCode::Esc => self.insert_mode(false),
            // emulate readline/bash shortcuts

            // move:
            Char('a') if mods == Mods::CONTROL => {
                area.move_cursor(Head);
            }
            Char('e') if mods == Mods::CONTROL => {
                area.move_cursor(End);
            }
            Char('b') if mods == Mods::ALT => {
                area.move_cursor(WordBack);
            }
            Char('f') if mods == Mods::ALT => {
                area.move_cursor(WordForward);
            }
            Char('b') if mods == Mods::CONTROL => {
                area.move_cursor(Back);
            }
            Char('f') if mods == Mods::CONTROL => {
                area.move_cursor(Forward);
            }

            // delete:
            Char('u') if mods == Mods::CONTROL => {
                area.delete_line_by_head();
            }
            Char('k') if mods == Mods::CONTROL => {
                area.delete_line_by_end();
            }
            // TODO c-w kill WORD back
            // TODO c-a-d -- kill WORD
            KeyCode::Backspace if mods == Mods::ALT => {
                area.delete_word();
            }
            Char('d') if mods == Mods::ALT => {
                area.delete_next_word();
            }
            Char('h') if mods == Mods::CONTROL => {
                area.delete_char();
            }
            Char('d') if mods == Mods::CONTROL => {
                area.delete_next_char();
            }
            _ => _ = area.input_without_shortcuts(input),
        }
        self.update_input_border();
        Ok(())
    }
}
