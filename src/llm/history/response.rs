use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::llm::history::HistoryState;
use crate::llm::history::event::AssistantEvent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::AssistantMessage;
use crate::llm::history::message::AssistantStatus;
use crate::llm::history::timing::Timing;
use crate::llm::history::tokens::TokenCount;

impl HistoryState {
    pub fn handle_response(
        &mut self,
        event: AssistantEvent,
    ) -> Result<()> {
        match event {
            AssistantEvent::Created { created_at } => {
                self.push(AssistantMessage::new(created_at).into());
            }
            AssistantEvent::Started { started_at } => {
                self.last_assistant_msg_mut()?.mark_started(started_at)?;
            }
            AssistantEvent::Delta(item_delta) => {
                self.push_delta(item_delta)?;
            }
            AssistantEvent::Item(item) => {
                self.push_item(*item)?;
            }
            AssistantEvent::Completed { ended_at } => {
                self.complete_response(ended_at)?;
            }
            AssistantEvent::Failed { message, ended_at } => {
                self.fail_response(message, ended_at)?;
            }
        }
        Ok(())
    }

    pub fn push_item(
        &mut self,
        mut new_item: AssistantItem,
    ) -> Result<()> {
        // HACK ideally we would somehow make sure that we initialize token count properly
        new_item.recount();
        let msg = self.last_assistant_msg_mut()?;

        // if item already exists -- preserve started_at, keep the latest ended_at
        if let Some(existing_item) = msg.content.get(&new_item.id()) {
            if let Some(started_at) = existing_item.started_at() {
                new_item.set_started_at(started_at);
            }
            if let Some(ended_at) = existing_item.ended_at() {
                new_item.touch_ended_at(ended_at);
            }
        }

        // touch the message
        if let Some(ended_at) = new_item.ended_at() {
            msg.touch_ended_at(ended_at);
        }
        if let Some(ready_at) = new_item.ready_at() {
            msg.touch_ready_at(ready_at);
        }

        drop(msg.content.insert(new_item.id(), new_item));
        msg.recount_shallow();
        self.recount_shallow();
        Ok(())
    }

    pub fn complete_response(
        &mut self,
        ended_at: u64,
    ) -> Result<()> {
        let msg = self.last_assistant_msg_mut()?;
        msg.touch_ended_at(ended_at);
        msg.status = AssistantStatus::Success;
        Ok(())
    }

    pub fn fail_response(
        &mut self,
        error_msg: String,
        ended_at: u64,
    ) -> Result<()> {
        let msg = self.last_assistant_msg_mut()?;
        msg.touch_ended_at(ended_at);
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
    use crate::llm::history::timing::Timing;
    use crate::llm::history::tokens::TokenCount;
    use crate::llm::history::tokens::count_text_tokens;

    fn response(event: AssistantEvent) -> HistoryUpdate {
        HistoryUpdate::TurnResponse(event)
    }

    fn completed() -> AssistantEvent {
        AssistantEvent::Completed { ended_at: 9 }
    }

    fn failed(message: &str) -> AssistantEvent {
        AssistantEvent::Failed {
            message: message.into(),
            ended_at: 9,
        }
    }

    fn output_item_at(
        id: &str,
        started_at: u64,
        ended_at: Option<u64>,
    ) -> AssistantItem {
        AssistantItem::Output(OutputItem {
            id: id.into(),
            started_at,
            ended_at,
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
    fn response_queued_creates_empty_assistant_message() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created { created_at: 7 }))
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
            .handle(0, response(AssistantEvent::Created { created_at: 7 }))
            .unwrap();
        history
            .handle(0, response(AssistantEvent::Started { started_at: 9 }))
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
            .handle(0, response(AssistantEvent::Created { created_at: 1 }))
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
            .handle(0, response(AssistantEvent::Created { created_at: 1 }))
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
    fn item_replacement_preserves_started_at_and_keeps_latest_ended_at() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created { created_at: 1 }))
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out",
                    2,
                    Some(9),
                )))),
            )
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out",
                    5,
                    Some(7),
                )))),
            )
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        let AssistantItem::Output(item) = msg.content.get("out").unwrap() else {
            panic!("expected output item");
        };
        assert_eq!((item.started_at, item.ended_at), (2, Some(9)));

        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out",
                    6,
                    Some(11),
                )))),
            )
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        let AssistantItem::Output(item) = msg.content.get("out").unwrap() else {
            panic!("expected output item");
        };
        assert_eq!((item.started_at, item.ended_at), (2, Some(11)));
    }

    #[test]
    fn delta_touches_message_timing() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created { created_at: 1 }))
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
                    timestamp: 3,
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
    fn response_completed_marks_message_success() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created { created_at: 0 }))
            .unwrap();
        history
            .handle(
                0,
                response(AssistantEvent::Item(Box::new(output_item_at(
                    "out", 0, None,
                )))),
            )
            .unwrap();
        history.handle(0, response(completed())).unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        let AssistantItem::Output(item) = msg.content.get("out").unwrap() else {
            panic!("expected output item");
        };
        assert_eq!(item.ended_at, None);
        assert_eq!(msg.ended_at, Some(9));
        assert!(matches!(msg.status, AssistantStatus::Success));
    }

    #[test]
    fn response_failed_marks_message_error_for_abort() {
        let mut history = History::new(String::new());
        history
            .handle(0, response(AssistantEvent::Created { created_at: 0 }))
            .unwrap();
        history
            .handle(0, response(failed("aborted by user")))
            .unwrap();
        let Some(Message::Assistant(msg)) = history.state().messages.first() else {
            panic!("expected assistant message");
        };
        assert!(matches!(
            msg.status,
            AssistantStatus::Error(ref text) if text == "aborted by user"
        ));
        assert_eq!(msg.ended_at, Some(9));
    }
}
