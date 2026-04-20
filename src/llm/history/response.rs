use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::llm::history::HistoryState;
use crate::llm::history::event::AssistantEvent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::AssistantMessage;
use crate::llm::history::message::AssistantStatus;
use crate::llm::history::timing::Timing;
use crate::llm::history::timing::now;
use crate::llm::history::tokens::TokenCount;

impl HistoryState {
    pub fn handle_response(
        &mut self,
        event: AssistantEvent,
    ) -> Result<()> {
        match event {
            AssistantEvent::Created(created_at) => {
                self.push(AssistantMessage::new(created_at).into());
            }
            AssistantEvent::Started(api_started_ms) => {
                self.last_assistant_msg_mut()?
                    .mark_started(api_started_ms)?;
            }
            AssistantEvent::Delta(item_delta) => {
                self.push_delta(item_delta)?;
            }
            AssistantEvent::Item(item) => {
                self.push_item(*item)?;
            }
            AssistantEvent::Completed(items) => {
                self.complete_response(items)?;
            }
            AssistantEvent::Failed(msg) => {
                self.fail_response(msg)?;
            }
        }
        self.recount_shallow();
        Ok(())
    }

    pub fn push_item(
        &mut self,
        mut new_item: AssistantItem,
    ) -> Result<()> {
        // HACK ideally we would somehow make sure that we initialize token count properly
        new_item.recount();
        let msg = self.last_assistant_msg_mut()?;

        // if item already exists -- preserve timing
        if let Some(existing_item) = msg.content.get(&new_item.id()) {
            if let Some(started_at) = existing_item.started_at() {
                new_item.set_started_at(started_at);
            }
            if let Some(ended_at) = existing_item.ended_at() {
                new_item.set_ended_at(ended_at);
            }
        }

        // touch the message
        if let Some(ended_at) = new_item.ended_at() {
            msg.touch_ended_at(ended_at);
        }
        if let Some(ready_at) = new_item.ready_at() {
            msg.touch_ready_at(ready_at);
        }

        _ = msg.content.insert(new_item.id(), new_item);
        msg.recount_shallow();
        Ok(())
    }

    pub fn complete_response(
        &mut self,
        _items: Vec<AssistantItem>,
    ) -> Result<()> {
        let msg = self.last_assistant_msg_mut()?;
        msg.touch_ended_at(now());
        msg.status = AssistantStatus::Success;
        Ok(())
    }

    pub fn fail_response(
        &mut self,
        error_msg: String,
    ) -> Result<()> {
        let msg = self.last_assistant_msg_mut()?;
        msg.touch_ended_at(now());
        match msg.status {
            AssistantStatus::Error(_) => return Ok(()), // keep the first error
            AssistantStatus::Success => {
                bail!("cannot fail a successful response");
            }
            AssistantStatus::Queued | AssistantStatus::InProgress => {
                msg.status = AssistantStatus::Error(error_msg);
            }
        }
        Ok(())
    }

    pub fn status(&self) -> Option<AssistantStatus> {
        self.last()
            .and_then(|v| v.try_as_assistant_ref())
            .map(|msg| &msg.status)
            .cloned()
    }

    fn last_assistant_msg_mut(&mut self) -> Result<&mut AssistantMessage> {
        self.last_mut()
            .and_then(|v| v.try_as_assistant_mut())
            .context("no last assistant message")
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use crate::llm::history::AssistantEvent;
    use crate::llm::history::History;
    use crate::llm::history::HistoryUpdate;
    use crate::llm::history::delta::Delta;
    use crate::llm::history::message::AssistantItem;
    use crate::llm::history::message::AssistantStatus;
    use crate::llm::history::message::Message;
    use crate::llm::history::message::OutputItem;
    use crate::llm::history::message::UserMessage;
    use crate::llm::history::timing::Timing;
    use crate::llm::history::tokens::TokenCount;
    use crate::llm::history::tokens::count_text_tokens;

    fn response(event: AssistantEvent) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(event)
    }

    fn output_item_at(
        id: &str,
        queued_ms: u64,
        last_modified_ms: Option<u64>,
    ) -> AssistantItem {
        AssistantItem::Output(OutputItem {
            id: id.into(),
            started_at: queued_ms,
            ended_at: last_modified_ms,
            token_count: 0,
            content: vec![],
        })
    }

