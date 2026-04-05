use derive_getters::Getters;
use derive_more::AsRef;
use derive_more::Deref;
use nucleo_matcher::pattern::Atom;
use nucleo_matcher::pattern::AtomKind;
use nucleo_matcher::pattern::CaseMatching;
use nucleo_matcher::pattern::Normalization;

use super::*;

#[derive(Debug, Clone, Getters)]
pub struct Completion<'a> {
    /// possible matches
    source: Vec<CompletionItem<'a>>,
    /// matches for  the current entry
    items: Vec<CompletionItem<'a>>,

    /// only complete if we're at the start of the line (as in cmdline)
    only_leading: bool,

    matcher: Matcher,
    pub(super) state: ListState,
    active: Option<CompletionRequest>,
    max_height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Getters)]
pub struct CompletionRequest {
    /// column index of the start of the typed part
    start: usize,
    /// the query; if we cancel completion, we reset to this
    typed: String,
}

#[derive(Debug, Clone, Deref, AsRef, Getters)]
pub struct CompletionItem<'a> {
    /// value to insert
    #[deref(forward)]
    #[as_ref(forward)]
    value: String,
    rendered: ListItem<'a>,
}

impl CompletionItem<'_> {
    pub fn new(value: String) -> Self {
        Self {
            rendered: ListItem::new(value.clone()),
            value,
        }
    }
}

impl<'a> Completion<'a> {
    pub fn new(
        max_height: u16,
        source: Vec<CompletionItem<'a>>,
        only_leading: bool,
    ) -> Self {
        Self {
            matcher: Matcher::default(),
            max_height,
            items: Vec::new(),
            active: None,
            source,
            state: ListState::default(),
            only_leading,
        }
    }

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
        if self.only_leading && start != 0 {
            return None;
        }
        CompletionRequest {
            start,
            typed: typed.to_string(),
        }
        .into()
    }

    fn init(&mut self) {
        if self.active.as_ref().is_some_and(|x| x.typed.is_empty()) && self.items.is_empty() {
            self.items = self.source.clone();
            self.state.select(None);
        }
    }

    pub fn set_source(
        &mut self,
        source: Vec<CompletionItem<'a>>,
    ) {
        self.source = source;
        self.active = None;
    }

    // PERF use nucleo crate instead, .match_list is explicitly slow
    fn match_items(
        &mut self,
        query: &str,
    ) -> Vec<CompletionItem<'a>> {
        Atom::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        )
        .match_list(&self.source, &mut self.matcher)
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
        let request = self.completion.request(&line);
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
