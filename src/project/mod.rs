pub mod backend;
pub mod cleanup;
pub mod lock;
pub mod paths;
pub mod store;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use derive_getters::Getters;
use derive_more::Deref;
use git2::Repository;
pub use paths::Paths;

use crate::agent::AgentId;
use crate::config::Config;
use crate::config::INSTRUCTIONS;
use crate::llm::provider::assistant::AssistantPool;
use crate::project::backend::Backend;
use crate::project::backend::BackendOps;
use crate::project::lock::ProjectLock;
use crate::project::store::StoreHandle;
use crate::sandbox::SandboxRunner;

#[derive(Clone, Debug, Deref, Getters)]
pub struct Workspace {
    #[deref]
    paths: Paths,
    backend: Backend,
    _lock: ProjectLock,
    store: StoreHandle,
}

#[derive(Clone, Debug, Deref, Getters)]
pub struct Project {
    #[deref]
    workspace: Workspace,
    config: Config,
    assistants: Arc<AssistantPool>,
}

impl Workspace {
    pub fn new(
        paths: Paths,
        backend: Backend,
        lock: ProjectLock,
        store: StoreHandle,
    ) -> Self {
        Self {
            paths,
            backend,
            _lock: lock,
            store,
        }
    }

    pub fn excluded_workdir_paths(&self) -> &[String] {
        self.backend.excluded_workdir_paths()
    }

    pub async fn mount_agent(
        &self,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        self.backend.mount_agent(&self.paths, commit, aid).await
    }

    pub async fn unmount_all(&self) -> Result<()> {
        self.backend.unmount_all(&self.paths).await
    }

    pub fn pin_base(
        &self,
        aid: &AgentId,
        commit: &str,
    ) -> Result<()> {
        let repo = Repository::open(self.root())?;
        repo.reference(
            &self.base_ref(aid),
            git2::Oid::from_str(commit)?,
            true,
            "inspect base",
        )?;
        Ok(())
    }

    pub async fn mint_spawn_base(
        &self,
        dst: &AgentId,
        base: &str,
        commit: &str,
    ) -> Result<String> {
        self.mount_agent(commit, dst).await?;
        let this = self.clone();
        let (dst, base) = (dst.clone(), base.to_string());
        tokio::task::spawn_blocking(move || {
            let tree =
                crate::git::workdir_tree(&this.agent_workdir(&dst), this.excluded_workdir_paths())?;
            let repo = Repository::open(this.root())?;
            let tree = repo.find_tree(tree)?;
            let parent = repo.find_commit(git2::Oid::from_str(&base)?)?;
            let sig = git2::Signature::new("vicode", "vicode", &git2::Time::new(0, 0))?;
            let oid = repo.commit(None, &sig, &sig, "spawn base", &tree, &[&parent])?;
            let oid = oid.to_string();
            this.pin_base(&dst, &oid)?;
            Ok(oid)
        })
        .await?
    }

    pub fn sandbox_runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner {
        self.backend.sandbox_runner(cwd, gitdir)
    }

    pub async fn duplicate_agent_workdir(
        &self,
        src_aid: &AgentId,
        dst_aid: &AgentId,
        commit: &str,
    ) -> Result<()> {
        self.backend
            .duplicate_agent_workdir(&self.paths, src_aid, dst_aid, commit)
            .await
    }

    pub async fn unmount_agent(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        self.backend.unmount_agent(&self.paths, aid).await
    }

    pub async fn delete_agent_workdir(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        if self.agent(aid).exists() {
            self.unmount_agent(aid).await?;
            tokio::fs::remove_dir_all(self.agent(aid)).await?;
        }
        let repo = Repository::open(self.root())?;
        let name = self.worktree_name(aid);
        crate::git::prune_worktree(&repo, &name)?;
        crate::git::delete_branch_if_exists(&repo, &name)?;
        crate::git::delete_ref_if_exists(&repo, &self.base_ref(aid))?;
        Ok(())
    }

