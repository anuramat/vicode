use std::collections::HashSet;
use std::fmt;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::Result;
use git2::Repository;

use crate::agent::AgentId;
use crate::config::Config;
use crate::project::Paths;
use crate::project::Workspace;
use crate::project::backend::Backend;
use crate::project::lock::ProjectLock;
use crate::project::paths::worktree_name_to_agent_id;
use crate::project::store::Store;

/// stale data eligible for deletion. A well-formed member — a state record
/// plus a live graph record under a live tab, or any archived graph record
/// awaiting memory extraction — is never garbage: its history must survive
/// cleanup (§2.6)
#[derive(Debug, PartialEq, Eq)]
pub struct Garbage {
    /// state records outside the keep-set (half-committed spawn residue,
    /// records predating the graph table)
    pub agents: Vec<AgentId>,
    /// dangling graph records (no state record): crashed-spawn residue
    pub records: Vec<AgentId>,
    /// orphan agent dirs
    pub dirs: Vec<PathBuf>,
    /// prunable worktrees
    pub worktrees: Vec<String>,
    /// `vc-*` branches with no state record: leaks from crash windows and
    /// old bugs
    pub branches: Vec<String>,
    /// snapshots not referenced by any kept agent
    pub snapshots: Vec<PathBuf>,
    /// spawn-base refs whose agent has no state record (archived agents keep
    /// theirs — their extraction diff still needs `tree(base)`)
    pub base_refs: Vec<String>,
}

impl Garbage {
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
            && self.records.is_empty()
            && self.dirs.is_empty()
            && self.worktrees.is_empty()
            && self.branches.is_empty()
            && self.snapshots.is_empty()
            && self.base_refs.is_empty()
    }
}

impl fmt::Display for Garbage {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        for aid in &self.agents {
            writeln!(f, "agent {aid}")?;
        }
        for aid in &self.records {
            writeln!(f, "record {aid}")?;
        }
        for path in &self.dirs {
            writeln!(f, "dir {}", path.display())?;
        }
        for name in &self.worktrees {
            writeln!(f, "worktree {name}")?;
        }
        for name in &self.branches {
            writeln!(f, "branch {name}")?;
        }
        for path in &self.snapshots {
            writeln!(f, "snapshot {}", path.display())?;
        }
        for name in &self.base_refs {
            writeln!(f, "base ref {name}")?;
        }
        Ok(())
    }
}