    #[test]
    fn response_starts_without_assistant_message() {
        let history = History::new(String::new());
        assert!(history.state().messages.is_empty());
    }

    #[test]
    fn response_failed_without_message_keeps_history_empty() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Failed("oops".into())))
            .unwrap();
        assert!(history.state().messages.is_empty());
    }

    #[test]
    fn response_failed_without_message_for_abort_keeps_history_empty() {
        let mut history = History::new(String::new());
        history
            .handle(
                0,
                response(AssistantEvent::Failed("aborted by user".into())),
            )
            .unwrap();
        assert!(history.state().messages.is_empty());
    }

    #[test]
    fn response_failed_after_user_message_keeps_history_unchanged() {
        let mut history = History::new(String::new());
        history
            .handle(0, HistoryUpdate::UserMessage("hello".into()))
            .unwrap();
        let total_tokens = history.state().token_count();
        history
            .handle(0, response(AssistantEvent::Failed("oops".into())))
            .unwrap();
        assert!(matches!(
            history.state().last(),
            Some(Message::User(UserMessage { text, .. })) if text == "hello"
        ));
        assert_eq!(history.state().token_count(), total_tokens);
    }

    #[test]
    fn response_queued_creates_empty_assistant_message() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created(7)))
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        assert!(msg.content.is_empty());
        assert_eq!(msg.created_at(), 7);
        assert_eq!(msg.started_at, None);
        assert_eq!(msg.ended_at(), None);
        assert!(matches!(msg.status, AssistantStatus::Queued));
        assert_eq!(history.state().token_count(), 10);
    }

    #[test]
    fn response_started_flips_message_to_in_progress() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created(7)))
            .unwrap();
        history
            .handle(0, response(AssistantEvent::Started(9)))
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        assert_eq!(msg.created_at(), 7);
        assert_eq!(msg.started_at, Some(9));
        assert!(matches!(msg.status, AssistantStatus::InProgress));
    }

    #[test]
    fn item_added_does_not_touch_message_timing() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created(1)))
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out", 2, None,
                )))),
            )
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        assert_eq!(msg.ended_at, None);
    }

    #[test]
    fn item_done_without_delta_touches_message_timing() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created(1)))
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out",
                    2,
                    Some(3),
                )))),
            )
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        assert_eq!(msg.ended_at, Some(3));
    }

    #[test]
    fn delta_touches_message_timing() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created(1)))
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out", 2, None,
                )))),
            )
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Delta(Delta {
                    id: "out".into(),
                    delta: crate::llm::history::delta::DeltaContent::Output("hello".into()),
                })),
            )
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        let AssistantItem::Output(item) = msg.content.get("out").unwrap() else {
            panic!("expected output item");
        };
        assert_eq!(msg.ended_at, item.ended_at);
        assert_eq!(item.token_count, count_text_tokens("hello"));
        assert_eq!(
            history.state().token_count(),
            10 + count_text_tokens("hello")
        );
    }

    #[test]
    fn response_item_starts_message_in_progress() {
        let mut history = History::new(String::new());
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out", 0, None,
                )))),
            )
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        assert!(matches!(msg.status, AssistantStatus::InProgress));
    }

    #[test]
    fn response_completed_without_message_creates_success_message() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Completed(vec![])))
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        assert!(matches!(msg.status, AssistantStatus::Success));
    }

    #[test]
    fn response_completed_marks_message_success() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created(0)))
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out", 0, None,
                )))),
            )
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Completed(vec![output_item_at(
                    "out",
                    1,
                    Some(2),
                )])),
            )
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        let AssistantItem::Output(item) = msg.content.get("out").unwrap() else {
            panic!("expected output item");
        };
        assert_eq!(item.ended_at, None);
        assert!(matches!(msg.status, AssistantStatus::Success));
    }

    #[test]
    fn response_failed_marks_message_error_for_abort() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created(0)))
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Failed("aborted by user".into())),
            )
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        assert!(matches!(
            msg.status,
            AssistantStatus::Error(ref text) if text == "aborted by user"
        ));
    }
}
