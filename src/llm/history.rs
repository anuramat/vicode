use anyhow::Result;
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
use crate::tui::widgets::container::composite::CompositeElement;
use crate::tui::widgets::container::element::Element;

#[derive(Default, Clone, Serialize, Deserialize, Debug)]
pub struct History {
    #[serde(skip)]
    generation: HistoryGeneration,
    messages: Vec<HistoryEntry>,
}

pub type HistoryGeneration = u64;

#[derive(Debug, Clone)]
pub enum HistoryEvent {
    GenerationIncremented,
    /// timestamp of start of response in ms
    ResponseStarted(u64),
    ResponseDelta(Delta),
    ResponseItem(Box<AssistantItem>),
    ResponseCompleted(Vec<AssistantItem>),
    ResponseAborted,
    ResponseFailed(String),
    UserMessage(String),
    DeveloperMessage(String),
    Pop(usize),
}

impl AsRef<[HistoryEntry]> for History {
    fn as_ref(&self) -> &[HistoryEntry] {
        &self.messages
    }
}

impl History {
    pub fn messages(self) -> Vec<Message> {
        self.messages
            .into_iter()
            .map(|entry| entry.message)
            .collect()
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn increment(&mut self) {
        self.generation += 1;
    }

    pub fn generation(&self) -> HistoryGeneration {
        self.generation
    }

    pub fn rebuild_token_cache(&mut self) {
        self.messages.iter_mut().for_each(|entry| {
            entry.meta.token_count = count_message_tokens(&entry.message);
        });
    }

    pub fn total_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|entry| entry.meta.token_count)
            .sum()
    }

    // TODO inline this everywhere?
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn last(&mut self) -> Option<&mut HistoryEntry> {
        self.messages.last_mut()
    }

    pub fn needs_another_turn(&self) -> bool {
        if let Some(entry) = self.messages.last() {
            match &entry.message {
                Message::Assistant(msg) => msg.content.iter().any(|(_, content)| {
                    content.try_as_tool_call_ref().is_some()
                        && matches!(msg.finish_reason, AssistantMessageStatus::Success)
                }),
                Message::Developer(_) => true,
                Message::User(_) => false,
            }
        } else {
            false
        }
    }

    /// returns change in token count after applying the event
    pub fn handle(
        &mut self,
        generation: HistoryGeneration,
        event: HistoryEvent,
    ) -> Result<isize> {
        anyhow::ensure!(
            generation == self.generation,
            "History event generation {} does not match current generation {} in {:?}",
            generation,
            self.generation,
            self.messages
        );
        // XXX verify recount_last_message logic
        let delta = match event {
            HistoryEvent::GenerationIncremented => {
                self.increment();
                0
            }
            HistoryEvent::ResponseStarted(started_at_ms) => {
                self.start_response(started_at_ms);
                self.recount_last_message()
            }
            HistoryEvent::ResponseDelta(item_delta) => {
                self.push_delta(item_delta);
                self.recount_last_message()
            }
            HistoryEvent::ResponseItem(item) => {
                self.push_item(*item);
                self.recount_last_message()
            }
            HistoryEvent::ResponseCompleted(items) => {
                self.complete_response(items);
                self.recount_last_message()
            }
            HistoryEvent::ResponseAborted => {
                self.abort_response();
                self.recount_last_message()
            }
            HistoryEvent::ResponseFailed(msg) => {
                self.fail_response(msg);
                self.recount_last_message()
            }
            HistoryEvent::DeveloperMessage(text) => {
                let msg = Message::Developer(DeveloperMessage { text });
                self.messages.push(HistoryEntry {
                    meta: MessageMeta::default(),
                    message: msg,
                });
                self.recount_last_message()
            }
            HistoryEvent::UserMessage(text) => {
                let msg = Message::User(UserMessage { text });
                self.messages.push(HistoryEntry {
                    meta: MessageMeta::default(),
                    message: msg,
                });
                self.recount_last_message()
            }
            HistoryEvent::Pop(n) => {
                let len = self.messages.len();
                anyhow::ensure!(
                    n <= len,
                    "Cannot pop {} messages from history of length {}",
                    n,
                    len
                );
                let popped = self.messages.split_off(len - n);
                let delta = -popped
                    .iter()
                    .map(|entry| entry.meta.token_count as isize)
                    .sum::<isize>();
                delta
            }
        };
        Ok(delta)
    }

    pub fn push_item(
        &mut self,
        mut item: AssistantItem,
    ) {
        let item_modified = item.timing().last_modified_ms;
        let item_started = item.timing().started_at_ms;
        if let Some(Message::Assistant(msg)) =
            self.messages.last_mut().map(|entry| &mut entry.message)
        {
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
            self.messages.push(HistoryEntry {
                meta: MessageMeta {
                    timing: ItemTiming::with_start(item_started),
                    ..Default::default()
                },
                message: msg.into(),
            });
        }
        if let Some(modified) = item_modified {
            self.messages
                .last_mut()
                .unwrap()
                .meta
                .timing
                .touch_at(modified);
        }
    }

    pub fn start_response(
        &mut self,
        started_at_ms: u64,
    ) {
        self.messages.push(HistoryEntry {
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
        if let Some(Message::Assistant(msg)) =
            self.messages.last_mut().map(|entry| &mut entry.message)
        {
            msg.finish_reason = AssistantMessageStatus::Success;
        } else {
            // XXX does this ever happen?
            self.messages.push(HistoryEntry {
                meta: MessageMeta::default(),
                message: AssistantMessage {
                    finish_reason: AssistantMessageStatus::Success,
                    content: indexmap! {},
                }
                .into(),
            });
        }
    }

    pub fn abort_response(&mut self) {
        let Some(Message::Assistant(msg)) =
            self.messages.last_mut().map(|entry| &mut entry.message)
        else {
            return;
        };
        match msg.finish_reason {
            AssistantMessageStatus::InProgress => {
                msg.finish_reason = AssistantMessageStatus::AbortedByUser
            }
            AssistantMessageStatus::Success => {
                if !self.needs_another_turn() {
                    return;
                }
                // we're trying to abort right before the next turn -- pretend it already started
                self.messages.push(HistoryEntry {
                    meta: MessageMeta::default(),
                    message: AssistantMessage {
                        finish_reason: AssistantMessageStatus::AbortedByUser,
                        content: indexmap! {},
                    }
                    .into(),
                });
            }
            _ => {}
        }
    }

    pub fn fail_response(
        &mut self,
        error_text: String, // TODO rename to msg or whatever
    ) {
        if let Some(Message::Assistant(msg)) =
            self.messages.last_mut().map(|entry| &mut entry.message)
        {
            msg.finish_reason = AssistantMessageStatus::Error(error_text);
        } else {
            self.messages.push(HistoryEntry {
                meta: MessageMeta::default(),
                message: AssistantMessage {
                    finish_reason: AssistantMessageStatus::Error(error_text),
                    content: indexmap! {},
                }
                .into(),
            });
        }
    }

    pub fn last_output(&self) -> Result<String> {
        if let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = self.messages.last()
        {
            Ok(msg.output())
        } else {
            Err(anyhow::anyhow!("last message is not from the assistant",))
        }
    }

    // XXX rename
    pub fn recount_last_message(&mut self) -> isize {
        let Some(entry) = self.messages.last_mut() else {
            return 0;
        };
        let new = count_message_tokens(&entry.message);
        let old = std::mem::replace(&mut entry.meta.token_count, new);
        new as isize - old as isize
    }
}

impl From<&History> for Vec<Message> {
    fn from(history: &History) -> Self {
        history
            .messages
            .iter()
            .map(|entry| entry.message.clone())
            .collect()
    }
}

impl From<&History> for CompositeElement {
    fn from(history: &History) -> Self {
        let vec: Vec<Element> = history
            .messages
            .iter()
            .map(|entry| (&entry.message).into())
            .collect();
        CompositeElement(vec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::message::OutputItem;
    use crate::llm::tokens::count_text_tokens;

    #[test]
    fn response_starts_without_assistant_message() {
        let history = History::new();
        assert!(history.messages.is_empty());
    }

    #[test]
    fn response_failed_without_message_creates_error_message() {
        let mut history = History::new();
        history
            .handle(0, HistoryEvent::ResponseFailed("oops".into()))
            .unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.messages.first()
        else {
            panic!("expected assistant message");
        };
        assert!(matches!(
            msg.finish_reason,
            AssistantMessageStatus::Error(ref text) if text == "oops"
        ));
    }

    #[test]
    fn response_aborted_without_message_is_noop() {
        let mut history = History::new();
        history.handle(0, HistoryEvent::ResponseAborted).unwrap();
        assert!(history.messages.is_empty());
    }

    #[test]
    fn response_started_creates_empty_assistant_message() {
        let mut history = History::new();
        history.handle(0, HistoryEvent::ResponseStarted(7)).unwrap();
        let Some(HistoryEntry {
            meta,
            message: Message::Assistant(msg),
        }) = history.messages.first()
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
        history.handle(0, HistoryEvent::ResponseStarted(1)).unwrap();
        history
            .handle(
                0,
                HistoryEvent::ResponseItem(Box::new(AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming::with_start(2),
                    content: vec![],
                }))),
            )
            .unwrap();
        let Some(HistoryEntry { meta, .. }) = history.messages.first() else {
            panic!("expected assistant message");
        };
        assert_eq!(meta.timing.last_modified_ms, None);
    }

    #[test]
    fn item_done_without_delta_touches_message_timing() {
        let mut history = History::new();
        history.handle(0, HistoryEvent::ResponseStarted(1)).unwrap();
        history
            .handle(
                0,
                HistoryEvent::ResponseItem(Box::new(AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming {
                        started_at_ms: 2,
                        last_modified_ms: Some(3),
                    },
                    content: vec![],
                }))),
            )
            .unwrap();
        let Some(HistoryEntry { meta, .. }) = history.messages.first() else {
            panic!("expected assistant message");
        };
        assert_eq!(meta.timing.last_modified_ms, Some(3));
    }

    #[test]
    fn delta_touches_message_timing() {
        let mut history = History::new();
        history.handle(0, HistoryEvent::ResponseStarted(1)).unwrap();
        history
            .handle(
                0,
                HistoryEvent::ResponseItem(Box::new(AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming::with_start(2),
                    content: vec![],
                }))),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryEvent::ResponseDelta(Delta {
                    id: "out".into(),
                    delta: crate::llm::delta::DeltaContent::Output("hello".into()),
                }),
            )
            .unwrap();
        let Some(HistoryEntry {
            meta,
            message: Message::Assistant(msg),
        }) = history.messages.first()
        else {
            panic!("expected assistant message");
        };
        let item = msg.content.get("out").unwrap().try_as_output_ref().unwrap();
        assert_eq!(meta.timing.last_modified_ms, item.timing.last_modified_ms);
        assert_eq!(history.total_tokens(), 10 + count_text_tokens("hello"));
    }

    #[test]
    fn response_item_starts_message_in_progress() {
        let mut history = History::new();
        history
            .handle(
                0,
                HistoryEvent::ResponseItem(Box::new(AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming::new(),
                    content: vec![],
                }))),
            )
            .unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.messages.first()
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
            .handle(0, HistoryEvent::ResponseCompleted(vec![]))
            .unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.messages.first()
        else {
            panic!("expected assistant message");
        };
        assert!(matches!(msg.finish_reason, AssistantMessageStatus::Success));
    }

    #[test]
    fn response_completed_marks_message_success() {
        let mut history = History::new();
        history.handle(0, HistoryEvent::ResponseStarted(0)).unwrap();
        history
            .handle(
                0,
                HistoryEvent::ResponseItem(Box::new(AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming::new(),
                    content: vec![],
                }))),
            )
            .unwrap();
        history
            .handle(
                0,
                HistoryEvent::ResponseCompleted(vec![AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming {
                        started_at_ms: 1,
                        last_modified_ms: Some(2),
                    },
                    content: vec![],
                })]),
            )
            .unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.messages.first()
        else {
            panic!("expected assistant message");
        };
        let item = msg.content.get("out").unwrap().try_as_output_ref().unwrap();
        assert_eq!(item.timing.last_modified_ms, None);
        assert!(matches!(msg.finish_reason, AssistantMessageStatus::Success));
    }

    #[test]
    fn response_aborted_marks_message_aborted() {
        let mut history = History::new();
        history.handle(0, HistoryEvent::ResponseStarted(0)).unwrap();
        history.handle(0, HistoryEvent::ResponseAborted).unwrap();
        let Some(HistoryEntry {
            message: Message::Assistant(msg),
            ..
        }) = history.messages.first()
        else {
            panic!("expected assistant message");
        };
        assert!(matches!(
            msg.finish_reason,
            AssistantMessageStatus::AbortedByUser
        ));
    }

    #[test]
    fn user_message_updates_token_cache() {
        let mut history = History::new();
        history
            .handle(0, HistoryEvent::UserMessage("hello".into()))
            .unwrap();
        assert_eq!(history.total_tokens(), 10 + count_text_tokens("hello"));
    }

    #[test]
    fn generation_changes_only_when_message_count_changes() {
        let mut history = History::new();
        history.handle(0, HistoryEvent::ResponseStarted(1)).unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(
                0,
                HistoryEvent::ResponseItem(Box::new(AssistantItem::Output(OutputItem {
                    id: "out".into(),
                    timing: ItemTiming::with_start(2),
                    content: vec![],
                }))),
            )
            .unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(
                0,
                HistoryEvent::ResponseDelta(Delta {
                    id: "out".into(),
                    delta: crate::llm::delta::DeltaContent::Output("hello".into()),
                }),
            )
            .unwrap();
        assert_eq!(history.generation(), 0);
        history
            .handle(0, HistoryEvent::UserMessage("hi".into()))
            .unwrap();
        assert_eq!(history.generation(), 0);
        history.increment();
        assert_eq!(history.generation(), 1);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let mut history = History::new();
        history
            .handle(0, HistoryEvent::UserMessage("hello".into()))
            .unwrap();
        history.increment();
        assert!(history.handle(0, HistoryEvent::Pop(1)).is_err());
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn generation_increment_event_updates_generation() {
        let mut history = History::new();
        history
            .handle(0, HistoryEvent::GenerationIncremented)
            .unwrap();
        assert_eq!(history.generation(), 1);
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn response_can_follow_generation_increment() {
        let mut history = History::new();
        history
            .handle(0, HistoryEvent::UserMessage("hello".into()))
            .unwrap();
        history
            .handle(0, HistoryEvent::GenerationIncremented)
            .unwrap();
        history.handle(1, HistoryEvent::ResponseStarted(7)).unwrap();
        assert_eq!(history.generation(), 1);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn generation_changes_for_external_history_edits() {
        let mut history = History::new();
        history
            .handle(0, HistoryEvent::DeveloperMessage("note".into()))
            .unwrap();
        assert_eq!(history.generation(), 0);
        history.increment();
        assert_eq!(history.generation(), 1);
        history.handle(1, HistoryEvent::Pop(1)).unwrap();
        assert_eq!(history.generation(), 1);
    }

    #[test]
    fn load_without_token_cache_rebuilds_on_demand() {
        let mut history: History = serde_json::from_value(serde_json::json!({
            "messages": [{
                "role": "user",
                "text": "hello"
            }]
        }))
        .unwrap();
        history.rebuild_token_cache();
        assert_eq!(history.total_tokens(), 10 + count_text_tokens("hello"));
    }
}
