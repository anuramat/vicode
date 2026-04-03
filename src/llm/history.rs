use std::mem;

use anyhow::Context;
use anyhow::Result;
use derive_more::AsMut;
use derive_more::AsRef;
use derive_more::Deref;
use derive_more::DerefMut;
use derive_more::From;
use derive_more::Into;
use indexmap::indexmap;
use serde::Deserialize;
use serde::Serialize;

use crate::llm::delta::Delta;
use crate::llm::message::AssistantItem;
use crate::llm::message::AssistantMessage;
use crate::llm::message::AssistantMessageStatus;
use crate::llm::message::DeveloperMessage;
use crate::llm::message::HistoryEntry;
use crate::llm::message::ItemTiming;
use crate::llm::message::Message;
use crate::llm::message::MessageMeta;
use crate::llm::message::UserMessage;
use crate::llm::tokens::count_message_tokens;
use crate::llm::tokens::count_text_tokens;

#[derive(Default, Clone, Serialize, Deserialize, Debug, Deref, DerefMut)]
pub struct History {
    instructions: Instructions,
    #[serde(skip)]
    generation: HistoryGeneration,
    #[deref]
    #[deref_mut]
    state: HistoryState,
    #[serde(default)]
    archive: Vec<ArchivedHistory>,
}

#[derive(Default, Clone, Serialize, Deserialize, Debug, Deref, DerefMut)]
pub struct Instructions {
    #[deref]
    #[deref_mut]
    text: String,
    token_count: usize,
}

#[derive(Default, Clone, Serialize, Deserialize, Debug, Deref, DerefMut, AsRef, AsMut)]
pub struct HistoryState {
    token_count: usize, // cached sum of entries.token_count
    #[deref]
    #[deref_mut]
    entries: Entries,
    pub compact: Option<CompactState>,
}

#[derive(Default, Clone, Serialize, Deserialize, Debug, Deref, DerefMut)]
pub struct CompactState {
    #[deref]
    #[deref_mut]
    pub entries: Entries,
    pub dropped: usize,
    pub needs_another_turn: bool,
}

#[derive(Default, Clone, Serialize, Deserialize, Debug, Deref, DerefMut, From, Into)]
pub struct Entries(Vec<HistoryEntry>);

#[derive(Clone, Serialize, Deserialize, Debug, Deref, DerefMut)]
pub struct ArchivedHistory {
    #[deref]
    #[deref_mut]
    pub state: HistoryState,
    pub reason: ArchivedHistoryReason,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum ArchivedHistoryReason {
    Compact,
    Undo,
}

pub type HistoryGeneration = u64;

#[derive(Debug, Clone)]
pub enum ResponseEvent {
    Started(u64),
    Delta(Delta),
    Item(Box<AssistantItem>),
    Completed(Vec<AssistantItem>),
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum HistoryUpdate {
    CompactStart {
        dropped: usize,
        needs_another_turn: bool,
    },
    CompactResponse(ResponseEvent),
    GenerationIncremented,
    TurnResponse(ResponseEvent),
    UserMessage(String),
    DeveloperMessage(DeveloperMessage),
    Pop(usize),
}

impl FromIterator<HistoryEntry> for Entries {
    fn from_iter<I: IntoIterator<Item = HistoryEntry>>(iter: I) -> Self {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

// TODO try making this work
// impl<T> FromIterator<T> for Vec<Message>
// where T: Into<HistoryEntry>
// {
//     fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
//         iter.into_iter().collect()
//     }
// }

impl FromIterator<HistoryEntry> for Vec<Message> {
    fn from_iter<I: IntoIterator<Item = HistoryEntry>>(iter: I) -> Self {
        iter.into_iter().map(|entry| entry.message).collect()
    }
}

impl<'a> FromIterator<&'a HistoryEntry> for Vec<Message> {
    fn from_iter<I: IntoIterator<Item = &'a HistoryEntry>>(iter: I) -> Self {
        iter.into_iter()
            .map(|entry| entry.message.clone())
            .collect()
    }
}

impl HistoryState {
    fn apply_delta(
        &mut self,
        delta: isize,
    ) {
        self.token_count = self.token_count.saturating_add_signed(delta);
    }

