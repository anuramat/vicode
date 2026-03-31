// TODO refactor using Input
use anyhow::Context;
use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Padding;

use crate::agent::handle::ExternalEvent;
use crate::agent::handle::UserPrompt;
use crate::llm::message::Message;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::llm::tokens::count_text_tokens;
use crate::tui::tab::Tab;

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
    async fn cycle_assistant(
        &mut self,
        step: isize,
    ) -> Result<()> {
        // XXX replace this with an externalEvent
        if !self.agent.state.status.idle() {
            return Ok(());
        }
        let pool = ASSISTANT_POOL.get().unwrap();
        let id = pool
            .switch_assistant(&self.agent.state.assistant.id, step)
            .with_context(|| "couldn't find the provided assistant id")?;
        self.agent.state.assistant = pool.assistant(&id)?;
        self.agent.send(ExternalEvent::SetAssistant(id)).await?;
        Ok(())
    }

    // TODO instead have two methods, and clean up if text area is empty on exit
    pub fn insert_mode(
        &mut self,
        active: bool,
    ) {
        self.insert_mode = active;
        self.update_input_border();
    }

    pub fn update_input_border(&mut self) {
        let block = if self.insert_mode { thick() } else { thin() };
        let text = self.user_input.0.textarea.lines().join("\n");
        let tokens = count_text_tokens(&text);

        let title = if self.multiplier > 1 {
            format!(" x{} | {} T ", self.multiplier, tokens)
        } else {
            format!(" {} T ", tokens)
        };

        self.user_input.0.textarea.set_block(block.title(title));
    }

    pub async fn submit(&mut self) -> Result<()> {
        // read user input
        let text = self
            .user_input
            .0
            .textarea
            .lines()
            .join("\n")
            .trim()
            .to_string();

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
            generation: self.agent.state.context.history.generation(),
        };

        self.agent.send(ExternalEvent::Submit(prompt)).await?;
        Ok(())
    }

    pub async fn retry(&mut self) -> Result<()> {
        self.agent.send(ExternalEvent::Retry).await?;
        Ok(())
    }

    pub async fn compact(
        &mut self,
        n: Option<&str>,
    ) -> Result<()> {
        let n = if let Some(n) = n {
            n.parse()
                .with_context(|| format!("invalid compact number: {n}"))?
        } else {
            self.agent.state.context.history.len()
        };
        self.agent.send(ExternalEvent::Compact(n)).await?;
        Ok(())
    }

    pub async fn abort(&mut self) -> Result<()> {
        self.agent.send(ExternalEvent::Abort).await?;
        Ok(())
    }

    pub async fn undo(
        &mut self,
        n: usize,
    ) -> Result<()> {
        if n > self.agent.state.context.history.len() {
            return Ok(());
        }
        self.agent.send(ExternalEvent::Undo(n)).await?;
        Ok(())
    }

    pub async fn undo_user(&mut self) -> Result<()> {
        let messages = &self.agent.state.context.history;
        let Some(loc) = messages
            .iter()
            .rposition(|entry| matches!(entry.message, Message::User(_)))
        else {
            return Ok(());
        };
        let n = messages.len() - loc;
        self.undo(n).await?;
        Ok(())
    }

    pub async fn next_assistant(&mut self) -> Result<()> {
        self.cycle_assistant(1).await
    }

    pub async fn prev_assistant(&mut self) -> Result<()> {
        self.cycle_assistant(-1).await
    }

    pub async fn key_insert(
        &mut self,
        input: KeyEvent,
    ) -> Result<()> {
        self.user_input.0.handle(input);
        self.update_input_border();
        Ok(())
    }

    pub async fn paste(
        &mut self,
        content: &str,
    ) {
        // TODO instead of putting it in the input area, show "pasted: <contents>" block above input area or something
        self.user_input.0.textarea.insert_str(content);
        self.update_input_border();
    }
}
