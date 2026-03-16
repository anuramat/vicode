use std::io::ErrorKind;
use std::path::Path;

use anyhow::Result;

use super::Project;
use crate::config::CONFIG;
use crate::config::INSTRUCTIONS;

impl Project {
    pub async fn instructions_by_commit(
        &self,
        commit: &str,
    ) -> Result<String> {
        self.instructions(&self.snapshot(commit)).await
    }

    async fn instructions(
        &self,
        root: &Path,
    ) -> Result<String> {
        let mut collected = INSTRUCTIONS.clone();
        for name in &CONFIG.context_files {
            match tokio::fs::read_to_string(root.join(name)).await {
                Ok(text) => collected.push_str(&text),
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Ok(collected)
    }
}