    fn recount(&mut self) {
        self.entries.count_tokens();
        self.token_count = self.entries.total_tokens();
    }

    pub fn init_compact(
        &mut self,
        dropped: usize,
        needs_another_turn: bool,
    ) -> Result<()> {
        self.compact = Some(CompactState {
            entries: self.iter().take(dropped).cloned().collect(),
            dropped,
            needs_another_turn,
        });
        Ok(())
    }

    pub fn compacting(&self) -> bool {
        self.compact.is_some()
    }
}

impl Entries {
    pub fn count_tokens(&mut self) {
        self.iter_mut().for_each(|entry| {
            entry.count_tokens();
        });
    }

    pub fn total_tokens(&self) -> usize {
        self.0.iter().map(|entry| entry.meta.token_count).sum()
    }

    // TODO rename two below
    pub fn compactable_messages(
        &self,
        dropped: usize,
    ) -> Vec<Message> {
        self.iter()
            .take(dropped)
            .map(|entry| entry.message.clone())
            .collect()
    }

    pub fn compact_dropped(
        &self,
        window: usize,
        target: usize,
    ) -> usize {
        let target = window * target / 100;
        let mut kept = self.total_tokens();
        for (idx, entry) in self.iter().enumerate() {
            if kept < target {
                return idx;
            }
            kept -= entry.meta.token_count;
        }
        self.len()
    }

    pub fn needs_another_turn(&self) -> bool {
        if let Some(entry) = self.last() {
            match &entry.message {
                Message::Assistant(msg) => msg.content.iter().any(|(_, content)| {
                    matches!(content, AssistantItem::ToolCall(_))
                        && matches!(msg.finish_reason, AssistantMessageStatus::Success)
                }),
                Message::Developer(msg) => match msg {
                    DeveloperMessage::Compact(compact) => compact.needs_another_turn,
                    DeveloperMessage::SubagentReport(_) => true,
                    _ => false,
                },
                Message::User(_) => false,
            }
        } else {
            false
        }
    }

    pub fn handle_response(
        &mut self,
        event: ResponseEvent,
    ) -> isize {
        match event {
            ResponseEvent::Started(started_at_ms) => {
                self.start_response(started_at_ms);
                self.recount_last_message()
            }
            ResponseEvent::Delta(item_delta) => {
                self.push_delta(item_delta);
                self.recount_last_message()
            }
            ResponseEvent::Item(item) => {
                self.push_item(*item);
                self.recount_last_message()
            }
            ResponseEvent::Completed(items) => {
                self.complete_response(items);
                self.recount_last_message()
            }
            ResponseEvent::Failed(msg) => {
                self.fail_response(msg);
                self.recount_last_message()
            }
        }
    }

    pub fn push_message(
        &mut self,
        message: Message,
    ) -> isize {
        let delta = count_message_tokens(&message) as isize;
        self.push(HistoryEntry {
            meta: MessageMeta::default(),
            message,
        });
        self.recount_last_message();
        delta
    }

    pub fn pop(
        &mut self,
        n: usize,
    ) -> Result<isize> {
        let len = self.len();
        anyhow::ensure!(
            n <= len,
            "Cannot pop {} messages from history of length {}",
            n,
            len
        );
        let popped = self.split_off(len - n);
        Ok(-popped
            .iter()
            .map(|entry| entry.meta.token_count as isize)
            .sum::<isize>())
    }

    pub fn push_item(
        &mut self,
        mut item: AssistantItem,
    ) {
        let item_modified = item.timing().last_modified_ms;
        let item_started = item.timing().started_at_ms;
        if let Some(Message::Assistant(msg)) = self.last_mut().map(|entry| &mut entry.message) {
            // if item already exists -- replace it but preserve start
            // if item has finish, it means that we constructed it from delta, the new item is
            // just for consistency guarantee, and thus we actually finished the
            // existing item when the last delta arrived, so we preserve the smaller finish value
            if let Some(existing) = msg.content.get(&item.id()) {
                item.timing_mut().started_at_ms = existing.timing().started_at_ms;
                if let Some(modified) = existing.timing().last_modified_ms {
                    item.timing_mut().last_modified_ms = Some(modified);
                }
            }
            _ = msg.content.insert(item.id(), item);
        } else {
            // XXX does this ever happen
            let msg = AssistantMessage {
                finish_reason: AssistantMessageStatus::InProgress,
                content: indexmap! {item.id() => item},
            };
            self.push(HistoryEntry {
                meta: MessageMeta {
                    timing: ItemTiming::with_start(item_started),
                    ..Default::default()
                },
                message: msg.into(),
            });
        }
        if let Some(modified) = item_modified {
            self.last_mut().unwrap().meta.timing.touch_at(modified);
        }
    }

