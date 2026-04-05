use anyhow::Context;
use anyhow::Result;
use crossterm::event::KeyEvent;
use git2::Repository;
use ratatui::widgets::Block;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Padding;
use tracing::warn;

use crate::agent::handle::ExternalEvent;
use crate::agent::handle::UserPrompt;
use crate::llm::message::Message;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::tui::tab::Tab;
use crate::tui::widgets::input::CompletionItem;

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

fn file_completion_items(paths: Vec<String>) -> Vec<CompletionItem<'static>> {
    paths.into_iter().map(CompletionItem::new).collect()
}

fn tracked_files(
    project: &Project,
    aid: &crate::agent::id::AgentId,
) -> Result<Vec<String>> {
    let path = project.agent_workdir(aid);
    let repo = Repository::open(path)?;
    let index = repo.index()?;
    index
        .iter()
        .map(|e| Ok(String::from_utf8_lossy(&e.path).to_string()))
        .collect()
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

    // TODO clean up if trimmed is empty
    pub fn insert_mode(
        &mut self,
        active: bool,
    ) {
        self.input.set_focus(active);
        self.update_input_border();
    }

    pub async fn refresh_file_completion(
        &mut self,
        project: &Project,
    ) {
        match tracked_files(project, &self.aid) {
            Ok(paths) => self
                .input
                .completion
                .set_source(file_completion_items(paths)),
            Err(err) => warn!(aid = %self.aid, ?err, "couldn't refresh file completion"),
        }
    }

    // TODO update on completions, ideally make a mut getter or something
    pub fn update_input_border(&mut self) {
        let block = if self.input.focused() {
            thick()
        } else {
            thin()
        };
        let tokens = self.input.count_tokens();

        let title = if self.multiplier > 1 {
            format!(" x{} | {} T ", self.multiplier, tokens)
        } else {
            format!(" {} T ", tokens)
        };

        self.input.0.textarea.set_block(block.title(title));
    }

    pub async fn submit(&mut self) -> Result<()> {
        let text = self.input.take_area().lines().join("\n").trim().to_string();
        if text.is_empty() {
            return Ok(());
        }
        let prompt = UserPrompt {
            text,
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
        self.input.0.handle(input);
        self.update_input_border();
        Ok(())
    }

    pub async fn paste(
        &mut self,
        content: &str,
    ) {
        // TODO instead of putting it in the input area, show "pasted: <contents>" block above input area or something
        self.input.0.textarea.insert_str(content);
        self.update_input_border();
    }
}

#[cfg(test)]
mod tests {
    use futures::future::AbortHandle;
    use git2::Repository;
    use similar_asserts::assert_eq;
    use tokio::sync::mpsc::channel;
    use tui_textarea::CursorMove;

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
    use crate::tui::widgets::input::Input;

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

    // async fn tab(text: &str) -> Tab<'static> {
    //     let project = Project::new_test().unwrap();
    //     let (tx, _rx) = channel(1);
    //     let (agent_tx, _agent_rx) = channel::<AgentEvent>(1);
    //     let aid = AgentId::from("tab-input".to_string());
    //     let state = AgentState {
    //         status: AgentStatus::Idle,
    //         assistant: assistant().await,
    //         topology: AgentTopology::default(),
    //         context: AgentContext::default(),
    //     };
    //     let mut tab = Tab::new(
    //         tx,
    //         aid,
    //         AgentHandle {
    //             tx: agent_tx,
    //             state,
    //             abort: AbortHandle::new_pair().0,
    //         },
    //         &project,
    //     )
    //     .await
    //     .unwrap();
    //     tab.input.0 = Input::new(text, CompletionSource::prefixed_word('@', vec![]), 5);
    //     tab.input.0.textarea.move_cursor(CursorMove::End);
    //     tab
    // }

    fn commit_file(
        repo: &Repository,
        path: &std::path::Path,
        name: &str,
    ) {
        std::fs::write(path.join(name), name).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new(name)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("vicode", "vicode@example.com").unwrap();
        let parent = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| repo.find_commit(oid).ok());
        let parents = parent.iter().collect::<Vec<_>>();
        repo.commit(Some("HEAD"), &signature, &signature, name, &tree, &parents)
            .unwrap();
    }

    // #[tokio::test]
    // async fn completion_accept_replaces_active_word_with_at_path() {
    //     let mut tab = tab("open @sr").await;
    //     tab.user_input
    //         .0
    //         .set_completion_items(file_completion_items(vec!["src/main.rs".into()]));
    //
    //     tab.completion_next();
    //
    //     assert_eq!(tab.user_input.0.textarea.lines(), ["open @src/main.rs"]);
    // }

    #[tokio::test]
    async fn refresh_reads_tracked_files() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from("tab-refresh".to_string());
        let workdir = project.agent_workdir(&aid);
        std::fs::create_dir_all(&workdir).unwrap();
        let repo = Repository::init(&workdir).unwrap();
        commit_file(&repo, &workdir, "src.rs");

        assert_eq!(tracked_files(&project, &aid).unwrap(), vec!["src.rs"]);
    }
}
