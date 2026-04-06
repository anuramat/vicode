use anyhow::bail;
use derive_getters::Getters;
use derive_more::AsRef;
use derive_more::Deref;
use nucleo_matcher::pattern::Atom;
use nucleo_matcher::pattern::AtomKind;
use nucleo_matcher::pattern::CaseMatching;
use nucleo_matcher::pattern::Normalization;

use super::*;

#[derive(Debug, Clone, Getters)]
pub struct Completion {
    /// possible matches
    source: CompletionSource,
    /// matches for  the current entry
    items: Vec<CompletionItem>,

    matcher: Matcher,
    pub(super) state: ListState,
    active: Option<CompletionRequest>,
    max_height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionSource {
    /// items always the same, triggered only on the leading word
    Command(Vec<CompletionItem>),
    /// matches based on a leading char
    Freeform(Vec<(char, Vec<CompletionItem>)>),
}

impl CompletionSource {
    fn request(
        &self,
        line: &str,
    ) -> Option<CompletionRequest> {
        fn last_word(line: &str) -> (usize, &str) {
            line.rsplit_once(' ')
                .map(|(head, tail)| (head.chars().count() + 1, tail))
                .unwrap_or((0, line))
        }
        let (start, typed) = last_word(line);
        match self {
            Self::Command(_) if start == 0 => Some(CompletionRequest {
                start,
                typed: typed.to_string(),
            }),
            Self::Freeform(items) => items
                .iter()
                .any(|(prefix, _)| typed.starts_with(*prefix))
                .then(|| CompletionRequest {
                    start,
                    typed: typed.to_string(),
                }),
            _ => None,
        }
    }

    fn items(
        &self,
        typed: &str,
    ) -> Vec<CompletionItem> {
        match self {
            Self::Command(items) => items.clone(),
            Self::Freeform(groups) => typed
                .chars()
                .next()
                .and_then(|prefix| groups.iter().find(|(x, _)| *x == prefix))
                .map(|(_, items)| items.clone())
                .unwrap_or_default(),
        }
    }

    pub fn set_items(
        &mut self,
        prefix: char,
        items: Vec<CompletionItem>,
    ) -> Result<()> {
        match self {
            Self::Freeform(groups) => {
                if let Some((_, current)) = groups.iter_mut().find(|(x, _)| *x == prefix) {
                    *current = items;
                } else {
                    groups.push((prefix, items));
                }
            }
            _ => {
                bail!("can only set items for freeform completion sources");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Getters)]
pub struct CompletionRequest {
    /// column index of the start of the typed part
    start: usize,
    /// the query; if we cancel completion, we reset to this
    typed: String,
}

#[derive(Debug, Clone, Deref, AsRef, Getters, PartialEq, Eq)]
pub struct CompletionItem {
    /// value to insert
    #[deref(forward)]
    #[as_ref(forward)]
    value: String,
    rendered: ListItem<'static>,
}

impl CompletionItem {
    pub fn new(value: String) -> Self {
        Self {
            rendered: ListItem::new(value.clone()),
            value,
        }
    }
}

impl Completion {
    pub fn new(
        max_height: u16,
        source: CompletionSource,
    ) -> Self {
        Self {
            matcher: Matcher::default(),
            max_height,
            items: Vec::new(),
            active: None,
            source,
            state: ListState::default(),
        }
    }

    pub fn source_mut(&mut self) -> &mut CompletionSource {
        &mut self.source
    }

    fn init(&mut self) {
        if self.items.is_empty() {
            self.items = self.source.items(
                self.active
                    .as_ref()
                    .map(|active| active.typed.as_str())
                    .unwrap_or_default(),
            );
            self.state.select(None);
        }
    }

    // PERF use nucleo crate instead, .match_list is explicitly slow
    fn match_items(
        &mut self,
        query: &str,
    ) -> Vec<CompletionItem> {
        Atom::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        )
        .match_list(&self.source.items(query), &mut self.matcher)
        .into_iter()
        .map(|(item, _)| item.clone())
        .collect()
    }

    fn handle(
        &mut self,
        request: Option<CompletionRequest>,
    ) {
        if request == self.active {
            return;
        }
        self.active = request;
        self.state.select(None);
        self.items = match self.active.clone() {
            Some(active) if !active.typed.is_empty() => self.match_items(&active.typed),
            _ => Vec::new(),
        };
    }
}

impl<'a> Input<'a> {
    pub(super) fn completion_update(&mut self) {
        let line = self.line_until_cursor();
        let request = self.completion.source.request(&line);
        self.completion.handle(request);
    }

    fn completion_accept(&mut self) {
        if let Some(item) = self
            .completion
            .items
            .get(self.completion.state.selected().unwrap_or(0))
            .cloned()
        {
            self.replace_word(&item.value);
        }
    }

    pub fn completion_cancel(&mut self) {
        let Some(active) = self.completion.active.as_ref() else {
            return;
        };
        let typed = active.typed.clone();
        self.replace_word(&typed);
        self.clear_completion();
    }

    pub fn completion_next(&mut self) {
        self.completion.init();
        if self.completion.state.selected().is_none() {
            self.completion.state.select(Some(0));
        } else {
            self.completion.state.select_next();
        }
        self.completion_accept();
    }

    pub fn completion_prev(&mut self) {
        self.completion.init();
        if self.completion.state.selected().is_none() {
            self.completion
                .state
                .select(Some(self.completion.items.len().saturating_sub(1)));
        } else {
            self.completion.state.select_previous();
        }
        self.completion_accept();
    }

    pub fn only_match(&self) -> Option<&str> {
        (self.completion.items.len() == 1).then(|| self.completion.items[0].value.as_str())
    }

    fn replace_word(
        &mut self,
        text: &str,
    ) {
        let Some(col) = self.completion.active.as_ref().map(|active| active.start) else {
            return;
        };
        let n_chars = self.textarea.cursor().1.saturating_sub(col);
        // NOTE very depressing api
        for _ in 0..n_chars {
            self.textarea.delete_char();
        }
        self.textarea.insert_str(text);
    }

    pub(super) fn clear_completion(&mut self) {
        self.completion.items.clear();
        self.completion.active = None;
        self.completion.state.select(None);
    }
}
