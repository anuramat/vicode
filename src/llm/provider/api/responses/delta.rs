use async_openai::types::responses;

use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::utils::now;

impl From<responses::ResponseReasoningTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseReasoningTextDeltaEvent) -> Self {
        Self {
            id: event.item_id,
            delta: DeltaContent::Reasoning(event.delta),
            received_at: now(),
        }
    }
}

impl From<responses::ResponseReasoningSummaryTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseReasoningSummaryTextDeltaEvent) -> Self {
        Self {
            id: event.item_id,
            delta: DeltaContent::ReasoningSummary(event.delta),
            received_at: now(),
        }
    }
}

impl From<responses::ResponseTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseTextDeltaEvent) -> Self {
        Self {
            id: event.item_id,
            delta: DeltaContent::Output(event.delta),
            received_at: now(),
        }
    }
}
