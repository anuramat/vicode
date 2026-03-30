use std::path::PathBuf;

use super::Overlay;
use crate::agent::id::AgentId;
use crate::project::Layout;
use crate::project::layout::LayoutTrait;

const OVERLAY_DIRNAME: &str = ".overlay";
const OVERLAY_UPPER_DIRNAME: &str = "upper";
const OVERLAY_WORKDIR_DIRNAME: &str = "workdir";
const SNAPSHOTS_DIRNAME: &str = "snapshots";
const SHARED_DIRNAME: &str = "shared";

impl Overlay {
    pub fn overlay(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        layout.agent(aid).join(OVERLAY_DIRNAME)
    }

    pub fn overlay_workdir(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay(layout, aid).join(OVERLAY_WORKDIR_DIRNAME)
    }

    pub fn overlay_upper(
        &self,
        layout: &Layout,
        aid: &AgentId,
    ) -> PathBuf {
        self.overlay(layout, aid).join(OVERLAY_UPPER_DIRNAME)
    }

    pub fn shared(
        &self,
        layout: &Layout,
    ) -> PathBuf {
        layout.data.join(SHARED_DIRNAME)
    }

    pub fn snapshots(
        &self,
        layout: &Layout,
    ) -> PathBuf {
        layout.data.join(SNAPSHOTS_DIRNAME)
    }

    pub fn snapshot(
        &self,
        layout: &Layout,
        commit: &str,
    ) -> PathBuf {
        self.snapshots(layout).join(commit)
    }
}
