use async_openai::types::responses;

use crate::llm::delta::Delta;
use crate::llm::delta::DeltaContent;

impl From<responses::ResponseReasoningTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseReasoningTextDeltaEvent) -> Self {
        Delta {
            id: event.item_id,
            delta: DeltaContent::Reasoning(event.delta),
        }
    }
}

impl From<responses::ResponseReasoningSummaryTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseReasoningSummaryTextDeltaEvent) -> Self {
        Delta {
            id: event.item_id,
            delta: DeltaContent::ReasoningSummary(event.delta),
        }
    }
}

impl From<responses::ResponseTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseTextDeltaEvent) -> Self {
        Delta {
            id: event.item_id,
            delta: DeltaContent::Output(event.delta),
        }
    }
}
