use std::fmt::Debug;
use std::fmt::Display;

use anyhow::Result;
use derive_more::From;
use derive_more::Into;
use petname::Generator;
use serde::Deserialize;
use serde::Serialize;

use crate::project::PROJECT;
use crate::project::layout::LayoutTrait;

#[derive(From, Into, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentId(String);

lazy_static::lazy_static! {
    static ref GENERATOR: petname::Petnames<'static> = petname::Petnames::small();
}

const SEPARATOR: &str = "-";
const WORDS: u8 = 3;
const PATIENCE: usize = 3;

impl AgentId {
    pub async fn new() -> Result<Self> {
        for _ in 0..PATIENCE {
            let id: Self = GENERATOR.generate_one(WORDS, SEPARATOR).unwrap().into();
            if !PROJECT.agent_id_exists(&id).await? {
                return Ok(id);
            }
        }
        anyhow::bail!("{} name collisions when generating AgentId", PATIENCE);
    }
}

impl Display for AgentId {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
