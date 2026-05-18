pub mod backend;
pub mod layout;

use std::path::Path;
use std::path::PathBuf;

use ambassador::Delegate;
use anyhow::Context;
use anyhow::Result;
use git2::Repository;

use crate::agent::AgentId;
use crate::config::Config;
use crate::config::DIRS;
use crate::config::INSTRUCTIONS;
use crate::project::backend::BackendKind;
use crate::project::backend::WorkspaceBackend;
use crate::project::layout::LayoutTrait;
use crate::project::layout::ambassador_impl_LayoutTrait;
use crate::sandbox::SandboxRunner;

#[derive(Clone, Delegate, Debug)]
#[delegate(LayoutTrait, target = "layout")]
pub struct Project {
    layout: Layout,
    backend: BackendKind,
    config: Config,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
    /// path-based unique identifier for the project
    pub id: String,
    /// per-project data directory
    pub data: PathBuf,
}

impl Project {
    pub fn name(&self) -> String {
        self.layout
            .root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
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

    pub fn new(config: Config) -> Result<Self> {
        // TODO discover vs open? normalize across codebase
        let repo = Repository::discover(".")?;
        let root = repo
            .workdir()
            .context("cannot run inside a bare repository")?
            .to_path_buf();
        let id = Self::id(&root);
        let data = DIRS.create_data_directory(&id)?;
        let backend = BackendKind::from_config(&config);
        Ok(Self {
            layout: Layout { root, id, data },
            backend,
            config,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    #[cfg(test)]
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
        // tests shouldn't depend on fuse-overlayfs availability
        let backend = BackendKind::Cow(Cow {
            sandbox: config.sandbox.clone(),
        });
        Ok(Self {
            layout: Layout {
                id: Self::id(&root),
                root,
                data,
            },
            backend,
            config,
        })
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
        self.unmount_agent(aid).await?;
        tokio::fs::remove_dir_all(self.agent(aid)).await?;
        let repo = Repository::open(self.root())?;
        let name = self.worktree_name(aid);
        crate::git::prune_worktree(&repo, &name)?;
        crate::git::delete_branch_if_at(&repo, &name, commit)?;
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
