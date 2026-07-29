//! `archive`: subtree teardown (strict spawn-descendants only) and the
//! whole-tab form used on tab close

use anyhow::Result;
use tokio::sync::oneshot;

use crate::agent::AgentId;
use crate::agent::router::AgentRouter;
use crate::agent::router::api::RouterError;
use crate::agent::router::graph::GraphRecord;

impl AgentRouter {
    pub fn handle_archive(
        &mut self,
        caller: AgentId,
        target: AgentId,
        done: oneshot::Sender<Result<(), RouterError>>,
    ) {
        if self.same_tab(&caller, &target).is_none() {
            drop(done.send(Err(RouterError::Unreachable)));
            return;
        }
        // ownership: strict spawn-descendant only
        if target == caller || !self.descends(&target, &caller) {
            drop(done.send(Err(RouterError::NotOwned)));
            return;
        }
        let members: Vec<AgentId> = self
            .graph
            .keys()
            .filter(|id| self.descends(id, &target))
            .cloned()
            .collect();
        self.archive_members(members, move |_| drop(done.send(Ok(()))));
    }

    pub fn handle_archive_tab(
        &mut self,
        primary: AgentId,
        done: oneshot::Sender<Result<()>>,
    ) {
        if !self.graph.contains_key(&primary) {
            drop(done.send(Err(anyhow::anyhow!("unknown agent {primary}"))));
            return;
        }
        let members: Vec<AgentId> = self
            .graph
            .iter()
            .filter(|(_, n)| n.root == primary)
            .map(|(id, _)| id.clone())
            .collect();
        self.archive_members(members, move |_| drop(done.send(Ok(()))));
    }

    /// synchronous teardown (remove from graph, abort runtimes, fire waiters
    /// `Unreachable`, enqueue the archived flips in-loop as one redb
    /// transaction — all-or-nothing, so a crash can't leave a live member
    /// under an archived root, H5), then a detached tail awaits the commit
    /// and unmounts before acking — `Ok` means durably archived and resources
    /// released
    fn archive_members(
        &mut self,
        members: Vec<AgentId>,
        ack: impl FnOnce(()) + Send + 'static,
    ) {
        let flips: Vec<(AgentId, GraphRecord)> = members
            .iter()
            .filter_map(|aid| {
                let node = self.graph.remove(aid)?;
                if let Some(abort) = &node.abort {
                    abort.abort();
                }
                self.fire_waiters(aid, Err(RouterError::Unreachable));
                Some((aid.clone(), node.record(true)))
            })
            .collect();
        let write = self.project.store().save_graph_batch(&flips);
        let project = self.project.clone();
        tokio::spawn(async move {
            let durable = write
                .await
                .inspect_err(|e| tracing::error!("archive record flip failed: {e:#}"))
                .is_ok();
            for aid in &members {
                if let Err(e) = project.unmount_agent(aid).await {
                    tracing::warn!("unmount {aid} failed: {e}");
                }
            }
            // ack only a durable archive: a failed flip left the graph records
            // unarchived, so the subtree resurrects at next boot — drop `done`
            // un-acked and let the caller's channel close into an error (M6)
            if durable {
                ack(());
            }
        });
    }
}
