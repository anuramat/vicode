use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::Context as TaskContext;
use std::task::Poll;

use anyhow::Result;
use async_openai::error::OpenAIError;
use async_openai::types::chat::ChatChoiceStream;
use async_openai::types::chat::ChatCompletionMessageToolCall;
use async_openai::types::chat::ChatCompletionMessageToolCalls;
use async_openai::types::chat::CreateChatCompletionStreamResponse;
use async_openai::types::chat::FinishReason;
use async_openai::types::chat::FunctionCall;
use futures::Stream;
use serde_json::Value;
use tokio::sync::OwnedSemaphorePermit;

use crate::llm::history::AssistantEvent;
use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::OutputItem;
use crate::llm::history::message::ReasoningItem;
use crate::utils::now;

fn output_id(
    response_id: &str,
    choice: u32,
) -> String {
    format!("{response_id}:message:{choice}")
}

pub struct ChatCompletionsStream {
    inner: Pin<Box<dyn Stream<Item = Result<Value, OpenAIError>> + Send>>,
    state: StreamState,
    pending: VecDeque<Result<AssistantEvent>>,
    _permit: OwnedSemaphorePermit,
}

impl ChatCompletionsStream {
    pub fn new(
        inner: Pin<Box<dyn Stream<Item = Result<Value, OpenAIError>> + Send>>,
        permit: OwnedSemaphorePermit,
        reasoning_content_field: String,
    ) -> Self {
        Self {
            inner,
            state: StreamState {
                reasoning_content_field,
                ..Default::default()
            },
            pending: VecDeque::new(),
            _permit: permit,
        }
    }
}

