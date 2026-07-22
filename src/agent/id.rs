use std::fmt::Debug;
use std::fmt::Display;

use derive_more::From;
use derive_more::Into;
use serde::Deserialize;
use serde::Serialize;

#[derive(
    From,
    Into,
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    schemars::JsonSchema,
)]
pub struct AgentId(String);

static GENERATOR: std::sync::LazyLock<petname::Petnames<'static>> =
    std::sync::LazyLock::new(petname::Petnames::small);

const SEPARATOR: &str = "-";
const WORDS: u8 = 3;

impl AgentId {
    pub fn generate_base() -> String {
        GENERATOR
            .namer(WORDS, SEPARATOR)
            .iter(&mut rand::rng())
            .next()
            .expect("petname generator is non-empty")
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
