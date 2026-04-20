use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use ignore::gitignore::Gitignore;
use ignore::gitignore::GitignoreBuilder;
use similar::TextDiff;

use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::agent::subagent::SubagentResult;
use crate::project::Project;
use crate::project::layout::LayoutTrait;

pub async fn collect(
    project: &Project,
    parent: &AgentId,
    aid: &AgentId,
) -> Result<SubagentResult> {
    let serialized = tokio::fs::read_to_string(project.agent_state(aid)).await?;
    let state: AgentState = serde_json::from_str(&serialized)?;
    Ok(SubagentResult {
        output: state.context.history.last_output()?,
        diff: diff(project, parent, aid)?,
    })
}

pub fn diff(
    project: &Project,
    parent: &AgentId,
    aid: &AgentId,
) -> Result<String> {
    let parent_upper = project.agent_changes_dir(parent);
    let child_upper = project.agent_changes_dir(aid);
    let parent_workdir = project.agent_workdir(parent);
    let child_workdir = project.agent_workdir(aid);
    let ignore = gitignore(&child_workdir)?;
    let mut paths = BTreeSet::new();
    collect_paths(&parent_upper, &parent_upper, &mut paths)?;
    collect_paths(&child_upper, &child_upper, &mut paths)?;

    let diffs = paths
        .into_iter()
        .filter(|path| !is_ignored(&ignore, &child_workdir, path))
        .map(|path| diff_path(&path, &parent_workdir, &child_workdir))
        .filter_map(Result::transpose)
        .collect::<Result<Vec<_>>>()?;
    Ok(diffs.join("\n"))
}

fn gitignore(root: &Path) -> Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    for path in [root.join(".gitignore"), root.join(".ignore")] {
        if path.exists() {
            builder.add(path);
        }
    }
    Ok(builder.build()?)
}

fn is_ignored(
    ignore: &Gitignore,
    root: &Path,
    rel: &Path,
) -> bool {
    if rel == Path::new(".git") || rel.starts_with(".git/") {
        return true;
    }
    ignore
        .matched_path_or_any_parents(root.join(rel), false)
        .is_ignore()
}