    pub fn start_response(
        &mut self,
        started_at_ms: u64,
    ) {
        self.push(HistoryEntry {
            meta: MessageMeta {
                timing: ItemTiming::with_start(started_at_ms),
                ..Default::default()
            },
            message: AssistantMessage::default().into(),
        });
    }

    pub fn complete_response(
        &mut self,
        _items: Vec<AssistantItem>,
    ) {
        if let Some(Message::Assistant(msg)) = self.last_mut().map(|entry| &mut entry.message) {
            msg.finish_reason = AssistantMessageStatus::Success;
        } else {
            // XXX does this ever happen?
            self.push(HistoryEntry {
                meta: MessageMeta::default(),
                message: AssistantMessage {
                    finish_reason: AssistantMessageStatus::Success,
                    content: indexmap! {},
                }
                .into(),
            });
        }
    }

    pub fn fail_response(
        &mut self,
        error_text: String, // TODO rename to msg or whatever
    ) {
        if let Some(Message::Assistant(msg)) = self.last_mut().map(|entry| &mut entry.message)
            && matches!(msg.finish_reason, AssistantMessageStatus::InProgress)
        // TODO inProgress check should be unnecessary
        {
            msg.finish_reason = AssistantMessageStatus::Error(error_text);
        }
    }

    pub fn last_output(&self) -> Result<String> {
        if let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = self.last()
        {
            Ok(msg.output())
        } else {
            Err(anyhow::anyhow!("last message is not from the assistant",))
        }
    }

    pub fn outputs_after(
        &self,
        n: usize,
    ) -> String {
        self.iter()
            .skip(n)
            .filter_map(|entry| match &entry.message {
                Message::Assistant(msg) => Some(msg.output()),
                _ => None,
            })
            .collect()
    }

