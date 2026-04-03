// SLOP
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
use crate::project::Project;
use crate::project::layout::LayoutTrait;

pub async fn response(
    project: &Project,
    parent: &AgentId,
    aid: &AgentId,
) -> Result<String> {
    let serialized = tokio::fs::read_to_string(project.agent_state(aid)).await?;
    let state: AgentState = serde_json::from_str(&serialized)?;
    let text = state.context.history.last_output()?;
    let diff = diff(project, parent, aid)?;
    Ok(format!(
        "<implementation id={}>\n{}\n```diff\n{}```\n</implementation>",
        aid, text, diff
    ))
}

fn diff(
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
    Ok(Some(render_diff(rel, old, new)?))
}

fn render_diff(
    rel: &Path,
    old: FileState,
    new: FileState,
) -> Result<String> {
    let rel = rel.to_string_lossy();
    let a = format!("a/{rel}");
    let b = format!("b/{rel}");
    match (old, new) {
        (FileState::Text(old), FileState::Text(new)) => Ok(TextDiff::from_lines(&old, &new)
            .unified_diff()
            .header(&a, &b)
            .to_string()),
        (FileState::Missing, FileState::Text(new)) => {
            Ok(TextDiff::from_lines(&String::new(), &new)
                .unified_diff()
                .header("/dev/null", &b)
                .to_string())
        }
        (FileState::Text(old), FileState::Missing) => {
            Ok(TextDiff::from_lines(&old, &String::new())
                .unified_diff()
                .header(&a, "/dev/null")
                .to_string())
        }
        (old, new) => Ok(format!("diff --git {a} {b}\n{}\n", status(old, new))),
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
