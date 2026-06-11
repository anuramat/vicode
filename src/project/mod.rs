pub mod backend;
pub mod cleanup;
pub mod layout;
pub mod lock;
pub mod state;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use ambassador::Delegate;
use anyhow::Context;
use anyhow::Result;
use git2::Repository;

use crate::agent::AgentId;
use crate::config::Config;
use crate::config::DIRS;
use crate::config::INSTRUCTIONS;
use crate::llm::provider::assistant::AssistantPool;
use crate::project::backend::BackendKind;
use crate::project::backend::WorkspaceBackend;
use crate::project::layout::LayoutTrait;
use crate::project::layout::ambassador_impl_LayoutTrait;
use crate::project::lock::ProjectLock;
use crate::project::state::StateStoreHandle;
use crate::sandbox::SandboxRunner;

#[derive(Clone, Delegate, Debug)]
#[delegate(LayoutTrait, target = "layout")]
pub struct Project {
    layout: Layout,
    backend: BackendKind,
    config: Config,
    _lock: ProjectLock,
    store: StateStoreHandle,
    assistants: Arc<AssistantPool>,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
    /// path-based unique identifier for the project
    pub id: String,
    /// per-project data directory
    pub data: PathBuf,
}

impl Layout {
    /// discover the project layout from the current working directory
    pub fn discover() -> Result<Self> {
        // TODO discover vs open? normalize across codebase
        let repo = Repository::discover(".")?;
        let root = repo
            .workdir()
            .context("cannot run inside a bare repository")?
            .to_path_buf();
        let id = Self::id(&root);
        let data = DIRS.create_data_directory(&id)?;
        Ok(Self { root, id, data })
    }

    fn id(root: &Path) -> String {
        let name_prefix = root
            .file_name()
            .map(|name| format!("{}_", name.to_string_lossy()))
            .unwrap_or_default();
        let uuid = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            root.to_string_lossy().as_bytes(),
        )
        .to_string();
        format!("{name_prefix}{uuid}")
    }
}

impl Project {
    /// assemble a project from its already-acquired lock and started state writer
    pub fn new(
        config: Config,
        layout: Layout,
        lock: ProjectLock,
        store: StateStoreHandle,
        assistants: Arc<AssistantPool>,
    ) -> Self {
        let backend = BackendKind::from_config(&config);
        Self {
            layout,
            backend,
            config,
            _lock: lock,
            store,
            assistants,
        }
    }

    pub fn name(&self) -> String {
        self.layout
            .root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn store(&self) -> &StateStoreHandle {
        &self.store
    }

    pub fn assistants(&self) -> &AssistantPool {
        &self.assistants
    }

    pub async fn mount_agent(
        &self,
        commit: &str,
        aid: &AgentId,
    ) -> Result<()> {
        self.backend.mount_agent(&self.layout, commit, aid).await
    }

    pub async fn init(&self) -> Result<()> {
        self.backend.init(&self.layout, self.config()).await
    }

    pub async fn unmount_all(&self) -> Result<()> {
        self.backend.unmount_all(&self.layout).await
    }

    pub fn agent_diff_root(
        &self,
        aid: &AgentId,
    ) -> PathBuf {
        self.backend.agent_diff_root(&self.layout, aid)
    }

    pub fn sandbox_runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner {
        self.backend.sandbox_runner(cwd, gitdir)
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

    pub async fn duplicate_agent_workdir(
        &self,
        src_aid: &AgentId,
        dst_aid: &AgentId,
        commit: &str,
        git: bool,
    ) -> Result<()> {
        self.backend
            .duplicate_agent_workdir(&self.layout, src_aid, dst_aid, commit, git)
            .await
    }

    pub async fn unmount_agent(
        &self,
        aid: &AgentId,
    ) -> Result<()> {
        self.backend.unmount_agent(&self.layout, aid).await
    }

    pub async fn delete_agent(
        &self,
        aid: &AgentId,
        commit: &str,
    ) -> Result<()> {
        if self.agent(aid).exists() {
            self.unmount_agent(aid).await?;
            tokio::fs::remove_dir_all(self.agent(aid)).await?;
        }
        let repo = Repository::open(self.root())?;
        let name = self.worktree_name(aid);
        crate::git::prune_worktree(&repo, &name)?;
        crate::git::delete_branch_if_at(&repo, &name, commit)?;
        self.store.delete_agent(aid).await?;
        Ok(())
    }

    pub async fn new_agent_workdir(
        &self,
        commit: &str,
        aid: &AgentId,
        git: bool,
    ) -> Result<()> {
        self.backend
            .new_agent_workdir(&self.layout, commit, aid, git)
            .await
    }
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
    use crate::project::lock::ProjectLock;

    impl Project {
        pub fn new_test() -> Result<Self> {
            use crate::project::backend::Cow;

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
            let layout = Layout {
                id: Layout::id(&root),
                root,
                data,
            };
            // tests shouldn't depend on fuse-overlayfs availability
            let backend = BackendKind::Cow(Cow {
                sandbox: config.sandbox.clone(),
            });
            let _lock = ProjectLock::acquire(&layout)?;
            let store = crate::project::state::StateStore::open(layout.state_db())?.into_handle();
            Ok(Self {
                layout,
                backend,
                config,
                _lock,
                store,
                assistants: Arc::new(AssistantPool::fake().0),
            })
        }
    }

    fn agent_state(commit: String) -> AgentState {
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

    fn head_commit(project: &Project) -> String {
        Repository::open(project.root())
            .unwrap()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string()
    }

    #[tokio::test]
    async fn delete_agent_removes_workdir() {
        let project = Project::new_test().unwrap();
        let aid = AgentId::from("delete-me".to_string());
        let commit = head_commit(&project);

        project
            .new_agent_workdir(&commit, &aid, true)
            .await
            .unwrap();
        project
            .store()
            .save_agent(&aid, &agent_state(commit.clone()))
            .await
            .unwrap();

        assert!(project.agent(&aid).exists());

        project.delete_agent(&aid, &commit).await.unwrap();

        assert!(!project.agent(&aid).exists());
        assert_eq!(head_commit(&project), commit);
    }

    #[test]
    fn project_holds_lock_until_dropped() {
        let project = Project::new_test().unwrap();
        let layout = Layout {
            root: project.root().to_path_buf(),
            id: project.id().into(),
            data: project.data().to_path_buf(),
        };

        let err = ProjectLock::acquire(&layout).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "vicode is already running in {} (PID: {})",
                project.id(),
                std::process::id(),
            )
        );

        drop(project);
        let _lock = ProjectLock::acquire(&layout).unwrap();
    }
}
