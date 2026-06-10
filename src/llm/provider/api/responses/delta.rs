use async_openai::types::responses;

use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;

impl From<responses::ResponseReasoningTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseReasoningTextDeltaEvent) -> Self {
        Self::new(event.item_id, DeltaContent::Reasoning(event.delta))
    }
}

impl From<responses::ResponseReasoningSummaryTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseReasoningSummaryTextDeltaEvent) -> Self {
        Self::new(event.item_id, DeltaContent::ReasoningSummary(event.delta))
    }
}

impl From<responses::ResponseTextDeltaEvent> for Delta {
    fn from(event: responses::ResponseTextDeltaEvent) -> Self {
        Self::new(event.item_id, DeltaContent::Output(event.delta))
    }
}
