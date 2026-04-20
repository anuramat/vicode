use derive_more::Deref;
use derive_more::DerefMut;
use serde::Deserialize;
use serde::Serialize;

use crate::llm::history::tokens::TokenCount;
use crate::llm::history::tokens::count_text_tokens;

#[derive(Clone, Serialize, Deserialize, Debug, Deref, DerefMut)]
pub struct Instructions {
    #[deref(forward)]
    #[deref_mut(forward)]
    pub text: String,
    pub token_count: usize,
}

impl Instructions {
    pub fn new(text: String) -> Self {
        let mut result = Self {
            text,
            token_count: 0,
        };
        result.recount();
        result
    }
}

impl TokenCount for Instructions {
    fn recount(&mut self) {
        self.token_count = count_text_tokens(&self.text);
    }

    fn token_count(&self) -> usize {
        self.token_count
    }
}
