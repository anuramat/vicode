use std::collections::HashSet;
use std::fmt;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::Result;
use git2::Repository;

use crate::agent::AgentId;
use crate::agent::router::graph::RecordState;
use crate::config::Config;
use crate::project::Paths;
use crate::project::Storage;
use crate::project::StorageTrait;
use crate::project::backend::BackendKind;
use crate::project::lock::ProjectLock;
use crate::project::paths::PathsTrait;
use crate::project::paths::worktree_name_to_agent_id;
use crate::project::state::StateStore;

/// stale data eligible for deletion
#[derive(Debug, PartialEq, Eq)]
pub struct Garbage {
    /// archived agents (i.e. not in `visible_order`) with their base commits
    pub agents: Vec<(AgentId, String)>,
    /// orphan agent dirs
    pub dirs: Vec<PathBuf>,
    /// prunable worktrees
    pub worktrees: Vec<String>,
    /// snapshots not referenced by any visible agent
    pub snapshots: Vec<PathBuf>,
}

impl Garbage {
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
            && self.dirs.is_empty()
            && self.worktrees.is_empty()
            && self.snapshots.is_empty()
    }
}

impl fmt::Display for Garbage {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        for (aid, _) in &self.agents {
            writeln!(f, "agent {aid}")?;
        }
        for path in &self.dirs {
            writeln!(f, "dir {}", path.display())?;
        }
        for name in &self.worktrees {
            writeln!(f, "worktree {name}")?;
        }
        for path in &self.snapshots {
            writeln!(f, "snapshot {}", path.display())?;
        }
        Ok(())
    }
}

pub fn scan(
    layout: &Layout,
    backend: &BackendKind,
    store: &StateStore,
) -> Result<Garbage> {
    let visible: HashSet<AgentId> = store.load_app()?.visible_order.into_iter().collect();
    let agents = store.agent_ids()?;

    let archived_agents = {
        let mut agents: Vec<(AgentId, String)> = agents
            .iter()
            .filter(|aid| !visible.contains(aid))
            .map(|aid| Ok((aid.clone(), store.agent_commit(aid)?)))
            .collect::<Result<_>>()?;
        agents.sort();
        agents
    };

    let orphan_agent_dirs = {
        let mut dirs: Vec<PathBuf> = read_dir_or_empty(layout.agents())?
            .into_iter()
            .filter(|e| {
                !agents.contains(&AgentId::from(e.file_name().to_string_lossy().to_string()))
            })
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs
    };

    // get prunable worktrees
    let prunable_worktrees = {
        let repo = Repository::open(layout.root())?;
        let mut worktrees = Vec::new();
        for name in repo.worktrees()?.iter().flatten() {
            let Some(aid) = worktree_name_to_agent_id(name) else {
                continue;
            };
            if !layout.agent(&aid).exists() {
                worktrees.push(name.to_string());
            }
        }
        worktrees.sort();
        worktrees
    };

    let unused_snapshots = {
        let mut snapshots = if let BackendKind::Overlay(overlay) = backend {
            let keep: HashSet<String> = visible
                .iter()
                .filter(|aid| agents.contains(*aid))
                .map(|aid| store.agent_commit(aid))
                .collect::<Result<_>>()?;
            read_dir_or_empty(overlay.snapshots(layout))?
                .into_iter()
                .filter(|e| !keep.contains(&e.file_name().to_string_lossy().to_string()))
                .map(|e| e.path())
                .collect()
        } else {
            Vec::new()
        };
        snapshots.sort();
        snapshots
    };

    Ok(Garbage {
        agents: archived_agents,
        dirs: orphan_agent_dirs,
        worktrees: prunable_worktrees,
        snapshots: unused_snapshots,
    })
}

fn read_dir_or_empty(path: PathBuf) -> Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        entries => Ok(entries?.collect::<std::io::Result<_>>()?),
    }
}

