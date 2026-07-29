//! `spawn`: a child created through [`Router::create`], its workdir checked
//! out at the requested commit -- the node joins the graph only once it's
//! set up, so nobody ever sees a spawn in progress

use anyhow::Result;

use crate::agent::AgentId;
use crate::agent::AgentState;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::TAB_AGENT_CAP;
use crate::agent::router::graph::Runtime;
use crate::llm::history::History;

impl Router {
    /// resolves once the child's {state, workdir at `commit`, graph record}
    /// are durable, its runtime started, and `prompt` sent to it
    pub async fn spawn_agent(
        &self,
        parent: &AgentId,
        commit: &str,
        inherited_history: Option<History>,
        prompt: &str,
    ) -> Result<AgentId> {
        // fail fast; `start` checks again at commit
        let aid = {
            let s = &mut *self.lock();
            s.child_root(parent)?;
            s.allocate()
        };
        let project = self.project.clone();
        let (parent_id, child) = (parent.clone(), aid.clone());
        let commit = commit.to_string();
        let setup = async move {
            let parent_state = project.store().load_state(&parent_id).await?;
            // the whole tab shares one snapshot; the child starts at `commit`
            let snapshot = parent_state.commit;
            project
                .spawn_agent_workdir(&child, &snapshot, &commit)
                .await?;
            let assistant_id = project
                .assistants()
                .subagent(&parent_state.assistant_id)?
                .id;
            // a fresh child reads its instructions from its own tree
            let history = match inherited_history {
                Some(history) => history,
                None => History::new_subagent(project.instructions(&child).await?),
            };
            Ok(AgentState {
                assistant_id,
                commit: snapshot,
                history,
                pending_messages: Vec::new(),
            })
        };
        self.create(aid.clone(), Some(parent.clone()), setup)
            .await?;
        // the seed is ordinary mail: startup never wakes an agent, delivery does
        self.send_message(parent, &aid, prompt)?;
        Ok(aid)
    }
}

impl RouterState {
    /// the root a new child of `parent` joins, if its tab has room
    pub fn child_root(
        &self,
        parent: &AgentId,
    ) -> Result<AgentId> {
        let root = match self.graph.get(parent) {
            Some(node) if !matches!(node.runtime, Runtime::Dead(_)) => node.root.clone(),
            Some(_) => anyhow::bail!("agent {parent} is dead"),
            None => anyhow::bail!("unknown agent {parent}"),
        };
        // per-tab count cap; recovery must not depend on
        // remembering ids — the error spells out the list → archive loop
        anyhow::ensure!(
            self.graph.values().filter(|n| n.root == root).count() < TAB_AGENT_CAP,
            "this tab is at its agent cap ({TAB_AGENT_CAP}): archive finished agents to \
             free slots (list shows every member and its status)"
        );
        Ok(root)
    }
}
