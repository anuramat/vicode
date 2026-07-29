//! `archive`: subtree teardown (strict spawn-descendants only) and the
//! whole-tab form used on tab close

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::agent::AgentId;
use crate::agent::router::Router;
use crate::agent::router::RouterState;
use crate::agent::router::api::RouterError;
use crate::agent::router::graph::AgentNode;
use crate::agent::router::graph::GraphRecord;
use crate::agent::router::graph::Runtime;

impl Router {
    /// resolves once the subtree's graph records are durably archived and its
    /// mounts released
    pub async fn archive(
        &self,
        caller: &AgentId,
        target: &AgentId,
    ) -> Result<Result<(), RouterError>> {
        let durable = {
            let s = &mut *self.lock();
            if s.same_tab(caller, target).is_none() {
                return Ok(Err(RouterError::Unreachable));
            }
            // ownership: strict spawn-descendant only
            if target == caller || !s.descends(target, caller) {
                return Ok(Err(RouterError::NotOwned));
            }
            let members: Vec<AgentId> = s
                .graph
                .keys()
                .filter(|id| s.descends(id, target))
                .cloned()
                .collect();
            s.archive_members(members)
        };
        durable.await??;
        Ok(Ok(()))
    }

    pub async fn archive_tab(
        &self,
        primary: &AgentId,
    ) -> Result<()> {
        let durable = {
            let s = &mut *self.lock();
            anyhow::ensure!(s.graph.contains_key(primary), "unknown agent {primary}");
            let members: Vec<AgentId> = s
                .graph
                .iter()
                .filter(|(_, n)| &n.root == primary)
                .map(|(id, _)| id.clone())
                .collect();
            s.archive_members(members)
        };
        durable.await?
    }
}

impl RouterState {
    /// remove a node from the live graph, aborting its runtime
    pub fn drop_node(
        &mut self,
        aid: &AgentId,
    ) -> Option<AgentNode> {
        let node = self.graph.remove(aid)?;
        if let Runtime::Live { abort, .. } = &node.runtime {
            abort.abort();
        }
        Some(node)
    }

    /// synchronous teardown (remove from graph, abort runtimes, enqueue the
    /// archived flips as one redb transaction — all-or-nothing, so a crash
    /// can't leave a live member under an archived root), then a detached
    /// tail awaits the commit and unmounts: `Ok` means durably archived and
    /// resources released; a failed flip left the graph records unarchived,
    /// so the subtree resurrects at next boot
    fn archive_members(
        &mut self,
        members: Vec<AgentId>,
    ) -> JoinHandle<Result<()>> {
        let flips: Vec<(AgentId, GraphRecord)> = members
            .iter()
            .filter_map(|aid| Some((aid.clone(), self.drop_node(aid)?.record(true))))
            .collect();
        let write = self.project.store().save_graph_batch(&flips);
        let project = self.project.clone();
        tokio::spawn(async move {
            let durable = write
                .await
                .inspect_err(|e| tracing::error!("archive record flip failed: {e:#}"));
            for aid in &members {
                if let Err(e) = project.unmount_agent(aid).await {
                    tracing::warn!("unmount {aid} failed: {e}");
                }
            }
            durable
        })
    }
}
