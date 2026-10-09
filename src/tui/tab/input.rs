use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use crossterm::event::KeyEvent;
use git2::Repository;

use crate::agent::event::UserCommand;
use crate::agent::event::UserPrompt;
use crate::llm::history::message::Message;
use crate::tui::tab::Tab;
use crate::tui::widgets::input::CompletionItem;
use crate::tui::widgets::tab::input::MessageInput;

fn file_completion_items(paths: Vec<String>) -> Vec<CompletionItem> {
    paths
        .into_iter()
        .map(|x| CompletionItem::new(format!("@{x}")))
        .collect()
}

pub fn tracked_files(workdir: &Path) -> Result<Vec<String>> {
    let repo = Repository::open(workdir)?;
    let index = repo.index()?;
    index
        .iter()
        .map(|e| Ok(String::from_utf8_lossy(&e.path).to_string()))
        .collect()
}

impl Tab<'_> {
    pub fn cycle_assistant(
        &self,
        prev: bool,
    ) -> Result<()> {
        let id = self
            .project
            .assistants()
            .switch_assistant(&self.state.assistant_id, prev)
            .with_context(|| "couldn't find the provided assistant id")?;
        self.send(UserCommand::SetAssistant(id))
    }

    // TODO clean up if trimmed is empty
    pub fn insert_mode(
        &mut self,
        active: bool,
    ) {
        self.input.set_focus(active);
        self.update_input_title();
    }

    pub fn set_file_completion(
        &mut self,
        paths: Vec<String>,
    ) -> Result<()> {
        self.input
            .completion
            .source_mut()
            .set_items('@', file_completion_items(paths))?;
        Ok(())
    }

    // TODO update on completions, ideally make a mut getter or something
    pub fn update_input_title(&mut self) {
        let title = format!(" {} T ", self.input.count_tokens());
        self.input = MessageInput {
            title,
            ..self.input.clone()
        }
    }

    pub fn submit(&mut self) -> Result<()> {
        let editor_text = self.input.take_area().lines().join("\n");
        let text = editor_text.trim().to_string();
        self.input.set_focus(false);
        if text.is_empty() {
            return Ok(());
        }
        let prompt = UserPrompt {
            text,
            generation: self.history().generation(),
        };

        let result = self.send(UserCommand::Submit(prompt));
        if result.is_err() {
            self.input.textarea.insert_str(&editor_text);
            self.update_input_title();
        }
        result
    }

    pub fn retry(&self) -> Result<()> {
        self.send(UserCommand::Retry)
    }

    pub fn compact(
        &self,
        n: Option<&str>,
    ) -> Result<()> {
        let n = if let Some(n) = n {
            n.parse()
                .with_context(|| format!("invalid compact number: {n}"))?
        } else {
            self.history().state().len()
        };
        self.send(UserCommand::Compact(n))
    }

    pub fn abort(&self) -> Result<()> {
        self.send(UserCommand::Abort)
    }

    pub fn undo(
        &self,
        n: usize,
    ) -> Result<()> {
        anyhow::ensure!(
            n <= self.history().state().len(),
            "cannot undo {n} messages, history is shorter"
        );
        self.send(UserCommand::Undo(n))
    }

    pub fn undo_user(&self) -> Result<()> {
        let messages = self.history().state();
        let Some(loc) = messages
            .iter()
            .rposition(|entry| matches!(entry, Message::User(_)))
        else {
            return Ok(());
        };
        let n = messages.len() - loc;
        self.undo(n)
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
    use crate::agent::event::AgentEvent;
    use crate::agent::id::AgentId;
    use crate::project::Project;
    use crate::tui::widgets::input::InputOpts;

    fn tab() -> Tab<'static> {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("tab-input".to_string());
        Repository::init(project.agent_workdir(&aid)).unwrap();
        let state = AgentState::fake();
        let mut tab = Tab::new(
            Some(tokio::sync::mpsc::unbounded_channel().0),
            aid,
            state,
            &project,
        );
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
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("tab-refresh".to_string());
        let workdir = project.agent_workdir(&aid);
        std::fs::create_dir_all(&workdir).unwrap();
        let repo = Repository::init(&workdir).unwrap();
        commit_file(&repo, &workdir, "src.rs");

        assert_eq!(tracked_files(&workdir).unwrap(), vec!["src.rs"]);
    }

    #[tokio::test]
    async fn cycle_assistant_sends_switch() {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("cycle".to_string());
        let (control, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let tab = Tab::new(Some(control), aid, AgentState::fake(), &project);

        tab.cycle_assistant(false).unwrap();
        assert!(matches!(
            events_rx.try_recv(),
            Ok(AgentEvent::User(UserCommand::SetAssistant(id))) if id == "test2"
        ));
    }

    #[tokio::test]
    async fn preview_submit_keeps_input() {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("preview-submit".to_string());
        let mut tab = Tab::new(None, aid, AgentState::fake(), &project);
        tab.input.textarea.insert_str("  do work  ");

        drop(tab.submit());

        assert_eq!(tab.input.textarea.lines(), ["  do work  "]);
    }

    #[tokio::test]
    async fn rejected_submit_restores_input() {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("rejected-submit".to_string());
        // the agent is gone: its event channel is closed
        let control = tokio::sync::mpsc::unbounded_channel().0;
        let mut tab = Tab::new(Some(control), aid, AgentState::fake(), &project);
        tab.input.textarea.insert_str("  do work  ");

        assert!(tab.submit().is_err());

        assert_eq!(tab.input.textarea.lines(), ["  do work  "]);
    }
}
