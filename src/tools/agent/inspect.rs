use anyhow::Result;
use anyhow::anyhow;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;

use crate::agent::id::AgentId;
use crate::agent::router::graph::NodeStatus;
use crate::agent::tool::context::ToolRuntimeContext;
use crate::agent::tool::traits::Function;
use crate::declare_tool;
use crate::tui::widgets::container::element::Element;
use crate::tui::widgets::message::toolcall::ToolCallWidget;
use crate::tui::widgets::syntax::HIGHLIGHTER;

declare_tool!(
    name: "inspect",
    description: "Pull another agent's current state: its status plus its working-directory \
        diff. The diff is the expensive half — for the answer text use wait instead.",
    call: InspectCall,
    arguments: InspectArguments,
    meta: (),
    result: InspectResult,
);

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct InspectArguments {
    #[schemars(description = "The target agent's id.")]
    pub id: AgentId,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InspectResult {
    pub status: NodeStatus,
    pub diff: String,
}

#[async_trait::async_trait]
impl Function<(), InspectResult> for InspectArguments {
    async fn call(
        &self,
        ctx: ToolRuntimeContext,
    ) -> Result<(InspectResult, ())> {
        let status = ctx.router.inspect(ctx.agent_id, self.id.clone()).await??;
        // A Spawning target has no state/base yet; serve retryably instead.
        if status == NodeStatus::Spawning {
            return Err(anyhow!(
                "target {} is still spawning; retry inspect once it is running",
                self.id
            ));
        }
        // the diff base is frozen at the target's birth: its own base commit
        // for a primary (cumulative), the minted C_spawn for a subagent — so
        // a racing parent edit can never surface as the target's own change
        // (H3). deliberately narrower than a subagent's own `git diff` (HEAD
        // at the base commit), which includes inherited parent changes
        let base = ctx.project.store().load_state(&self.id).await?.context.base;
        let workdir = ctx.project.agent_workdir(&self.id);
        let excluded = ctx.project.excluded_workdir_paths().to_vec();
        let diff =
            tokio::task::spawn_blocking(move || crate::diff::worktree(&workdir, &base, &excluded))
                .await??;
        Ok((InspectResult { status, diff }, ()))
    }
}

impl From<&InspectCall> for Element {
    fn from(call: &InspectCall) -> Self {
        let text: Option<Text<'_>> = call.output.as_ref().map(|out| match out {
            Ok(res) => {
                let mut text = HIGHLIGHTER.highlight(&res.diff, &HIGHLIGHTER.diff);
                text.lines
                    .insert(0, format!("status: {:?}", res.status).into());
                text
            }
            Err(err) => format!("error: {err}").into(),
        });
        ToolCallWidget {
            name: call
                .arguments
                .as_ref()
                .map_or_else(|| "inspect".into(), |a| format!("inspect: {}", a.id)),
            inner: text.map(Paragraph::new),
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::agent::id::AgentId;
    use crate::project::Project;

    /// spawn a child off a hand-built parent workdir and mint its C_spawn,
    /// exactly like the spawn tail (which the store-less rig bypasses)
    async fn spawn_child(
        project: &Project,
        parent: &AgentId,
        child: &AgentId,
    ) -> String {
        let commit = project.head_commit();
        project
            .duplicate_agent_workdir(parent, child, &commit)
            .await
            .unwrap();
        project
            .mint_spawn_base(child, &commit, &commit)
            .await
            .unwrap()
    }

    /// the inspect diff: the target's workdir vs its frozen base commit
    fn inspect_diff(
        project: &Project,
        target: &AgentId,
        base: &str,
    ) -> String {
        crate::diff::worktree(&project.agent_workdir(target), base, &[]).unwrap()
    }

    /// regression: an agent that cloned a repo into its workdir can still
    /// spawn — the mint pins the embedded repo as a gitlink instead of
    /// erroring — and inspect stays git-shaped: silent while the inherited
    /// pin is untouched, a subproject row once the child moves it
    #[tokio::test]
    async fn spawning_survives_a_nested_repo() {
        let pin = |vendor: &std::path::Path, content: &str, msg: &str, time: i64| {
            let repo = git2::Repository::init(vendor).unwrap();
            fs::write(vendor.join("lib.rs"), content).unwrap();
            let mut idx = repo.index().unwrap();
            idx.add_all(["*"], git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            idx.write().unwrap();
            let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
            let sig = git2::Signature::new("v", "v@v", &git2::Time::new(time, 0)).unwrap();
            let parents: Vec<_> = repo
                .head()
                .ok()
                .and_then(|h| h.peel_to_commit().ok())
                .into_iter()
                .collect();
            let parents: Vec<&git2::Commit> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
                .unwrap()
        };

        let project = Project::new_test().unwrap().0;
        let parent = AgentId::from("parent".to_string());
        let child = AgentId::from("child".to_string());
        let parent_workdir = project.agent_workdir(&parent);
        fs::create_dir_all(&parent_workdir).unwrap();
        fs::write(parent_workdir.join("own.txt"), "work\n").unwrap();
        pin(&parent_workdir.join("vendor"), "one\n", "A", 0);

        let base = spawn_child(&project, &parent, &child).await;

        // the inherited pin is not the child's work
        insta::assert_snapshot!(inspect_diff(&project, &child, &base), @"");

        // the child moves it: exactly that surfaces
        pin(
            &project.agent_workdir(&child).join("vendor"),
            "two\n",
            "B",
            1,
        );
        insta::assert_snapshot!(inspect_diff(&project, &child, &base), @"
        diff --git a/vendor b/vendor
        index ae50680..878909b 160000
        --- a/vendor
        +++ b/vendor
        @@ -1 +1 @@
        -Subproject commit ae50680c1872897cbf0483ba1c7e9317ea9ef748
        +Subproject commit 878909b999abc050218fef773609ea28d8e4dc38
        ");
    }

    /// H3: the diff is anchored to the frozen spawn base — a parent edit
    /// after spawn never leaks in, and only the child's own changes show
    #[tokio::test]
    async fn diff_is_frozen_at_the_spawn_base() {
        let project = Project::new_test().unwrap().0;
        let parent = AgentId::from("parent".to_string());
        let child = AgentId::from("child".to_string());
        let parent_workdir = project.agent_workdir(&parent);
        fs::create_dir_all(&parent_workdir).unwrap();
        fs::write(parent_workdir.join("shared.txt"), "original\n").unwrap();
        fs::write(parent_workdir.join("doomed.txt"), "bye\n").unwrap();

        let base = spawn_child(&project, &parent, &child).await;

        // parent moves on after the spawn: must not appear below
        fs::write(parent_workdir.join("shared.txt"), "parent moved on\n").unwrap();
        fs::write(parent_workdir.join("parent-only.txt"), "not yours\n").unwrap();
        // the child's own work
        let child_workdir = project.agent_workdir(&child);
        fs::write(child_workdir.join("shared.txt"), "original\nchild line\n").unwrap();
        fs::remove_file(child_workdir.join("doomed.txt")).unwrap();
        fs::write(child_workdir.join("new.txt"), "fresh\n").unwrap();

        insta::assert_snapshot!(inspect_diff(&project, &child, &base), @"
        diff --git a/doomed.txt b/doomed.txt
        deleted file mode 100644
        index b023018..0000000
        --- a/doomed.txt
        +++ /dev/null
        @@ -1 +0,0 @@
        -bye
        diff --git a/new.txt b/new.txt
        new file mode 100644
        index 0000000..92d5444
        --- /dev/null
        +++ b/new.txt
        @@ -0,0 +1 @@
        +fresh
        diff --git a/shared.txt b/shared.txt
        index 4b48dee..6f45752 100644
        --- a/shared.txt
        +++ b/shared.txt
        @@ -1 +1,2 @@
         original
        +child line
        ");
    }

    /// a child deleting a file its *parent* created after the base commit —
    /// the file exists in no lower, so on overlay its removal leaves no
    /// whiteout behind. Diffing the workdir rather than reconstructing it
    /// from delta layers makes that unrepresentable: the mount simply
    /// doesn't have the file
    #[tokio::test]
    async fn deleting_an_inherited_file_renders_as_a_deletion() {
        let project = Project::new_test().unwrap().0;
        let parent = AgentId::from("parent".to_string());
        let child = AgentId::from("child".to_string());
        let parent_workdir = project.agent_workdir(&parent);
        fs::create_dir_all(&parent_workdir).unwrap();
        // created by the parent after the base commit: not in any lower
        fs::write(parent_workdir.join("parent-made.txt"), "inherited\n").unwrap();

        let base = spawn_child(&project, &parent, &child).await;
        fs::remove_file(project.agent_workdir(&child).join("parent-made.txt")).unwrap();

        insta::assert_snapshot!(inspect_diff(&project, &child, &base), @"
        diff --git a/parent-made.txt b/parent-made.txt
        deleted file mode 100644
        index 9152fd1..0000000
        --- a/parent-made.txt
        +++ /dev/null
        @@ -1 +0,0 @@
        -inherited
        ");
    }

    /// a moved file pairs its deletion with the addition and renders as a
    /// rename — exact moves as a bare header, edited moves with hunks
    #[tokio::test]
    async fn moved_files_render_as_renames() {
        let project = Project::new_test().unwrap().0;
        let parent = AgentId::from("parent".to_string());
        let child = AgentId::from("child".to_string());
        let parent_workdir = project.agent_workdir(&parent);
        fs::create_dir_all(&parent_workdir).unwrap();
        fs::write(parent_workdir.join("exact.txt"), "alpha\nbeta\ngamma\n").unwrap();
        fs::write(parent_workdir.join("edited.txt"), "one\ntwo\nthree\nfour\n").unwrap();

        let base = spawn_child(&project, &parent, &child).await;

        let child_workdir = project.agent_workdir(&child);
        fs::rename(
            child_workdir.join("exact.txt"),
            child_workdir.join("moved.txt"),
        )
        .unwrap();
        fs::remove_file(child_workdir.join("edited.txt")).unwrap();
        fs::write(child_workdir.join("renamed.txt"), "one\ntwo\n3\nfour\n").unwrap();

        insta::assert_snapshot!(inspect_diff(&project, &child, &base), @"
        diff --git a/exact.txt b/moved.txt
        similarity index 100%
        rename from exact.txt
        rename to moved.txt
        diff --git a/edited.txt b/renamed.txt
        similarity index 68%
        rename from edited.txt
        rename to renamed.txt
        index f384549..dd35c86 100644
        --- a/edited.txt
        +++ b/renamed.txt
        @@ -1,4 +1,4 @@
         one
         two
        -three
        +3
         four
        ");
    }

    /// a chmod with untouched content surfaces as a mode block instead of
    /// comparing equal and vanishing
    #[tokio::test]
    async fn chmod_renders_mode_block() {
        use std::os::unix::fs::PermissionsExt;

        let project = Project::new_test().unwrap().0;
        let parent = AgentId::from("parent".to_string());
        let child = AgentId::from("child".to_string());
        let parent_workdir = project.agent_workdir(&parent);
        fs::create_dir_all(&parent_workdir).unwrap();
        fs::write(parent_workdir.join("tool.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(
            parent_workdir.join("tool.sh"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let base = spawn_child(&project, &parent, &child).await;
        let script = project.agent_workdir(&child).join("tool.sh");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        insta::assert_snapshot!(inspect_diff(&project, &child, &base), @r"
        diff --git a/tool.sh b/tool.sh
        old mode 100644
        new mode 100755
        ");
    }

    /// an untouched child diffs empty even while the parent keeps editing
    #[tokio::test]
    async fn untouched_child_diffs_empty_despite_parent_edits() {
        let project = Project::new_test().unwrap().0;
        let parent = AgentId::from("parent".to_string());
        let child = AgentId::from("child".to_string());
        let parent_workdir = project.agent_workdir(&parent);
        fs::create_dir_all(&parent_workdir).unwrap();
        fs::write(parent_workdir.join("file.txt"), "v1\n").unwrap();

        let base = spawn_child(&project, &parent, &child).await;
        fs::write(parent_workdir.join("file.txt"), "v2\n").unwrap();

        similar_asserts::assert_eq!(inspect_diff(&project, &child, &base), "");
    }
}