    // XXX rename
    pub fn recount_last_message(&mut self) -> isize {
        let Some(entry) = self.last_mut() else {
            return 0;
        };
        let old = entry.meta.token_count;
        entry.count_tokens();
        entry.meta.token_count as isize - old as isize
    }
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_instructions(instructions: String) -> Self {
        let mut history = Self::new();
        history.set_instructions(instructions);
        history
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    pub fn set_instructions(
        &mut self,
        instructions: String,
    ) {
        self.instructions.text = instructions;
        self.instructions.token_count = count_text_tokens(&self.instructions.text);
    }

    pub fn total_tokens(&self) -> usize {
        self.instructions.token_count + self.state.token_count
    }

    pub fn count_tokens(&mut self) {
        self.instructions.token_count = count_text_tokens(&self.instructions.text);
        self.state.recount();
        if let Some(compact) = &mut self.compact {
            compact.entries.count_tokens();
        }
    }

    pub fn compact_dropped(
        &self,
        window: usize,
        target: usize,
    ) -> usize {
        let target = window.saturating_mul(target) / 100;
        let mut kept = self.total_tokens();
        for (idx, entry) in self.iter().enumerate() {
            if kept < target {
                return idx;
            }
            kept -= entry.meta.token_count;
        }
        self.len()
    }

    fn increment(&mut self) {
        self.generation += 1;
    }

    pub fn generation(&self) -> HistoryGeneration {
        self.generation
    }

    // TODO go though this again
    pub fn handle(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryUpdate,
    ) -> Result<isize> {
        anyhow::ensure!(
            generation == self.generation,
            "history event generation {} does not match current generation {} in {:?}",
            generation,
            self.generation,
            self.entries
        );
        let delta = match event {
            HistoryUpdate::GenerationIncremented => {
                self.increment();
                0
            }
            HistoryUpdate::TurnResponse(event) => {
                let delta = self.entries.handle_response(event);
                self.state.apply_delta(delta);
                delta
            }
            HistoryUpdate::CompactResponse(event) => {
                self.compact
                    .as_mut()
                    .context("no compact in progress")?
                    .entries
                    .handle_response(event.clone());
                match event {
                    ResponseEvent::Completed(_) => self.apply_compact()?,
                    ResponseEvent::Failed(_) => 0,
                    _ => 0,
                }
            }
            HistoryUpdate::CompactStart {
                dropped,
                needs_another_turn,
            } => {
                self.state.init_compact(dropped, needs_another_turn)?;
                0
            }
            HistoryUpdate::DeveloperMessage(message) => {
                let delta = self.entries.push_message(Message::Developer(message));
                self.state.apply_delta(delta);
                delta
            }
            HistoryUpdate::UserMessage(text) => {
                let delta = self
                    .entries
                    .push_message(Message::User(UserMessage { text }));
                self.state.apply_delta(delta);
                delta
            }
            HistoryUpdate::Pop(n) => {
                let delta = self.entries.pop(n)?;
                self.state.apply_delta(delta);
                delta
            }
        };
        Ok(delta)
    }

    pub fn apply_compact(&mut self) -> Result<isize> {
        let CompactState {
            entries,
            dropped,
            needs_another_turn,
        } = self.compact.take().context("no compact to apply")?;
        let summary = entries.outputs_after(dropped).trim().to_string();

        anyhow::ensure!(!summary.is_empty(), "compact summary is empty");
        anyhow::ensure!(
            dropped <= self.entries.len(),
            "cannot compact {} messages from history of length {}",
            dropped,
            self.entries.len()
        );

        let old_total = self.total_tokens() as isize;
        let old_state = mem::take(&mut self.state);

        let new_state = {
            let summary_msg = HistoryEntry::new(Message::Developer(DeveloperMessage::Compact(
                crate::llm::message::CompactMessage {
                    text: summary,
                    needs_another_turn,
                },
            )));
            let mut entries: Entries = vec![summary_msg].into();
            entries.extend(old_state.iter().skip(dropped).cloned());
            HistoryState {
                token_count: entries.total_tokens(),
                entries,
                compact: None,
            }
        };

        self.state = new_state;
        self.archive.push(ArchivedHistory {
            state: old_state,
            reason: ArchivedHistoryReason::Compact,
        });

        self.generation += 1;

        let new_total = self.total_tokens() as isize;
        Ok(new_total - old_total)
    }
}

#[cfg(test)]
mod tests {
    use indexmap::indexmap;

    use super::*;
    use crate::llm::message::CompactMessage;
    use crate::llm::message::OutputContent;
    use crate::llm::message::OutputItem;
    use crate::llm::tokens::count_text_tokens;

    fn response(event: ResponseEvent) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(event)
    }

    fn compact_response(event: ResponseEvent) -> HistoryUpdate {
        HistoryUpdate::CompactResponse(event)
    }

