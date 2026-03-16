use std::iter::Iterator;
use std::path::PathBuf;
use std::process::Output;

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use shellexpand::full;
use tokio::process::Command;

use crate::config::CONFIG;

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct BwrapConfig {
    pub bin: String,
    pub args: Vec<String>,
    pub stages: Vec<Stage>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(default)]
pub struct Stage {
    pub ro: Vec<String>,
    pub rw: Vec<String>,
    pub tmpfs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BwrapRunner {
    pub bwrap_bin: String,
    pub bwrap_args: Vec<String>,
}

const WORKDIR_MOUNT_NAME: &str = "/workdir";

impl BwrapRunner {
    pub fn ro(
        mut self,
        args: Vec<String>,
    ) -> Self {
        for arg in args {
            self.bwrap_args.push("--ro-bind-try".to_string());
            self.bwrap_args.push(arg.clone());
            self.bwrap_args.push(arg);
        }
        self
    }

    pub fn rw(
        mut self,
        args: Vec<String>,
    ) -> Self {
        for arg in args {
            self.bwrap_args.push("--bind-try".to_string());
            self.bwrap_args.push(arg.clone());
            self.bwrap_args.push(arg);
        }
        self
    }

    pub fn workdir(
        mut self,
        workdir: &str,
    ) -> Self {
        self.bwrap_args.push("--bind-try".to_string());
        self.bwrap_args.push(workdir.to_string());
        self.bwrap_args.push(WORKDIR_MOUNT_NAME.to_string());
        self.bwrap_args.push("--chdir".to_string());
        self.bwrap_args.push(WORKDIR_MOUNT_NAME.to_string());
        self
    }

    pub fn tmpfs(
        mut self,
        args: Vec<String>,
    ) -> Self {
        for arg in args {
            self.bwrap_args.push("--tmpfs".to_string());
            self.bwrap_args.push(arg);
        }
        self
    }

    pub async fn exec(
        &self,
        script: String,
    ) -> Result<Output> {
        let mut bash_cmd = CONFIG.bash.cmd.clone();
        bash_cmd.push(script);
        let mut bwrap_bin = Command::new(&self.bwrap_bin);
        Ok(bwrap_bin
            .args(&self.bwrap_args)
            .args(bash_cmd.into_iter())
            .output()
            .await?)
    }

    pub fn new(
        cwd: PathBuf,
        gitdir: PathBuf,
    ) -> Self {
        let mut runner = Self {
            bwrap_bin: CONFIG.bash.bwrap.bin.clone(),
            bwrap_args: CONFIG.bash.bwrap.args.clone(),
        };

        for stage in &CONFIG.bash.bwrap.stages {
            let rw = expand_vec(&stage.rw);
            let ro = expand_vec(&stage.ro);
            let tmpfs = expand_vec(&stage.tmpfs);
            runner = runner.rw(rw).ro(ro).tmpfs(tmpfs);
        }

        runner
            .workdir(&cwd.to_string_lossy())
            .rw(vec![gitdir.to_string_lossy().to_string()])
    }
}

fn expand_vec(v: &[String]) -> Vec<String> {
    v.iter()
        .flat_map(|s| full(s).ok())
        .map(String::from)
        .collect()
}