pub async fn run(force: bool) -> Result<()> {
    let config = Config::load()?;
    let layout = Layout::discover()?;
    let lock = ProjectLock::acquire(&layout)?;
    let store = StateStore::open(layout.state_db())?;
    let backend = BackendKind::from_config(&config);

    let garbage = scan(&layout, &backend, &store)?;
    if garbage.is_empty() {
        println!("nothing to clean");
        return Ok(());
    }
    print!("{garbage}");
    if !force {
        // TODO when we implement memory extraction, on extraction we should mark agents as safe to delete without -f, then delete them here
        println!("-f not given: refusing to clean");
        return Ok(());
    }

    let project = Project::new(config, layout, lock, store.into_handle());
    project.unmount_all().await?;
    for (aid, commit) in &garbage.agents {
        project.delete_agent(aid, commit).await?;
    }
    for dir in &garbage.dirs {
        tokio::fs::remove_dir_all(dir).await?;
    }
    crate::git::prune_stale_worktrees(&project)?;
    for snapshot in &garbage.snapshots {
        tokio::fs::remove_dir_all(snapshot).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::router::graph::AgentRecord;
    use crate::project::backend::Overlay;

    #[tokio::test]
    async fn scan_keeps_members_and_archived_reaps_residue() {
        let root = std::env::temp_dir().join(format!("vicode-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let repo = Repository::init(&root).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("vicode", "vicode@example.com").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap()
            .to_string();
        let data = root.join(".vicode");
        std::fs::create_dir_all(&data).unwrap();
        let paths = Paths {
            id: Paths::id(&root),
            root,
            data,
        };
        let store = StateStore::open(paths.state_db()).unwrap();

        let aid = |s: &str| AgentId::from(s.to_string());
        let record = |root: &str, parent: Option<&str>, state| AgentRecord {
            root: aid(root),
            parent: parent.map(aid),
            state,
        };
        // vis = live tab; sub = its member; arch = archived (awaits memory
        // extraction); ghost = row predating the graph store; lostsub = row
        // whose root has no live primary; dangling = record without a row
        for name in ["vis", "sub", "arch", "ghost", "lostsub"] {
            let state = AgentState::new("test".into(), commit.clone(), "".into());
            store.save_agent_sync(&aid(name), &state).unwrap();
        }
        for (name, rec) in [
            ("vis", record("vis", None, RecordState::Alive)),
            ("sub", record("vis", Some("vis"), RecordState::Alive)),
            ("arch", record("arch", None, RecordState::Archived)),
            ("lostsub", record("gone", Some("gone"), RecordState::Alive)),
            ("dangling", record("vis", Some("vis"), RecordState::Alive)),
        ] {
            store.save_record_sync(&aid(name), &rec).unwrap();
        }
        for name in ["vis", "sub", "arch"] {
            std::fs::create_dir_all(paths.agent(&aid(name))).unwrap();
        }
        let orphan_dir = paths.agent(&aid("orphan"));
        std::fs::create_dir_all(&orphan_dir).unwrap();

        // worktree without an agent dir
        repo.worktree("vc-stale", &paths.data().join("stale-wt"), None)
            .unwrap();

        // snapshots: one referenced by kept agents, one unreferenced
        let backend = BackendKind::Overlay(Overlay::test());
        let BackendKind::Overlay(overlay) = &backend else {
            unreachable!()
        };
        std::fs::create_dir_all(overlay.snapshot(&paths, &commit)).unwrap();
        let stale_snapshot = overlay.snapshot(&paths, "deadbeef");
        std::fs::create_dir_all(&stale_snapshot).unwrap();

        // base refs: kept agents (rows) keep theirs, an orphan ref is garbage
        let oid = git2::Oid::from_str(&commit).unwrap();
        repo.reference("refs/vicode/base/sub", oid, false, "test")
            .unwrap();
        repo.reference("refs/vicode/base/orphanref", oid, false, "test")
            .unwrap();

        // branches: a rowed agent keeps its branch; `vc-stale` (created by
        // the worktree above, no row) is garbage
        repo.branch("vc-sub", &repo.find_commit(oid).unwrap(), false)
            .unwrap();

        let garbage = scan(&paths, &backend, &store).unwrap();

        assert_eq!(
            garbage,
            Garbage {
                agents: vec![aid("ghost"), aid("lostsub")],
                records: vec![aid("dangling")],
                dirs: vec![orphan_dir],
                worktrees: vec!["vc-stale".to_string()],
                branches: vec!["vc-stale".to_string()],
                snapshots: vec![stale_snapshot],
                base_refs: vec!["refs/vicode/base/orphanref".to_string()],
            }
        );
    }
}
