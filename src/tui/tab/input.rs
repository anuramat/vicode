use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use crossterm::event::KeyEvent;
use git2::Repository;

use crate::agent::handle::ExternalEvent;
use crate::agent::handle::UserPrompt;
use crate::llm::history::message::Message;
use crate::project::layout::LayoutTrait;
use crate::tui::tab::Tab;
use crate::tui::widgets::input::CompletionItem;
use crate::tui::widgets::tab::input::MessageInput;

fn file_completion_items(paths: Vec<String>) -> Vec<CompletionItem> {
    paths
        .into_iter()
        .map(|x| CompletionItem::new(format!("@{x}")))
        .collect()
}

fn tracked_files(workdir: &Path) -> Result<Vec<String>> {
    let repo = Repository::open(workdir)?;
    let index = repo.index()?;
    index
        .iter()
        .map(|e| Ok(String::from_utf8_lossy(&e.path).to_string()))
        .collect()
}

impl Tab<'_> {
    pub async fn cycle_assistant(
        &self,
        prev: bool,
    ) -> Result<()> {
        let router = self.router()?;
        if !self.state.status.idle() {
            return Ok(());
        }
        let id = self
            .project
            .assistants()
            .switch_assistant(&self.state.assistant.id, prev)
            .with_context(|| "couldn't find the provided assistant id")?;
        router
            .forward(self.aid.clone(), ExternalEvent::SetAssistant(id))
            .await?;
        Ok(())
    }

    // TODO clean up if trimmed is empty
    pub fn insert_mode(
        &mut self,
        active: bool,
    ) {
        self.input.set_focus(active);
        self.update_input_title();
    }

    pub fn refresh_file_completion(&mut self) -> Result<()> {
        let workdir = self.project.agent_workdir(&self.aid);
        let paths = tracked_files(&workdir)?;
        self.input
            .completion
            .source_mut()
            .set_items('@', file_completion_items(paths))?;
        Ok(())
    }

    // TODO update on completions, ideally make a mut getter or something
    pub fn update_input_title(&mut self) {
        let title = {
            let tokens = self.input.count_tokens();
            if self.multiplier > 1 {
                format!(" x{} | {} T ", self.multiplier, tokens)
            } else {
                format!(" {tokens} T ")
            }
        };
        self.input = MessageInput {
            title,
            ..self.input.clone()
        }
    }

    pub async fn submit(&mut self) -> Result<()> {
        self.router()?;
        let text = self.input.take_area().lines().join("\n").trim().to_string();
        self.input.set_focus(false);
        if text.is_empty() {
            return Ok(());
        }
        let prompt = UserPrompt {
            text,
            multiplier: self.multiplier,
            generation: self.history().generation(),
        };

        self.router()?
            .forward(self.aid.clone(), ExternalEvent::Submit(prompt, None))
            .await
    }

    pub async fn retry(&self) -> Result<()> {
        self.router()?
            .forward(self.aid.clone(), ExternalEvent::Retry)
            .await
    }

    pub async fn compact(
        &self,
        n: Option<&str>,
    ) -> Result<()> {
        self.router()?;
        let n = if let Some(n) = n {
            n.parse()
                .with_context(|| format!("invalid compact number: {n}"))?
        } else {
            self.history().state().len()
        };
        self.router()?
            .forward(self.aid.clone(), ExternalEvent::Compact(n))
            .await
    }

    pub async fn abort(&self) -> Result<()> {
        self.router()?
            .forward(self.aid.clone(), ExternalEvent::Abort)
            .await
    }

    pub async fn undo(
        &self,
        n: usize,
    ) -> Result<()> {
        anyhow::ensure!(
            n <= self.history().state().len(),
            "cannot undo {n} messages, history is shorter"
        );
        self.router()?
            .forward(self.aid.clone(), ExternalEvent::Undo(n))
            .await
    }

    pub async fn undo_user(&self) -> Result<()> {
        let messages = self.history().state();
        let Some(loc) = messages
            .iter()
            .rposition(|entry| matches!(entry, Message::User(_)))
        else {
            return Ok(());
        };
        let n = messages.len() - loc;
        self.undo(n).await
    }

    pub fn key_insert(
        &mut self,
        input: KeyEvent,
    ) {
        self.input.handle(input);
        self.update_input_title();
    }

    pub fn paste(
        &mut self,
        content: &str,
    ) {
        self.input.textarea.insert_str(content);
        self.update_input_title();
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyModifiers;
    use git2::Repository;
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::AgentStatus;
    use crate::agent::id::AgentId;
    use crate::agent::router::AgentRouter;
    use crate::llm::history::History;
    use crate::llm::provider::assistant::Assistant;
    use crate::project::Project;
    use crate::project::layout::LayoutTrait;
    use crate::tui::widgets::input::InputOpts;

    fn tab() -> Tab<'static> {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from("tab-input".to_string());
        Repository::init(project.agent_workdir(&aid)).unwrap();
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: Assistant::fake().0,
            max_depth: 1,
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("".into()),
            },
        };
        let mut tab = Tab::new(Some(AgentRouter::test_handle()), aid, state, &project);
        tab.input.input = crate::tui::widgets::input::Input::new(InputOpts {
            source: crate::tui::widgets::input::CompletionSource::Freeform(vec![(
                '@',
                file_completion_items(vec!["src/main.rs".into()]),
            )]),
            height: 5,
            clear_on_unfocus: false,
        });
        tab
    }

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

    #[tokio::test]
    async fn completion_accept_replaces_active_word_with_at_path() {
        let mut tab = tab();
        tab.insert_mode(true);
        for ch in "open @sr".chars() {
            tab.key_insert(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }

        tab.input.completion_next();

        assert_eq!(tab.input.textarea.lines(), ["open @src/main.rs"]);
    }

    #[tokio::test]
    async fn refresh_reads_tracked_files() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from("tab-refresh".to_string());
        let workdir = project.agent_workdir(&aid);
        std::fs::create_dir_all(&workdir).unwrap();
        let repo = Repository::init(&workdir).unwrap();
        commit_file(&repo, &workdir, "src.rs");

        assert_eq!(tracked_files(&workdir).unwrap(), vec!["src.rs"]);
    }

    #[tokio::test]
    async fn preview_submit_keeps_input() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from("preview-submit".to_string());
        let state = AgentState {
            status: AgentStatus::default(),
            assistant: Assistant::fake().0,
            max_depth: 1,
            context: crate::agent::AgentContext {
                commit: "".into(),
                history: History::new("".into()),
            },
        };
        let mut tab = Tab::new(None, aid, state, &project);
        tab.input.textarea.insert_str("do work");

        drop(tab.submit().await);

        assert_eq!(tab.input.textarea.lines(), ["do work"]);
    }
}
