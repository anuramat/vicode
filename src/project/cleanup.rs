use std::collections::HashSet;
use std::fmt;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::Result;
use git2::Repository;

use crate::agent::AgentId;
use crate::config::Config;
use crate::project::Layout;
use crate::project::Project;
use crate::project::backend::BackendKind;
use crate::project::layout::LayoutTrait;
use crate::project::layout::worktree_name_to_agent_id;
use crate::project::lock::ProjectLock;
use crate::project::state::StateStore;

/// stale data eligible for deletion
#[derive(Debug, PartialEq)]
pub struct Garbage {
    /// archived agents (state row not in `visible_order`) with their base commits
    pub agents: Vec<(AgentId, String)>,
    /// agent dirs without a state row
    pub dirs: Vec<PathBuf>,
    /// `vc-*` worktrees without an agent dir
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
            writeln!(f, "agent     {aid}")?;
        }
        for path in &self.dirs {
            writeln!(f, "dir       {}", path.display())?;
        }
        for name in &self.worktrees {
            writeln!(f, "worktree  {name}")?;
        }
        for path in &self.snapshots {
            writeln!(f, "snapshot  {}", path.display())?;
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
    let rows = store.agent_ids()?;

    let mut agents: Vec<(AgentId, String)> = rows
        .iter()
        .filter(|aid| !visible.contains(aid))
        .map(|aid| Ok((aid.clone(), store.agent_commit(aid)?)))
        .collect::<Result<_>>()?;
    agents.sort();

    let mut dirs: Vec<PathBuf> = read_dir_or_empty(layout.agents())?
        .into_iter()
        .filter(|e| !rows.contains(&AgentId::from(e.file_name().to_string_lossy().to_string())))
        .map(|e| e.path())
        .collect();
    dirs.sort();

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

    let mut snapshots = Vec::new();
    if let BackendKind::Overlay(overlay) = backend {
        let keep: HashSet<String> = visible
            .iter()
            .filter(|aid| rows.contains(*aid))
            .map(|aid| store.agent_commit(aid))
            .collect::<Result<_>>()?;
        snapshots = read_dir_or_empty(overlay.snapshots(layout))?
            .into_iter()
            .filter(|e| !keep.contains(&e.file_name().to_string_lossy().to_string()))
            .map(|e| e.path())
            .collect();
    }
    snapshots.sort();

    Ok(Garbage {
        agents,
        dirs,
        worktrees,
        snapshots,
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
        println!("\nrun `vc cleanup --force` to delete");
        return Ok(());
    }

    let project = Project::new(
        config,
        layout,
        lock,
        store.into_handle(),
        Default::default(),
    );
    project.unmount_all().await?;
    for (aid, commit) in &garbage.agents {
        project.delete_agent(aid, commit).await?;
    }
    for dir in &garbage.dirs {
        tokio::fs::remove_dir_all(dir).await?;
    }
    // also catches worktrees whose agent dir was just removed
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
    use crate::agent::AgentContext;
    use crate::agent::AgentState;
    use crate::agent::AgentStatus;
    use crate::llm::history::History;
    use crate::llm::provider::assistant::Assistant;
    use crate::project::backend::Overlay;
    use crate::tui::app::AppState;

    fn state(commit: String) -> AgentState {
        AgentState {
            status: AgentStatus::default(),
            assistant: Assistant::fake().0,
            max_depth: 1,
            context: AgentContext {
                commit,
                history: History::new("".into()),
            },
        }
    }

    #[tokio::test]
    async fn scan_finds_archived_orphans_stale_worktrees_and_snapshots() {
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
        let layout = Layout {
            id: Layout::id(&root),
            root,
            data,
        };
        let store = StateStore::open(layout.state_db()).unwrap();

        // rows: vis is a tab, arch has a dir, ghost doesn't
        let vis = AgentId::from("vis".to_string());
        let arch = AgentId::from("arch".to_string());
        let ghost = AgentId::from("ghost".to_string());
        for aid in [&vis, &arch, &ghost] {
            store.save_agent_sync(aid, &state(commit.clone())).unwrap();
        }
        store
            .save_app_sync(&AppState {
                visible_order: vec![vis.clone()],
            })
            .unwrap();
        for aid in [&vis, &arch] {
            std::fs::create_dir_all(layout.agent(aid)).unwrap();
        }
        let orphan_dir = layout.agent(&AgentId::from("orphan".to_string()));
        std::fs::create_dir_all(&orphan_dir).unwrap();

        // worktree without an agent dir
        repo.worktree("vc-stale", &layout.data().join("stale-wt"), None)
            .unwrap();

        // snapshots: one referenced by vis, one unreferenced
        let backend = BackendKind::Overlay(Overlay {
            sandbox: Config::test().sandbox.clone(),
        });
        let BackendKind::Overlay(overlay) = &backend else {
            unreachable!()
        };
        std::fs::create_dir_all(overlay.snapshot(&layout, &commit)).unwrap();
        let stale_snapshot = overlay.snapshot(&layout, "deadbeef");
        std::fs::create_dir_all(&stale_snapshot).unwrap();

        let garbage = scan(&layout, &backend, &store).unwrap();

        assert_eq!(
            garbage,
            Garbage {
                agents: vec![(arch, commit.clone()), (ghost, commit)],
                dirs: vec![orphan_dir],
                worktrees: vec!["vc-stale".to_string()],
                snapshots: vec![stale_snapshot],
            }
        );
    }
}
