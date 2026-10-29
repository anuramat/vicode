use anyhow::Result;

use crate::llm::history::HistoryUpdate;
use crate::llm::history::Splice;
use crate::llm::history::message::Message;
use crate::llm::history::message::UserMessage;
use crate::tui::osc7::set_osc7;
use crate::tui::tab::Tab;

impl Tab<'_> {
    pub fn set_osc7(&self) {
        let path = self.project.agent_workdir(&self.aid);
        set_osc7(&path);
    }

    pub fn update(
        &mut self,
        event: HistoryUpdate,
    ) -> Result<()> {
        let input = if let HistoryUpdate::Pop(n) = &event {
            Some(self.combined_user_msgs(*n))
        } else {
            None
        };
        let splices = self.history_mut().handle(event)?;
        if let Some(input) = input {
            self.input.prepend_text(input);
            self.update_input_title();
        }
        self.mirror(&splices)
    }

    /// apply the history's splices to the scroll cache; splices that don't
    /// fit it are a bug, so the cache is rebuilt instead of rendering a
    /// corrupt view, and the bug is reported
    fn mirror(
        &mut self,
        splices: &[Splice],
    ) -> Result<()> {
        // in bounds, disjoint and descending
        let valid = splices
            .first()
            .is_none_or(|s| s.range.end <= self.scroll.len())
            && splices
                .windows(2)
                .all(|w| w[1].range.end <= w[0].range.start);
        if !valid {
            return self.resync(splices);
        }
        for Splice { range, len } in splices.iter().cloned() {
            self.scroll.splice(range, len);
        }
        if self.scroll.len() != self.state.history.state().messages.len() {
            return self.resync(splices);
        }
        Ok(())
    }

    fn resync(
        &mut self,
        splices: &[Splice],
    ) -> Result<()> {
        self.reset_scroll();
        anyhow::bail!("history view went out of sync and was rebuilt; splices: {splices:?}")
    }

    pub fn combined_user_msgs(
        &self,
        popped: usize,
    ) -> String {
        // NOTE we only apply the results if history event was successfully handled, so we don't have to check it here
        let mut result = Vec::new();
        let messages = &self.state.history.state().messages;
        let start = messages.len().saturating_sub(popped);
        for msg in &messages[start..] {
            if let Message::User(UserMessage { text, .. }) = msg {
                result.push(text.clone());
            }
        }
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::id::AgentId;
    use crate::project::Project;
    use crate::tui::tab::TabRuntime;

    fn user(text: &str) -> HistoryUpdate {
        HistoryUpdate::UserMessage(UserMessage::new(text.into(), 0))
    }

    /// a tab whose history holds `n` user messages, mirrored by its cache
    fn tab(n: usize) -> Tab<'static> {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("tab-mirror".to_string());
        let mut tab = Tab::new(TabRuntime::Pending, aid, AgentState::fake(), &project);
        for i in 0..n {
            tab.update(user(&i.to_string())).unwrap();
        }
        tab
    }

    /// splices that don't fit the cache are reported, and the cache is
    /// rebuilt to mirror the history -- without panicking on bad bounds
    #[test]
    fn mirror_rebuilds_on_bad_splices() {
        let mut errors = Vec::new();
        for splices in [
            // ascending
            vec![Splice::edit(0), Splice::edit(1)],
            // past the end of the cache
            vec![Splice::edit(5)],
            // overlapping
            vec![Splice::edit(1), Splice { range: 0..2, len: 2 }],
        ] {
            let mut tab = tab(2);
            errors.push(tab.mirror(&splices).unwrap_err().to_string());
            assert_eq!(tab.scroll.len(), 2);
        }
        // an unreported change
        let mut tab = tab(2);
        tab.history_mut().handle(user("unreported")).unwrap();
        errors.push(tab.mirror(&[]).unwrap_err().to_string());
        assert_eq!(tab.scroll.len(), 3);

        insta::assert_snapshot!(errors.join("\n"), @"
        history view went out of sync and was rebuilt; splices: [Splice { range: 0..1, len: 1 }, Splice { range: 1..2, len: 1 }]
        history view went out of sync and was rebuilt; splices: [Splice { range: 5..6, len: 1 }]
        history view went out of sync and was rebuilt; splices: [Splice { range: 1..2, len: 1 }, Splice { range: 0..2, len: 2 }]
        history view went out of sync and was rebuilt; splices: []
        ");
    }

    /// valid splices keep the cache mirroring the history
    #[test]
    fn update_mirrors_history() {
        let mut tab = tab(3);
        assert_eq!(tab.scroll.len(), 3);
        tab.update(HistoryUpdate::Pop(2)).unwrap();
        assert_eq!(tab.scroll.len(), 1);
    }
}