pub fn scan(
    paths: &Paths,
    backend: &Backend,
    store: &Store,
) -> Result<Garbage> {
    let state_ids = store.state_ids()?;
    let records = store.load_graph()?;

    // keep every live member of a live tab and every archived agent
    // (extraction hasn't shipped, so all await it); an agent is well-formed
    // iff it has both a graph and a state record (§2.1)
    let alive_primary = |aid: &AgentId| {
        records
            .get(aid)
            .is_some_and(|r| !r.archived && r.parent.is_none())
    };
    let keep: HashSet<&AgentId> = records
        .iter()
        .filter(|(aid, record)| {
            state_ids.contains(aid) && (record.archived || alive_primary(&record.root))
        })
        .map(|(aid, _)| aid)
        .collect();

    let stale_rows: Vec<AgentId> = state_ids
        .iter()
        .filter(|aid| !keep.contains(aid))
        .cloned()
        .collect();

    let dangling_records: Vec<AgentId> = records
        .keys()
        .filter(|aid| !state_ids.contains(aid))
        .cloned()
        .collect();

    let orphan_agent_dirs = {
        let mut dirs: Vec<PathBuf> = read_dir_or_empty(paths.agents())?
            .into_iter()
            .filter(|e| {
                !state_ids.contains(&AgentId::from(e.file_name().to_string_lossy().to_string()))
            })
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs
    };

    // get prunable worktrees
    let prunable_worktrees = {
        let repo = Repository::open(paths.root())?;
        let mut worktrees = Vec::new();
        for name in repo.worktrees()?.iter().flatten() {
            let Some(aid) = worktree_name_to_agent_id(name) else {
                continue;
            };
            if !paths.agent(&aid).exists() {
                worktrees.push(name.to_string());
            }
        }
        worktrees.sort();
        worktrees
    };

    let orphan_branches = {
        let repo = Repository::open(paths.root())?;
        let mut branches = Vec::new();
        for branch in repo.branches(Some(git2::BranchType::Local))? {
            let Some(name) = branch?.0.name()?.map(str::to_string) else {
                continue;
            };
            let Some(aid) = worktree_name_to_agent_id(&name) else {
                continue;
            };
            if !state_ids.contains(&aid) {
                branches.push(name);
            }
        }
        branches.sort();
        branches
    };

    let unused_snapshots = {
        let mut snapshots = if let Backend::Overlay(overlay) = backend {
            // an archived agent's snapshot is needed to remount it for its
            // extraction diff — the diff reads the composed workdir
            let commits: HashSet<String> = keep
                .iter()
                .map(|aid| store.load_state(aid).map(|s| s.context.commit))
                .collect::<Result<_>>()?;
            read_dir_or_empty(overlay.snapshots(paths))?
                .into_iter()
                .filter(|e| !commits.contains(&e.file_name().to_string_lossy().to_string()))
                .map(|e| e.path())
                .collect()
        } else {
            Vec::new()
        };
        snapshots.sort();
        snapshots
    };

    let orphan_base_refs = {
        let repo = Repository::open(paths.root())?;
        let mut refs = Vec::new();
        for reference in repo.references_glob("refs/vicode/base/*")? {
            let Some(name) = reference?.name().map(str::to_string) else {
                continue;
            };
            let Some(aid) = crate::project::paths::base_ref_to_agent_id(&name) else {
                continue;
            };
            if !state_ids.contains(&aid) {
                refs.push(name);
            }
        }
        refs.sort();
        refs
    };

    Ok(Garbage {
        agents: stale_rows,
        records: dangling_records,
        dirs: orphan_agent_dirs,
        worktrees: prunable_worktrees,
        branches: orphan_branches,
        snapshots: unused_snapshots,
        base_refs: orphan_base_refs,
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
    let paths = Paths::new()?;
    let lock = ProjectLock::acquire(&paths)?;
    let store = Store::open(paths.state_db())?;
    let backend = Backend::from_config(&config);

    let garbage = scan(&paths, &backend, &store)?;
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

    let ws = Workspace::new(paths, backend, lock, store.into_handle());
    ws.unmount_all().await?;
    // a stale agent's graph record (e.g. a live member of a dead tab) goes
    // with it, else it would resurface as a dangling graph record
    for aid in &garbage.agents {
        ws.delete_agent(aid).await?;
    }
    for aid in &garbage.records {
        ws.store().delete_graph(aid).await?;
    }
    for dir in &garbage.dirs {
        tokio::fs::remove_dir_all(dir).await?;
    }
    crate::git::prune_stale_worktrees(&ws)?;
    for snapshot in &garbage.snapshots {
        tokio::fs::remove_dir_all(snapshot).await?;
    }
    let repo = Repository::open(ws.root())?;
    // after the dir removal and worktree prune above — a branch checked out
    // in a live worktree refuses deletion
    for name in &garbage.branches {
        crate::git::delete_branch_if_exists(&repo, name)?;
    }
    for name in &garbage.base_refs {
        crate::git::delete_ref_if_exists(&repo, name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::router::graph::GraphRecord;
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
            id: Paths::derive_id(&root),
            root,
            data,
        };
        let store = Store::open(paths.state_db()).unwrap();

        let aid = |s: &str| AgentId::from(s.to_string());
        let record = |root: &str, parent: Option<&str>, archived| GraphRecord {
            root: aid(root),
            parent: parent.map(aid),
            archived,
        };
        // vis = live tab; sub = its member; arch = archived (awaits memory
        // extraction); ghost = state predating the graph table; lostsub =
        // state whose root has no live primary; dangling = graph record
        // without state
        for name in ["vis", "sub", "arch", "ghost", "lostsub"] {
            let state = AgentState::new("test".into(), commit.clone(), "".into());
            store.save_state_sync(&aid(name), &state).unwrap();
        }
        for (name, rec) in [
            ("vis", record("vis", None, false)),
            ("sub", record("vis", Some("vis"), false)),
            ("arch", record("arch", None, true)),
            ("lostsub", record("gone", Some("gone"), false)),
            ("dangling", record("vis", Some("vis"), false)),
        ] {
            store.save_graph_sync(&aid(name), &rec).unwrap();
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
        let backend = Backend::Overlay(Overlay::test());
        let Backend::Overlay(overlay) = &backend else {
            unreachable!()
        };
        std::fs::create_dir_all(overlay.snapshot(&paths, &commit)).unwrap();
        let stale_snapshot = overlay.snapshot(&paths, "deadbeef");
        std::fs::create_dir_all(&stale_snapshot).unwrap();

        // base refs: kept agents keep theirs, an orphan ref is garbage
        let oid = git2::Oid::from_str(&commit).unwrap();
        repo.reference("refs/vicode/base/sub", oid, false, "test")
            .unwrap();
        repo.reference("refs/vicode/base/orphanref", oid, false, "test")
            .unwrap();

        // branches: an agent with state keeps its branch; `vc-stale` (created
        // by the worktree above, no state record) is garbage
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
