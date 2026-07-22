use std::path::PathBuf;

use super::Overlay;
use crate::agent::id::AgentId;
use crate::project::Paths;

const OVERLAY_DIRNAME: &str = ".overlay";
const OVERLAY_UPPER_DIRNAME: &str = "upper";
const OVERLAY_WORKDIR_DIRNAME: &str = "workdir";
const SNAPSHOTS_DIRNAME: &str = "snapshots";
const SHARED_DIRNAME: &str = "shared";

impl Overlay {
    pub fn overlay(
        &self,
        paths: &Paths,
        aid: &AgentId,
    ) -> PathBuf {
        paths.agent(aid).join(OVERLAY_DIRNAME)
    }

    pub fn overlay_workdir(
        &self,
        paths: &Paths,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay(paths, aid).join(OVERLAY_WORKDIR_DIRNAME)
    }

    pub fn overlay_upper(
        &self,
        paths: &Paths,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay(paths, aid).join(OVERLAY_UPPER_DIRNAME)
    }

    pub fn shared(
        &self,
        paths: &Paths,
    ) -> PathBuf {
        paths.data.join(SHARED_DIRNAME)
    }

    pub fn snapshots(
        &self,
        paths: &Paths,
    ) -> PathBuf {
        paths.data.join(SNAPSHOTS_DIRNAME)
    }

    pub fn snapshot(
        &self,
        paths: &Paths,
        commit: &str,
    ) -> PathBuf {
        self.snapshots(paths).join(commit)
    }
}
