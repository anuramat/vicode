use derive_getters::Getters;
use nucleo_matcher::pattern::Atom;
use nucleo_matcher::pattern::AtomKind;
use nucleo_matcher::pattern::CaseMatching;
use nucleo_matcher::pattern::Normalization;

use super::*;

#[derive(Debug, Clone, Default, Getters)]
pub struct Completion<'a> {
    sources: Vec<CompletionSource<'a>>,
    matcher: Matcher,
    matches: Vec<CompletionItem<'a>>,
    pub(super) state: ListState,
    active: Option<ActiveCompletion>,
    max_height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveCompletion {
    source: usize,
    pub(super) request: CompletionRequest,
}

#[derive(Clone)]
pub struct CompletionSource<'a> {
    pub id: &'static str,
    pub items: Vec<CompletionItem<'a>>,
    extract: CompletionExtract,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    pub start: usize,
    pub query: String,
    pub typed: String,
}

#[derive(Debug, Clone)]
pub struct CompletionItem<'a> {
    pub match_text: String,
    pub insert_text: String,
    pub rendered: ListItem<'a>,
}

impl CompletionItem<'_> {
    pub fn plain(text: String) -> Self {
        Self {
            match_text: text.clone(),
            insert_text: text.clone(),
            rendered: ListItem::new(text),
        }
    }
}

impl AsRef<str> for CompletionItem<'_> {
    fn as_ref(&self) -> &str {
        self.match_text.as_str()
    }
}

impl<'a> Completion<'a> {
    pub fn new(
        completion_max_height: u16,
        completion_sources: Vec<CompletionSource<'a>>,
    ) -> Self {
        Self {
            matcher: Matcher::default(),
            max_height: completion_max_height,
            matches: Vec::new(),
            active: None,
            sources: completion_sources,
            state: ListState::default(),
        }
    }
}

impl<'a> Input<'a> {
    pub fn handle_completion(&mut self) {
        let active = self.completion_request();
        if active == self.completion.active {
            return;
        }
        self.completion.active = active;
        self.completion.state.select(None);
        self.completion.matches = match self.completion.active.clone() {
            Some(active) if !active.request.query.is_empty() => {
                self.match_items(active.source, &active.request.query)
            }
            _ => Vec::new(),
        };
    }

    fn completion_accept(&mut self) {
        if let Some(item) = self
            .completion
            .matches
            .get(self.completion.state.selected().unwrap_or(0))
            .cloned()
        {
            self.replace_completion(&item.insert_text);
        }
    }

    pub fn completion_cancel(&mut self) {
        let Some(active) = self.completion.active.as_ref() else {
            return;
        };
        let typed = active.request.typed.clone();
        self.replace_completion(&typed);
        self.clear_completion();
    }

    pub fn completion_next(&mut self) {
        if !self.init_if_empty() {
            return;
        }
        if self.completion.state.selected().is_none() {
            self.completion.state.select(Some(0));
        } else {
            self.completion.state.select_next();
        }
        self.completion_accept();
    }

    pub fn completion_prev(&mut self) {
        if !self.init_if_empty() {
            return;
        }
        if self.completion.state.selected().is_none() {
            self.completion
                .state
                .select(Some(self.completion.matches.len().saturating_sub(1)));
        } else {
            self.completion.state.select_previous();
        }
        self.completion_accept();
    }

    pub fn set_completion_items(
        &mut self,
        id: &str,
        items: Vec<CompletionItem<'a>>,
    ) {
        let active = self
            .completion
            .active
            .as_ref()
            .is_some_and(|active| self.completion.sources[active.source].id == id);
        if let Some(source) = self
            .completion
            .sources
            .iter_mut()
            .find(|source| source.id == id)
        {
            source.items = items;
            if active {
                self.completion.active = None;
            }
            self.handle_completion();
        }
    }

    pub fn completion_matches(&self) -> &[CompletionItem<'a>] {
        &self.completion.matches
    }

    pub fn completion_items(
        &self,
        id: &str,
    ) -> Option<&[CompletionItem<'a>]> {
        self.completion
            .sources
            .iter()
            .find(|source| source.id == id)
            .map(|source| source.items.as_slice())
    }

    pub fn single_completion_match_text(&self) -> Option<&str> {
        (self.completion.matches.len() == 1).then(|| self.completion.matches[0].match_text.as_str())
    }

    pub fn completion_request(&self) -> Option<ActiveCompletion> {
        let line = self.line();
        self.completion
            .sources
            .iter()
            .enumerate()
            .find_map(|(source, completion)| {
                (completion.extract)(&line).map(|request| ActiveCompletion { source, request })
            })
    }

    fn replace_completion(
        &mut self,
        text: &str,
    ) {
        let Some(start) = self
            .completion
            .active
            .as_ref()
            .map(|active| active.request.start)
        else {
            return;
        };
        let active = self.line().chars().count().saturating_sub(start);
        for _ in 0..active {
            self.textarea.delete_char();
        }
        self.textarea.insert_str(text);
    }

    pub fn clear_completion(&mut self) {
        self.completion.matches.clear();
        self.completion.active = None;
        self.completion.state.select(None);
    }

    fn init_if_empty(&mut self) -> bool {
        let Some(active) = self.completion.active.as_ref() else {
            return false;
        };
        if active.request.query.is_empty() && self.completion.matches.is_empty() {
            self.completion.matches = self.completion.sources[active.source].items.clone();
            self.completion.state.select(None);
        }
        !self.completion.matches.is_empty()
    }

    pub fn match_items(
        &mut self,
        source: usize,
        query: &str,
    ) -> Vec<CompletionItem<'a>> {
        Atom::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        )
        .match_list(
            &self.completion.sources[source].items,
            &mut self.completion.matcher,
        )
        .into_iter()
        .map(|item| item.0.clone())
        .collect()
    }
}

impl std::fmt::Debug for CompletionSource<'_> {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        f.debug_struct("CompletionSource")
            .field("id", &self.id)
            .field("items", &self.items)
            .finish()
    }
}

impl<'a> CompletionSource<'a> {
    pub fn new<F>(
        id: &'static str,
        items: Vec<CompletionItem<'a>>,
        extract: F,
    ) -> Self
    where
        F: Fn(&str) -> Option<CompletionRequest> + Send + Sync + 'static,
    {
        Self {
            id,
            items,
            extract: Arc::new(extract),
        }
    }

    pub fn leading_word(
        id: &'static str,
        items: Vec<CompletionItem<'a>>,
    ) -> Self {
        Self::new(id, items, leading_word_request)
    }

    pub fn prefixed_word(
        id: &'static str,
        prefix: char,
        items: Vec<CompletionItem<'a>>,
    ) -> Self {
        Self::new(id, items, move |line| prefixed_word_request(line, prefix))
    }
}

fn prefixed_word_request(
    text: &str,
    prefix: char,
) -> Option<CompletionRequest> {
    let (start, typed) = word(text);
    typed.strip_prefix(prefix).map(|query| CompletionRequest {
        start,
        query: query.to_string(),
        typed: typed.to_string(),
    })
}

fn leading_word_request(text: &str) -> Option<CompletionRequest> {
    let (start, typed) = word(text);
    (start == 0).then(|| CompletionRequest {
        start,
        query: typed.to_string(),
        typed: typed.to_string(),
    })
}

fn word(text: &str) -> (usize, &str) {
    text.rsplit_once(' ')
        .map(|(head, tail)| (head.chars().count() + 1, tail))
        .unwrap_or((0, text))
}
