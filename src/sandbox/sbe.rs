use std::path::Path;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use smart_default::SmartDefault;

use super::SandboxRunner;

// from openai/codex at aa184548b1ee0a559fba7be658240053e270e16f
const BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");
const NETWORK_POLICY: &str = include_str!("seatbelt_network_policy.sbpl");
const READONLY_POLICY: &str = include_str!("restricted_read_only_platform_defaults.sbpl");

fn default_profile() -> String {
    let mut profile = String::new();
    profile.push_str(BASE_POLICY);
    profile.push_str(NETWORK_POLICY);
    profile.push_str(READONLY_POLICY);
    profile.push_str(
        r#"
(allow file-read*
    (subpath (param "XDG_CONFIG_HOME"))
)
(allow file-read* file-write*
    (subpath (param "WORKDIR"))
    (subpath (param "GITDIR"))
    (subpath (param "XDG_CACHE_HOME"))
    (subpath (param "XDG_DATA_HOME"))
    (subpath (param "XDG_STATE_HOME"))
)
"#,
    );
    profile
}

#[derive(Debug, Clone, SmartDefault, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SbeConfig {
    #[default("sandbox-exec")]
    pub bin: String,
    pub profile: String,
}

impl SbeConfig {
    fn make_param_args(
        &self,
        cwd: &Path,
        gitdir: &Path,
    ) -> Option<Vec<String>> {
        let xdg = xdg::BaseDirectories::new();
        let params = vec![
            format!("WORKDIR={}", cwd.to_str()?),
            format!("GITDIR={}", gitdir.to_str()?),
            format!("HOME={}", std::env::home_dir()?.to_str()?),
            format!("XDG_CACHE_HOME={}", xdg.get_cache_home()?.to_str()?),
            format!("XDG_CONFIG_HOME={}", xdg.get_config_home()?.to_str()?),
            format!("XDG_DATA_HOME={}", xdg.get_data_home()?.to_str()?),
            format!("XDG_STATE_HOME={}", xdg.get_state_home()?.to_str()?),
        ];
        let mut result = Vec::new();
        for i in params {
            result.push("-D".to_string());
            result.push(i.clone());
        }
        Some(result)
    }
}

impl crate::sandbox::Sandbox for SbeConfig {
    fn runner(
        &self,
        cwd: std::path::PathBuf,
        gitdir: std::path::PathBuf,
    ) -> SandboxRunner {
        let mut args = self
            .make_param_args(&cwd, &gitdir)
            .expect("non-unicode path in sandbox config or no home directory set");
        args.extend(vec!["-p".to_string(), self.profile.clone()]);
        SandboxRunner {
            bin: self.bin.clone(),
            args,
            cwd,
        }
    }

    fn merge_default(&mut self) {
        self.profile.insert_str(0, &default_profile());
    }
}