    fn compact_summary(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            finish_reason: AssistantMessageStatus::Success,
            content: indexmap! {
                "out".into() => AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming::new(),
                    content: vec![OutputContent::Text(text.into())],
                }),
            },
        })
    }

    #[test]
    fn response_starts_without_assistant_message() {
        let history = History::new();
        assert!(history.entries.is_empty());
    }

    #[test]
    fn response_failed_without_message_keeps_history_empty() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Failed("oops".into())))
            .unwrap();
        assert!(history.entries.is_empty());
    }

    #[test]
    fn response_failed_without_message_for_abort_keeps_history_empty() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Failed("aborted by user".into())))
            .unwrap();
        assert!(history.entries.is_empty());
    }

    #[test]
    fn response_failed_after_user_message_keeps_history_unchanged() {
        let mut history = History::new();
        history
            .handle(0, HistoryUpdate::UserMessage("hello".into()))
            .unwrap();
        let total_tokens = history.total_tokens();
        history
            .handle(0, response(ResponseEvent::Failed("oops".into())))
            .unwrap();
        assert!(matches!(
            history.last().map(|entry| &entry.message),
            Some(Message::User(UserMessage { text })) if text == "hello"
        ));
        assert_eq!(history.total_tokens(), total_tokens);
    }

    #[test]
    fn response_started_creates_empty_assistant_message() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Started(7)))
            .unwrap();
        let Some(HistoryEntry {
            meta,
            message: Message::Assistant(msg),
        }) = history.entries.first()
        else {
            panic!("expected assistant message");
        };
        assert!(msg.content.is_empty());
        assert_eq!(meta.timing.started_at_ms, 7);
        assert_eq!(meta.timing.last_modified_ms, None);
        assert!(matches!(
            msg.finish_reason,
            AssistantMessageStatus::InProgress
        ));
        assert_eq!(history.total_tokens(), 10);
    }

    #[test]
    fn item_added_does_not_touch_message_timing() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Started(1)))
            .unwrap();
        history
            .handle(
                0,
                response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming::with_start(2),
                        content: vec![],
                    },
                )))),
            )
            .unwrap();
        let Some(HistoryEntry { meta, .. }) = history.entries.first() else {
            panic!("expected assistant message");
        };
        assert_eq!(meta.timing.last_modified_ms, None);
    }

    #[test]
    fn item_done_without_delta_touches_message_timing() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Started(1)))
            .unwrap();
        history
            .handle(
                0,
                response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming {
                            started_at_ms: 2,
                            last_modified_ms: Some(3),
                        },
                        content: vec![],
                    },
                )))),
            )
            .unwrap();
        let Some(HistoryEntry { meta, .. }) = history.entries.first() else {
            panic!("expected assistant message");
        };
        assert_eq!(meta.timing.last_modified_ms, Some(3));
    }

    #[test]
    fn delta_touches_message_timing() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Started(1)))
            .unwrap();
        history
            .handle(
                0,
                response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming::with_start(2),
                        content: vec![],
                    },
                )))),
            )
            .unwrap();
        history
            .handle(
                0,
                response(ResponseEvent::Delta(Delta {
                    id: "out".into(),
                    delta: crate::llm::delta::DeltaContent::Output("hello".into()),
                })),
            )
            .unwrap();
        let Some(HistoryEntry {
            meta,
            message: Message::Assistant(msg),
        }) = history.entries.first()
        else {
            panic!("expected assistant message");
        };
        let AssistantItem::Output(item) = msg.content.get("out").unwrap() else {
            panic!("expected output item");
        };
        assert_eq!(meta.timing.last_modified_ms, item.timing.last_modified_ms);
        assert_eq!(history.total_tokens(), 10 + count_text_tokens("hello"));
    }

    #[test]
    fn response_item_starts_message_in_progress() {
        let mut history = History::new();
        history
            .handle(
                0,
                response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming::new(),
                        content: vec![],
                    },
                )))),
            )
            .unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.entries.first()
        else {
            panic!("expected assistant message");
        };
        assert!(matches!(
            msg.finish_reason,
            AssistantMessageStatus::InProgress
        ));
    }

    #[test]
    fn response_completed_without_message_creates_success_message() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Completed(vec![])))
            .unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.entries.first()
        else {
            panic!("expected assistant message");
        };
        assert!(matches!(msg.finish_reason, AssistantMessageStatus::Success));
    }

    #[test]
    fn response_completed_marks_message_success() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Started(0)))
            .unwrap();
        history
            .handle(
                0,
                response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming::new(),
                        content: vec![],
                    },
                )))),
            )
            .unwrap();
        history
            .handle(
                0,
                response(ResponseEvent::Completed(vec![AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming {
                            started_at_ms: 1,
                            last_modified_ms: Some(2),
                        },
                        content: vec![],
                    },
                )])),
            )
            .unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.entries.first()
        else {
            panic!("expected assistant message");
        };
        let AssistantItem::Output(item) = msg.content.get("out").unwrap() else {
            panic!("expected output item");
        };
        assert_eq!(item.timing.last_modified_ms, None);
        assert!(matches!(msg.finish_reason, AssistantMessageStatus::Success));
    }

    #[test]
    fn response_failed_marks_message_error_for_abort() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Started(0)))
            .unwrap();
        history
            .handle(0, response(ResponseEvent::Failed("aborted by user".into())))
            .unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.entries.first()
        else {
            panic!("expected assistant message");
        };
        assert!(matches!(
            msg.finish_reason,
            AssistantMessageStatus::Error(ref text) if text == "aborted by user"
        ));
    }

    #[test]
    fn user_message_updates_token_cache() {
        let mut history = History::new();
        history
            .handle(0, HistoryUpdate::UserMessage("hello".into()))
            .unwrap();
        assert_eq!(history.total_tokens(), 10 + count_text_tokens("hello"));
    }

    #[test]
    fn generation_changes_only_when_message_count_changes() {
        let mut history = History::new();
        history
            .handle(0, response(ResponseEvent::Started(1)))
            .unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(
                0,
                response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming::with_start(2),
                        content: vec![],
                    },
                )))),
            )
            .unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(
                0,
                response(ResponseEvent::Delta(Delta {
                    id: "out".into(),
                    delta: crate::llm::delta::DeltaContent::Output("hello".into()),
                })),
            )
            .unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(0, HistoryUpdate::UserMessage("hi".into()))
            .unwrap();
        assert_eq!(history.generation(), 0);
        history.increment();
        assert_eq!(history.generation(), 1);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let mut history = History::new();
        history
            .handle(0, HistoryUpdate::UserMessage("hello".into()))
            .unwrap();
        history.increment();
        assert!(history.handle(0, HistoryUpdate::Pop(1)).is_err());
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn generation_increment_event_updates_generation() {
        let mut history = History::new();
        history
            .handle(0, HistoryUpdate::GenerationIncremented)
            .unwrap();
        assert_eq!(history.generation(), 1);
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn response_can_follow_generation_increment() {
        let mut history = History::new();
        history
            .handle(0, HistoryUpdate::UserMessage("hello".into()))
            .unwrap();
        history
            .handle(0, HistoryUpdate::GenerationIncremented)
            .unwrap();
        history
            .handle(1, response(ResponseEvent::Started(7)))
            .unwrap();
        assert_eq!(history.generation(), 1);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn generation_changes_for_external_history_edits() {
        let mut history = History::new();
        history
            .handle(
                0,
                HistoryUpdate::DeveloperMessage(DeveloperMessage::new("note".into())),
            )
            .unwrap();
        assert_eq!(history.generation(), 0);
        history.increment();
        assert_eq!(history.generation(), 1);
        history.handle(1, HistoryUpdate::Pop(1)).unwrap();
        assert_eq!(history.generation(), 1);
    }

    #[test]
    fn compact_completed_applies_and_archives_history() {
        let mut history = History::new();
        history.entries.push_message(Message::User(UserMessage {
            text: "first".into(),
        }));
        history.entries.push_message(compact_summary("old reply"));
        history.entries.push_message(Message::User(UserMessage {
            text: "last".into(),
        }));
        let generation = history.generation();

        history
            .handle(
                generation,
                HistoryUpdate::CompactStart {
                    dropped: 2,
                    needs_another_turn: false,
                },
            )
            .unwrap();
        history
            .handle(generation, compact_response(ResponseEvent::Started(0)))
            .unwrap();
        history
            .handle(
                generation,
                compact_response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming::new(),
                        content: vec![OutputContent::Text("summary".into())],
                    },
                )))),
            )
            .unwrap();
        history
            .handle(
                generation,
                compact_response(ResponseEvent::Completed(vec![AssistantItem::Output(
                    OutputItem {
                        id: "out".into(),
                        timing: ItemTiming::new(),
                        content: vec![],
                    },
                )])),
            )
            .unwrap();

        assert!(history.compact.is_none());
        assert_eq!(history.archive.len(), 1);
        assert_eq!(history.len(), 2);
        assert!(matches!(
            &history[0].message,
            Message::Developer(DeveloperMessage::Compact(CompactMessage { text, .. })) if text == "summary"
        ));
        assert!(
            matches!(&history[1].message, Message::User(UserMessage { text }) if text == "last")
        );
    }

    #[test]
    fn compact_failed_keeps_state_without_rewriting_history() {
        let mut history = History::new();
        history.entries.push_message(Message::User(UserMessage {
            text: "first".into(),
        }));
        history.entries.push_message(compact_summary("reply"));
        let generation = history.generation();
        let total_tokens = history.total_tokens();

        history
            .handle(
                generation,
                HistoryUpdate::CompactStart {
                    dropped: 1,
                    needs_another_turn: false,
                },
            )
            .unwrap();
        history
            .handle(
                generation,
                compact_response(ResponseEvent::Failed("oops".into())),
            )
            .unwrap();

        assert!(history.compact.is_some());
        assert_eq!(history.archive.len(), 0);
        assert_eq!(history.len(), 2);
        assert_eq!(history.total_tokens(), total_tokens);
        assert!(
            matches!(&history[0].message, Message::User(UserMessage { text }) if text == "first")
        );
        assert!(matches!(
            &history[1].message,
            Message::Assistant(AssistantMessage {
                finish_reason: AssistantMessageStatus::Success,
                ..
            })
        ));
    }

    #[test]
    fn compact_completed_concatenates_outputs_from_all_attempts() {
        let mut history = History::new();
        history.entries.push_message(Message::User(UserMessage {
            text: "first".into(),
        }));
        history.entries.push_message(compact_summary("reply"));
        let generation = history.generation();

        history
            .handle(
                generation,
                HistoryUpdate::CompactStart {
                    dropped: 1,
                    needs_another_turn: false,
                },
            )
            .unwrap();
        history
            .handle(generation, compact_response(ResponseEvent::Started(0)))
            .unwrap();
        history
            .handle(
                generation,
                compact_response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out-1".into(),
                        timing: ItemTiming::new(),
                        content: vec![OutputContent::Text("part 1".into())],
                    },
                )))),
            )
            .unwrap();
        history
            .handle(
                generation,
                compact_response(ResponseEvent::Failed("oops".into())),
            )
            .unwrap();
        history
            .handle(generation, compact_response(ResponseEvent::Started(0)))
            .unwrap();
        history
            .handle(
                generation,
                compact_response(ResponseEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem {
                        id: "out-2".into(),
                        timing: ItemTiming::new(),
                        content: vec![OutputContent::Text("part 2".into())],
                    },
                )))),
            )
            .unwrap();
        history
            .handle(
                generation,
                compact_response(ResponseEvent::Completed(vec![AssistantItem::Output(
                    OutputItem {
                        id: "out-2".into(),
                        timing: ItemTiming::new(),
                        content: vec![],
                    },
                )])),
            )
            .unwrap();

        assert!(matches!(
            &history[0].message,
            Message::Developer(DeveloperMessage::Compact(CompactMessage { text, .. })) if text == "part 1part 2"
        ));
    }

    #[test]
    fn compact_completed_with_empty_summary_keeps_history() {
        let mut history = History::new();
        history.entries.push_message(Message::User(UserMessage {
            text: "first".into(),
        }));
        history.entries.push_message(compact_summary("reply"));
        let generation = history.generation();
        let total_tokens = history.total_tokens();

        history
            .handle(
                generation,
                HistoryUpdate::CompactStart {
                    dropped: 1,
                    needs_another_turn: false,
                },
            )
            .unwrap();
        history
            .handle(generation, compact_response(ResponseEvent::Started(0)))
            .unwrap();

        let err = history
            .handle(
                generation,
                compact_response(ResponseEvent::Completed(vec![])),
            )
            .unwrap_err();

        assert_eq!(err.to_string(), "compact summary is empty");
        assert!(history.compact.is_none());
        assert_eq!(history.archive.len(), 0);
        assert_eq!(history.len(), 2);
        assert_eq!(history.total_tokens(), total_tokens);
        assert!(
            matches!(&history[0].message, Message::User(UserMessage { text }) if text == "first")
        );
        assert!(matches!(
            &history[1].message,
            Message::Assistant(AssistantMessage {
                finish_reason: AssistantMessageStatus::Success,
                ..
            })
        ));
    }

    #[test]
    fn collecting_entries_into_messages_returns_entry_messages() {
        let entries = vec![
            HistoryEntry::new(Message::User(UserMessage { text: "u".into() })),
            HistoryEntry::new(Message::Developer(DeveloperMessage::new("d".into()))),
        ];

        let owned: Vec<Message> = entries.clone().into_iter().collect();
        let borrowed: Vec<Message> = entries.iter().collect();

        assert!(matches!(
            owned.as_slice(),
            [Message::User(UserMessage { text }), Message::Developer(DeveloperMessage::Misc(dev))]
            if text == "u" && dev == "d"
        ));
        assert!(matches!(
            borrowed.as_slice(),
            [Message::User(UserMessage { text }), Message::Developer(DeveloperMessage::Misc(dev))]
            if text == "u" && dev == "d"
        ));
    }

    // TODO uncomment and fix stale tests

    // #[test]
    // fn compact_dropped_keeps_target_budget() {
    //     let history = History::from_messages(vec![
    //         Message::User(UserMessage {
    //             text: "a".repeat(100),
    //         }),
    //         Message::User(UserMessage {
    //             text: "b".repeat(100),
    //         }),
    //     ]);
    //     let dropped = history.compact_dropped(history.total_tokens(), 50);
    //     let kept: usize = history.messages[dropped..]
    //         .iter()
    //         .map(|entry| entry.meta.token_count)
    //         .sum();
    //     let previous: usize = history.messages[dropped.saturating_sub(1)..]
    //         .iter()
    //         .map(|entry| entry.meta.token_count)
    //         .sum();
    //     assert!(kept < history.total_tokens() / 2);
    //     if dropped > 0 {
    //         assert!(previous >= history.total_tokens() / 2);
    //     }
    // }

    // #[test]
    // fn finish_compact_archives_and_rewrites_history() {
    //     let mut history = History::from_messages(vec![
    //         Message::User(UserMessage {
    //             text: "first".into(),
    //         }),
    //         Message::Assistant(AssistantMessage::default()),
    //         Message::User(UserMessage {
    //             text: "last".into(),
    //         }),
    //     ]);
    //     let generation = history.generation();
    //     history.start_compact(2, false);
    //     history
    //         .compact
    //         .as_mut()
    //         .unwrap()
    //         .messages
    //         .push_message(compact_summary("summary"));
    //     history.apply_compact().unwrap();
    //
    //     assert_eq!(history.archive.len(), 1);
    //     assert!(history.compact.is_none());
    //     assert_eq!(history.messages.len(), 2);
    //     assert!(matches!(
    //         history.messages[0].message,
    //         Message::Developer(DeveloperMessage { ref text }) if text == "summary"
    //     ));
    //     assert!(matches!(history.messages[1].message, Message::User(_)));
    //     assert!(history.total_tokens() > 0);
    //     assert_eq!(history.generation(), generation + 1);
    // }

    // #[test]
    // fn compact_failure_keeps_main_transcript_and_records_error() {
    //     let mut history = History::from_messages(vec![
    //         Message::User(UserMessage {
    //             text: "first".into(),
    //         }),
    //         Message::Assistant(AssistantMessage {
    //             finish_reason: AssistantMessageStatus::Success,
    //             ..Default::default()
    //         }),
    //     ]);
    //     let generation = history.generation();
    //     let total_tokens = history.total_tokens();
    //     history.start_compact(1, false);
    //
    //     assert_eq!(
    //         history
    //             .handle(
    //                 generation,
    //                 compact_response(ResponseEvent::Failed("oops".into()))
    //             )
    //             .unwrap(),
    //         0
    //     );
    //     let compact = history.compact.as_ref().unwrap();
    //     assert_eq!(history.len(), 2);
    //     assert_eq!(history.total_tokens(), total_tokens);
    //     assert_eq!(history.generation(), generation);
    //     assert!(matches!(
    //         compact.messages.last().map(|entry| &entry.message),
    //         Some(Message::Assistant(AssistantMessage {
    //             finish_reason: AssistantMessageStatus::Error(text),
    //             ..
    //         })) if text == "oops"
    //     ));
    //     assert!(matches!(
    //         history.last().map(|entry| &entry.message),
    //         Some(Message::Assistant(AssistantMessage {
    //             finish_reason: AssistantMessageStatus::Success,
    //             ..
    //         }))
    //     ));
    // }
}