fn collect_paths(
    root: &Path,
    path: &Path,
    out: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_paths(root, &path, out)?;
        } else {
            out.insert(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn diff_path(
    rel: &Path,
    parent_workdir: &Path,
    child_workdir: &Path,
) -> Result<Option<String>> {
    let old = read_state(&parent_workdir.join(rel))?;
    let new = read_state(&child_workdir.join(rel))?;
    if old == new {
        return Ok(None);
    }
    Ok(Some(render_diff(rel, old, new)))
}

fn render_diff(
    rel: &Path,
    old: FileState,
    new: FileState,
) -> String {
    let rel = rel.to_string_lossy();
    let a = format!("a/{rel}");
    let b = format!("b/{rel}");
    match (old, new) {
        (FileState::Text(old), FileState::Text(new)) => TextDiff::from_lines(&old, &new)
            .unified_diff()
            .header(&a, &b)
            .to_string(),
        (FileState::Missing, FileState::Text(new)) => TextDiff::from_lines(&String::new(), &new)
            .unified_diff()
            .header("/dev/null", &b)
            .to_string(),
        (FileState::Text(old), FileState::Missing) => TextDiff::from_lines(&old, &String::new())
            .unified_diff()
            .header(&a, "/dev/null")
            .to_string(),
        (old, new) => format!("diff --git {a} {b}\n{}\n", status(old, new)),
    }
}

fn status(
    old: FileState,
    new: FileState,
) -> &'static str {
    match (old, new) {
        (FileState::Missing, _) => "Binary or non-text file added",
        (_, FileState::Missing) => "Binary or non-text file deleted",
        _ => "Binary or non-text file changed",
    }
}

#[derive(Debug, PartialEq, Eq)]
enum FileState {
    Missing,
    Text(String),
    Binary,
}

fn read_state(path: &Path) -> Result<FileState> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(FileState::Missing),
        Err(err) => return Err(err.into()),
    };
    #[cfg(unix)]
    if meta.file_type().is_char_device() {
        return Ok(FileState::Missing);
    }
    if !meta.is_file() {
        return Ok(FileState::Binary);
    }
    let bytes = fs::read(path)?;
    match String::from_utf8(bytes) {
        Ok(text) => Ok(FileState::Text(text)),
        Err(_) => Ok(FileState::Binary),
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;
    use crate::agent::AgentContext;
    use crate::agent::AgentStatus;
    use crate::agent::AgentTopology;
    use crate::config::Config;
    use crate::llm::history::History;
    use crate::llm::message::AssistantItem;
    use crate::llm::message::AssistantMessage;
    use crate::llm::message::AssistantMessageStatus;
    use crate::llm::message::OutputContent;
    use crate::llm::message::OutputItem;
    use crate::llm::message::now_ms;
    use crate::llm::provider::assistant::ASSISTANT_POOL;
    use crate::llm::provider::assistant::Assistant;
    use crate::llm::provider::assistant::AssistantPool;

    fn config() -> Config {
        Config::parse_with_defaults(
            r#"
            primary_assistant = ["test"]
            shell_cmd = ["bash", "-c"]

            [sandbox]
            kind = "bwrap"
            bin = "bwrap"
            args = []
            stages = []

            [keymap.cmdline]

            [keymap.normal]

            [keymap.insert]

            [providers.main]
            api = "responses"
            base_url = "https://api.example.com/v1"

            [assistants.test]
            provider = "main"
            model = "gpt-test"
            "#,
        )
        .unwrap()
    }

    async fn assistant() -> Assistant {
        ASSISTANT_POOL
            .get_or_init(|| async { AssistantPool::from_config(&config()).await.unwrap() })
            .await
            .assistant("test")
            .unwrap()
    }

    #[tokio::test]
    async fn collect_reads_output_and_diff() {
        let project = Project::new_test().unwrap();
        let parent = AgentId::from("parent".to_string());
        let child = AgentId::from("child".to_string());

        tokio::fs::create_dir_all(project.agent_changes_dir(&parent))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.agent_changes_dir(&child))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.agent_workdir(&parent))
            .await
            .unwrap();
        tokio::fs::create_dir_all(project.agent_workdir(&child))
            .await
            .unwrap();

        tokio::fs::write(project.agent_workdir(&parent).join("file.txt"), "before\n")
            .await
            .unwrap();
        tokio::fs::write(project.agent_workdir(&child).join("file.txt"), "after\n")
            .await
            .unwrap();
        tokio::fs::write(project.agent_changes_dir(&parent).join("file.txt"), "")
            .await
            .unwrap();
        tokio::fs::write(project.agent_changes_dir(&child).join("file.txt"), "")
            .await
            .unwrap();

        let mut history = History::with_instructions("rules".to_string());
        history.push_message(crate::llm::message::Message::Assistant(AssistantMessage {
            finish_reason: AssistantMessageStatus::Success,
            content: indexmap::indexmap! {
                "output".to_string() => AssistantItem::Output(OutputItem {
                    id: "output".to_string(),
                    timing: crate::llm::message::ItemTiming::with_start(now_ms()),
                    content: vec![OutputContent::Text("done".to_string())],
                })
            },
        }));
        history.recount_tokens();

        tokio::fs::create_dir_all(project.agent(&child))
            .await
            .unwrap();
        AgentState {
            status: AgentStatus::Idle,
            assistant: assistant().await,
            topology: AgentTopology::default(),
            context: AgentContext {
                commit: "deadbeef".to_string(),
                history,
            },
        }
        .save(&project, &child)
        .await
        .unwrap();

        let output = collect(&project, &parent, &child).await.unwrap();

        assert_eq!(output.output, "done");
        assert!(output.diff.contains("--- a/file.txt"));
        assert!(output.diff.contains("+++ b/file.txt"));
        assert!(output.diff.contains("-before"));
        assert!(output.diff.contains("+after"));
    }
}