impl Stream for ChatCompletionsStream {
    type Item = Result<AssistantEvent, anyhow::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Poll::Ready(Some(event));
            }

            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(value))) => {
                    tracing::debug!(response = %value);
                    match serde_json::from_value::<CreateChatCompletionStreamResponse>(
                        value.clone(),
                    ) {
                        Ok(chunk) => {
                            let events = self.state.handle_chunk(chunk, Some(&value));
                            self.pending.extend(events.into_iter().map(Ok));
                        }
                        Err(e) => return Poll::Ready(Some(Err(anyhow::Error::from(e)))),
                    }
                }
                Poll::Ready(Some(Err(err))) => {
                    return Poll::Ready(Some(Err(anyhow::Error::from(err))));
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[derive(Default)]
pub struct StreamState {
    outputs: BTreeMap<u32, bool>,
    reasoning_outputs: BTreeMap<u32, bool>,
    reasoning_content_field: String,
    tool_calls: BTreeMap<u32, BTreeMap<u32, PendingToolCall>>,
}

#[derive(Default)]
struct PendingToolCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl StreamState {
    pub fn handle_chunk(
        &mut self,
        chunk: CreateChatCompletionStreamResponse,
        raw: Option<&Value>,
    ) -> Vec<AssistantEvent> {
        let mut events = Vec::new();
        for (i, choice) in chunk.choices.into_iter().enumerate() {
            let raw_delta = raw.map(|r| &r["choices"][i]["delta"]);
            events.extend(self.handle_choice(&chunk.id, choice, raw_delta));
        }
        events
    }

    fn handle_choice(
        &mut self,
        response_id: &str,
        choice: ChatChoiceStream,
        raw_delta: Option<&Value>,
    ) -> Vec<AssistantEvent> {
        let mut events = Vec::new();
        let output_id = output_id(response_id, choice.index);
        let timestamp = now();

        let key = self.reasoning_content_field.clone();
        if let Some(delta) = raw_delta {
            if let Some(text) = delta[key.as_str()].as_str().filter(|s| !s.is_empty()) {
                let reasoning_id = format!("{response_id}:reasoning:{}", choice.index);
                if let std::collections::btree_map::Entry::Vacant(e) =
                    self.reasoning_outputs.entry(choice.index)
                {
                    e.insert(true);
                    events.push(AssistantEvent::Item(Box::new(AssistantItem::Reasoning(
                        ReasoningItem::new(reasoning_id.clone(), timestamp),
                    ))));
                }
                events.push(AssistantEvent::Delta(Delta::new_at(
                    reasoning_id,
                    DeltaContent::Reasoning(text.to_string()),
                    timestamp,
                )));
            }
        }

        if let Some(content) = choice.delta.content.clone() {
            if let std::collections::btree_map::Entry::Vacant(e) = self.outputs.entry(choice.index)
            {
                e.insert(true);
                events.push(AssistantEvent::Item(Box::new(AssistantItem::Output(
                    OutputItem::new(output_id.clone(), timestamp),
                ))));
            }
            events.push(AssistantEvent::Delta(Delta::new_at(
                output_id,
                DeltaContent::Output(content),
                timestamp,
            )));
        }

        if let Some(tool_calls) = choice.delta.tool_calls {
            for call in tool_calls {
                let pending = self
                    .tool_calls
                    .entry(choice.index)
                    .or_default()
                    .entry(call.index)
                    .or_default();
                if let Some(id) = call.id {
                    pending.id = Some(id);
                }
                if let Some(function) = call.function {
                    if let Some(name) = function.name {
                        pending.name = name;
                    }
                    if let Some(arguments) = function.arguments {
                        pending.arguments.push_str(&arguments);
                    }
                }
            }
        }

        let completed = match choice.finish_reason {
            Some(FinishReason::ToolCalls) | Some(FinishReason::FunctionCall) => {
                if let Some(calls) = self.tool_calls.remove(&choice.index) {
                    for (tool_index, call) in calls {
                        let call_id = call.id.unwrap_or_else(|| {
                            format!("{response_id}:tool:{}:{tool_index}", choice.index)
                        });
                        let item = ChatCompletionMessageToolCalls::Function(
                            ChatCompletionMessageToolCall {
                                id: call_id,
                                function: FunctionCall {
                                    name: call.name,
                                    arguments: call.arguments,
                                },
                            },
                        );
                        if let Ok(item) = item.try_into() {
                            events.push(AssistantEvent::item_done(AssistantItem::ToolCall(item)));
                        }
                    }
                }
                true
            }
            Some(FinishReason::Stop)
            | Some(FinishReason::Length)
            | Some(FinishReason::ContentFilter) => true,
            None => false,
        };
        if completed {
            events.push(AssistantEvent::completed());
        }

        events
    }
}

#[cfg(test)]
mod tests {

    use std::collections::VecDeque;
    use std::sync::Arc;

    use async_openai::types::chat::ChatChoiceStream;
    use async_openai::types::chat::ChatCompletionMessageToolCallChunk;
    use async_openai::types::chat::ChatCompletionStreamResponseDelta;
    use async_openai::types::chat::CreateChatCompletionStreamResponse;
    use async_openai::types::chat::FinishReason;
    use async_openai::types::chat::FunctionCallStream;
    use async_openai::types::chat::FunctionType;
    use async_openai::types::chat::Role;
    use futures::StreamExt;
    use tokio::sync::Semaphore;

    use super::ChatCompletionsStream;
    use super::StreamState;
    use crate::llm::history::AssistantEvent;
    use crate::llm::history::message::AssistantItem;

    #[test]
    fn chat_text_chunk_creates_output_before_delta() {
        let mut state = StreamState::default();
        let events = state.handle_chunk(
            CreateChatCompletionStreamResponse {
                id: "resp".into(),
                choices: vec![ChatChoiceStream {
                    index: 0,
                    delta: ChatCompletionStreamResponseDelta {
                        content: Some("hello".into()),
                        function_call: None,
                        tool_calls: None,
                        role: None,
                        refusal: None,
                    },
                    finish_reason: None,
                    logprobs: None,
                }],
                created: 0,
                model: "model".into(),
                system_fingerprint: None,
                object: "chat.completion.chunk".into(),
                service_tier: None,
                usage: None,
            },
            None,
        );

        assert!(matches!(events[0], AssistantEvent::Item(_)));
        assert!(matches!(events[1], AssistantEvent::Delta(_)));
    }

    #[test]
    fn chat_tool_call_chunks_flush_on_finish() {
        let mut state = StreamState::default();
        let events = state.handle_chunk(
            CreateChatCompletionStreamResponse {
                id: "resp".into(),
                choices: vec![ChatChoiceStream {
                    index: 0,
                    delta: ChatCompletionStreamResponseDelta {
                        content: None,
                        function_call: None,
                        tool_calls: Some(vec![ChatCompletionMessageToolCallChunk {
                            index: 0,
                            id: Some("call_1".into()),
                            r#type: Some(FunctionType::Function),
                            function: Some(FunctionCallStream {
                                name: Some("bash".into()),
                                arguments: Some("{\"command\":\"echo".into()),
                            }),
                        }]),
                        role: None,
                        refusal: None,
                    },
                    finish_reason: None,
                    logprobs: None,
                }],
                created: 0,
                model: "model".into(),
                system_fingerprint: None,
                object: "chat.completion.chunk".into(),
                service_tier: None,
                usage: None,
            },
            None,
        );
        assert!(events.is_empty());

        let events = state.handle_chunk(
            CreateChatCompletionStreamResponse {
                id: "resp".into(),
                choices: vec![ChatChoiceStream {
                    index: 0,
                    delta: ChatCompletionStreamResponseDelta {
                        content: None,
                        function_call: None,
                        tool_calls: Some(vec![ChatCompletionMessageToolCallChunk {
                            index: 0,
                            id: None,
                            r#type: Some(FunctionType::Function),
                            function: Some(FunctionCallStream {
                                name: None,
                                arguments: Some(" hello\"}".into()),
                            }),
                        }]),
                        role: None,
                        refusal: None,
                    },
                    finish_reason: Some(FinishReason::ToolCalls),
                    logprobs: None,
                }],
                created: 0,
                model: "model".into(),
                system_fingerprint: None,
                object: "chat.completion.chunk".into(),
                service_tier: None,
                usage: None,
            },
            None,
        );

        assert!(matches!(
            events.as_slice(),
            [
                AssistantEvent::Item(item),
                AssistantEvent::Completed { .. }
            ] if matches!(item.as_ref(), AssistantItem::ToolCall(_))
        ));
    }

    #[tokio::test]
    async fn stream_emits_completed_after_stop() {
        let chunks: VecDeque<Result<serde_json::Value, async_openai::error::OpenAIError>> = [
            CreateChatCompletionStreamResponse {
                id: "resp".into(),
                choices: vec![ChatChoiceStream {
                    index: 0,
                    delta: ChatCompletionStreamResponseDelta {
                        content: None,
                        function_call: None,
                        tool_calls: None,
                        role: Some(Role::Assistant),
                        refusal: None,
                    },
                    finish_reason: None,
                    logprobs: None,
                }],
                created: 0,
                model: "model".into(),
                system_fingerprint: None,
                object: "chat.completion.chunk".into(),
                service_tier: None,
                usage: None,
            },
            CreateChatCompletionStreamResponse {
                id: "resp".into(),
                choices: vec![ChatChoiceStream {
                    index: 0,
                    delta: ChatCompletionStreamResponseDelta {
                        content: Some("hello".into()),
                        function_call: None,
                        tool_calls: None,
                        role: None,
                        refusal: None,
                    },
                    finish_reason: Some(FinishReason::Stop),
                    logprobs: None,
                }],
                created: 0,
                model: "model".into(),
                system_fingerprint: None,
                object: "chat.completion.chunk".into(),
                service_tier: None,
                usage: None,
            },
        ]
        .into_iter()
        .map(|c| Ok(serde_json::to_value(c).unwrap()))
        .collect();
        let permit = Arc::new(Semaphore::new(1))
            .acquire_owned()
            .await
            .expect("semaphore closed");
        let mut stream = ChatCompletionsStream::new(
            Box::pin(futures::stream::iter(chunks)),
            permit,
            "reasoning".into(),
        );

        assert!(matches!(
            stream.next().await,
            Some(Ok(AssistantEvent::Item(_)))
        ));
        assert!(matches!(
            stream.next().await,
            Some(Ok(AssistantEvent::Delta(_)))
        ));
        assert!(matches!(
            stream.next().await,
            Some(Ok(AssistantEvent::Completed { .. }))
        ));
        assert!(stream.next().await.is_none());
    }
}