    pub async fn delete_agent(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        self.delete_agent_workdir(aid).await?;
        self.store.delete_agent(aid).await
    }

    pub async fn new_agent_workdir(
        &self,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        self.backend
            .new_agent_workdir(&self.paths, commit, aid)
            .await
    }
}

impl Project {
    pub fn new(
        config: Config,
        paths: Paths,
        lock: ProjectLock,
        store: StoreHandle,
        assistants: Arc<AssistantPool>,
    ) -> Self {
        let backend = Backend::from_config(&config);
        Self {
            workspace: Workspace::new(paths, backend, lock, store),
            config,
            assistants,
        }
    }

    pub fn name(&self) -> String {
        self.root()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    }

    pub async fn init(&self) -> Result<()> {
        self.workspace.backend.init(&self.workspace.paths).await
    }

    pub async fn instructions(
        &self,
        aid: &AgentId,
    ) -> Result<String> {
        use std::io::ErrorKind;
        let mut collected = INSTRUCTIONS.clone();
        let root = self.agent(aid);
        for name in &self.config.context_files {
            match tokio::fs::read_to_string(root.join(name)).await {
                Ok(text) => collected.push_str(&text),
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(collected)
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentState;
    use crate::agent::router::graph::GraphRecord;
    use crate::project::lock::ProjectLock;

    impl Project {
        /// fresh project on a temp git repo; the returned handle scripts the
        /// fake pool's api. Cow-backed: tests generally shouldn't depend on
        /// fuse-overlayfs availability
        pub fn new_test() -> Result<(Self, Arc<crate::llm::provider::api::fake::FakeApi>)> {
            let config = Config::test();
            Self::new_test_backend(Backend::Cow(backend::Cow {
                sandbox: config.sandbox.clone(),
            }))
        }

        /// like `new_test` but on the real Overlay backend — for tests that
        /// actually mount (gate on fuse availability before using)
        pub fn new_test_overlay() -> Result<(Self, Arc<crate::llm::provider::api::fake::FakeApi>)> {
            Self::new_test_backend(Backend::Overlay(backend::Overlay::test()))
        }

        fn new_test_backend(
            backend: Backend
        ) -> Result<(Self, Arc<crate::llm::provider::api::fake::FakeApi>)> {
            let config = Config::test();
            let root = std::env::temp_dir().join(format!("vicode-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root)?;
            let repo = Repository::init(&root)?;
            let tree_id = repo.index()?.write_tree()?;
            let tree = repo.find_tree(tree_id)?;
            let signature = git2::Signature::now("vicode", "vicode@example.com")?;
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])?;
            let data = root.join(".vicode");
            std::fs::create_dir_all(&data)?;
            let paths = Paths {
                id: Paths::derive_id(&root),
                root,
                data,
            };
            let lock = ProjectLock::acquire(&paths)?;
            let store = crate::project::store::Store::open(paths.state_db())?.into_handle();
            let (pool, api) = AssistantPool::fake();
            Ok((
                Self {
                    workspace: Workspace::new(paths, backend, lock, store),
                    config,
                    assistants: Arc::new(pool),
                },
                api,
            ))
        }

        pub fn head_commit(&self) -> String {
            Repository::open(self.root())
                .unwrap()
                .head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .id()
                .to_string()
        }

        /// fake state anchored to the test repo's HEAD — worktree creation
        /// needs a real commit
        pub fn fake_state(&self) -> AgentState {
            let mut state = AgentState::fake();
            state.context.commit = self.head_commit();
            state.context.base = state.context.commit.clone();
            state
        }
    }

    #[tokio::test]
    async fn delete_agent_removes_workdir_git_objects_state_and_graph() {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("delete-me".to_string());
        let commit = project.head_commit();

        project.new_agent_workdir(&commit, &aid).await.unwrap();
        project
            .store()
            .save_state(&aid, &AgentState::fake())
            .await
            .unwrap();
        let record = GraphRecord {
            root: aid.clone(),
            parent: None,
            archived: false,
        };
        project.store().save_graph(&aid, &record).await.unwrap();
        let repo = Repository::open(project.root()).unwrap();
        repo.reference(
            &project.base_ref(&aid),
            git2::Oid::from_str(&commit).unwrap(),
            false,
            "test",
        )
        .unwrap();
        // the agent committed: the branch moved off its base — deleted anyway
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let moved = repo
            .commit(
                None,
                &sig,
                &sig,
                "agent work",
                &head.tree().unwrap(),
                &[&head],
            )
            .unwrap();
        repo.find_reference(&format!("refs/heads/{}", project.worktree_name(&aid)))
            .unwrap()
            .set_target(moved, "test")
            .unwrap();

        assert!(project.agent(&aid).exists());

        project.delete_agent(&aid).await.unwrap();

        assert!(!project.agent(&aid).exists());
        assert!(repo.find_reference(&project.base_ref(&aid)).is_err());
        assert!(
            repo.find_branch(&project.worktree_name(&aid), git2::BranchType::Local)
                .is_err()
        );
        assert_eq!(project.head_commit(), commit);
        assert!(project.store().load_state(&aid).await.is_err());
        assert!(
            !project
                .store()
                .load_graph()
                .await
                .unwrap()
                .contains_key(&aid)
        );
    }

    /// a failed spawn's rollback leaves no git residue: workdir, worktree
    /// registration, branch and base ref all gone
    #[tokio::test]
    async fn delete_agent_workdir_leaves_no_git_residue() {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("workdir-me".to_string());
        let commit = project.head_commit();

        project.new_agent_workdir(&commit, &aid).await.unwrap();
        let repo = Repository::open(project.root()).unwrap();
        repo.reference(
            &project.base_ref(&aid),
            git2::Oid::from_str(&commit).unwrap(),
            false,
            "test",
        )
        .unwrap();

        project.delete_agent_workdir(&aid).await.unwrap();

        assert!(!project.agent(&aid).exists());
        assert!(repo.find_worktree(&project.worktree_name(&aid)).is_err());
        assert!(
            repo.find_branch(&project.worktree_name(&aid), git2::BranchType::Local)
                .is_err()
        );
        assert!(repo.find_reference(&project.base_ref(&aid)).is_err());
    }

    /// `vc-*` is a reserved namespace: a colliding branch is crash residue
    /// and spawn reclaims it by force instead of failing
    #[tokio::test]
    async fn spawn_reclaims_residue_branch() {
        let project = Project::new_test().unwrap().0;
        let aid = AgentId::from("residue".to_string());
        let commit = project.head_commit();
        let repo = Repository::open(project.root()).unwrap();
        // residue pointing off the spawn target
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let stale = repo
            .commit(None, &sig, &sig, "residue", &head.tree().unwrap(), &[&head])
            .unwrap();
        repo.branch(
            &project.worktree_name(&aid),
            &repo.find_commit(stale).unwrap(),
            false,
        )
        .unwrap();

        project.new_agent_workdir(&commit, &aid).await.unwrap();

        let branch = repo
            .find_branch(&project.worktree_name(&aid), git2::BranchType::Local)
            .unwrap();
        assert_eq!(branch.get().target().unwrap().to_string(), commit);
    }

    #[test]
    fn project_holds_lock_until_dropped() {
        let project = Project::new_test().unwrap().0;
        let paths = Paths {
            root: project.root().to_path_buf(),
            id: project.id().into(),
            data: project.data().to_path_buf(),
        };

        let err = ProjectLock::acquire(&paths).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "vicode is already running in {} (PID: {})",
                project.id(),
                std::process::id(),
            )
        );

        drop(project);
        // flock releases on close, but a subprocess forked by a concurrent
        // test briefly inherits the fd until its exec: poll out the window
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let _lock = loop {
            match ProjectLock::acquire(&paths) {
                Ok(lock) => break lock,
                Err(e) if std::time::Instant::now() >= deadline => {
                    panic!("lock never released: {e}")
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        };
    }
}
