use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use smart_default::SmartDefault;

// XXX add "clear_default" like with keymaps so that by default stages append instead of
// overwriting
use super::Sandbox;
use super::SandboxRunner;
use crate::config::expand_vec;
use crate::config::vec;

fn default_stages() -> Vec<Stage> {
    let args = vec(["--die-with-parent", "--proc", "/proc", "--dev", "/dev"]);
    let ro = expand_vec([
        "/nix",
        "/bin",
        "/usr",
        "/etc",
        "/lib",
        "/lib64",
        "/run/current-system",
        "/run/systemd/resolve/stub-resolv.conf",
        "$XDG_CONFIG_HOME",
        "~/.bashrc",
        "~/.bash_profile",
        "~/.profile",
    ]);
    let tmpfs = expand_vec([
        "/tmp",
        "$TMPDIR",
        "$XDG_CACHE_HOME",
        "$XDG_STATE_HOME",
        "$XDG_DATA_HOME",
    ]);
    vec![Stage {
        args,
        ro,
        tmpfs,
        ..Default::default()
    }]
}

#[derive(Deserialize, Serialize, Debug, Clone, SmartDefault, JsonSchema)]
#[serde(default)]
pub struct BwrapConfig {
    #[default("bwrap")]
    pub bin: String,
    #[default(default_stages())]
    pub stages: Vec<Stage>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, JsonSchema)]
#[serde(default)]
pub struct Stage {
    pub ro: Vec<String>,
    pub rw: Vec<String>,
    pub tmpfs: Vec<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BwrapRunner {
    pub bin: String,
    pub args: Vec<String>,
}

const WORKDIR_MOUNT_NAME: &str = "/workdir";

impl SandboxRunner {
    fn ro(
        &mut self,
        args: Vec<String>,
    ) {
        for arg in args {
            self.args.push("--ro-bind-try".to_string());
            self.args.push(arg.clone());
            self.args.push(arg);
        }
    }

    fn rw(
        &mut self,
        args: Vec<String>,
    ) {
        for arg in args {
            self.args.push("--bind-try".to_string());
            self.args.push(arg.clone());
            self.args.push(arg);
        }
    }

    fn tmpfs(
        &mut self,
        args: Vec<String>,
    ) {
        for arg in args {
            self.args.push("--tmpfs".to_string());
            self.args.push(arg);
        }
    }

    fn workdir(
        &mut self,
        workdir: &str,
    ) {
        // TODO expose in config
        self.args.push("--bind-try".to_string());
        self.args.push(workdir.to_string());
        self.args.push(WORKDIR_MOUNT_NAME.to_string());
        self.args.push("--chdir".to_string());
        self.args.push(WORKDIR_MOUNT_NAME.to_string());
    }

    fn apply_stage(
        &mut self,
        stage: &Stage,
    ) {
        self.rw(expand_vec(&stage.rw));
        self.ro(expand_vec(&stage.ro));
        self.tmpfs(expand_vec(&stage.tmpfs));
        for arg in &stage.args {
            // TODO expand but if it fails, error out
            self.args.push(arg.clone());
        }
    }
}

impl Sandbox for BwrapConfig {
    fn runner(
        &self,
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> SandboxRunner {
        let mut runner = SandboxRunner {
            bin: self.bin.clone(),
            args: vec![],
            cwd: cwd.clone(),
        };
        for stage in &self.stages {
            runner.apply_stage(stage);
        }
        runner.rw(vec![gitdir.to_string_lossy().to_string()]);
        runner.workdir(&cwd.to_string_lossy());
        runner
    }
}
