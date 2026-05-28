mod delta;
mod message;
mod output;
mod reasoning;
mod toolcall;

use std::pin::Pin;
use std::task::Context as TaskContext;
use std::task::Poll;

use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::responses;
use async_trait::async_trait;
use futures::Stream;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolRegistry;
use crate::config::ApiCompatConfig;
use crate::config::ModelConfig;
use crate::llm::history::message::AsMessageText;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::Message;
use crate::llm::history::message::UserMessage;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::StartedAssistantStream;
use crate::llm::provider::api::StreamEvent;
use crate::llm::provider::assistant::ReasoningEffort;
use crate::utils::now;

#[derive(Debug)]
pub struct ResponsesApi {
    client: Client<OpenAIConfig>,
    compat: ApiCompatConfig,
}

impl ResponsesApi {
    pub fn new(
        client: Client<OpenAIConfig>,
        compat: ApiCompatConfig,
    ) -> Self {
        Self { client, compat }
    }
}

#[async_trait]
impl Api for ResponsesApi {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        model: ModelConfig,
        instructions: String,
        messages: Vec<Message>,
        tools: ToolRegistry,
    ) -> Result<StartedAssistantStream> {
        let request = request(model, instructions, messages, tools, &self.compat)?;
        let inner = self.client.responses().create_stream(request).await?;
        Ok(started_stream(permit, inner))
    }
}

pub fn started_stream(
    permit: OwnedSemaphorePermit,
    inner: responses::ResponseStream,
) -> StartedAssistantStream {
    StartedAssistantStream {
        started_at_ms: now(),
        stream: Box::pin(ResponsesStream {
            inner,
            _permit: permit,
        }),
    }
}

pub fn request(
    model: ModelConfig,
    instructions: String,
    mut messages: Vec<Message>,
    tools: ToolRegistry,
    compat: &ApiCompatConfig,
) -> Result<responses::CreateResponse> {
    let mut builder = responses::CreateResponseArgs::default();
    builder
        .model(&model.model)
        .parallel_tool_calls(true)
        .reasoning(responses::Reasoning {
            effort: model.effort.map(Into::into),
            summary: Some(responses::ReasoningSummary::Detailed),
        })
        .include(vec![responses::IncludeEnum::ReasoningEncryptedContent])
        .store(false);

    // NOTE here order is important -- message with instructions (if any) should stay a developer message regardless
    if compat.developer_as_user {
        for message in &mut messages {
            if let Message::Developer(dev_msg) = message {
                *message = Message::User(UserMessage::new(dev_msg.as_message_text().to_string()));
            }
        }
    }

    if compat.instructions_as_message {
        let msg = Message::Developer(DeveloperMessage::misc(instructions));
        messages.insert(0, msg);
    } else {
        builder.instructions(instructions);
    }

    if let Some(tag) = compat.reasoning_as_output.clone() {
        for message in &mut messages {
            crate::llm::provider::compat::reasoning_to_output(&tag, message);
        }
    }

    builder.stream(true);
    let input = messages
        .into_iter()
        .flat_map(|message| {
            let items: Vec<responses::InputItem> = (&message).into();
            items
        })
        .collect();
    builder.input(responses::InputParam::Items(input));
    if !tools.is_empty() {
        builder.tools(tools);
    }
    Ok(builder.build()?)
}

struct ResponsesStream {
    inner: responses::ResponseStream,
    _permit: OwnedSemaphorePermit,
}

impl Stream for ResponsesStream {
    type Item = Result<StreamEvent, anyhow::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx).map(|e| match e {
            Some(Ok(event)) => Some(event.try_into()),
            Some(Err(err)) => Some(Err(anyhow::Error::from(err))),
            None => None,
        })
    }
}

impl From<ToolRegistry> for Vec<responses::Tool> {
    fn from(registry: ToolRegistry) -> Self {
        registry
            .schemas
            .into_iter()
            .map(|tool| {
                responses::Tool::Function(responses::FunctionTool {
                    name: tool.name().clone(),
                    parameters: Some(tool.parameters().clone()),
                    strict: Some(true),
                    description: Some(tool.description().clone()),
                })
            })
            .collect::<Self>()
    }
}

fn failure_message(response: responses::Response) -> String {
    let mut combined_error_message = String::new();
    if let Some(error) = response.error {
        combined_error_message.push_str(
            format!("error; code={}, message: {};\n", error.code, error.message).as_str(),
        );
    }
    if let Some(inc) = response.incomplete_details {
        combined_error_message
            .push_str(format!("incomplete response; reason: {};\n", inc.reason).as_str());
    }
    combined_error_message
}

fn output_items(items: Vec<responses::OutputItem>) -> Result<Vec<AssistantItem>> {
    items.into_iter().map(AssistantItem::try_from).collect()
}

impl TryFrom<responses::ResponseStreamEvent> for StreamEvent {
    type Error = anyhow::Error;

    fn try_from(event: responses::ResponseStreamEvent) -> Result<Self> {
        Ok(match event {
            responses::ResponseStreamEvent::ResponseOutputTextDelta(event) => {
                Self::Delta(event.into())
            }
            responses::ResponseStreamEvent::ResponseReasoningTextDelta(event) => {
                Self::Delta(event.into())
            }
            responses::ResponseStreamEvent::ResponseReasoningSummaryTextDelta(event) => {
                Self::Delta(event.into())
            }
            responses::ResponseStreamEvent::ResponseFailed(responses::ResponseFailedEvent {
                response,
                ..
            })
            | responses::ResponseStreamEvent::ResponseIncomplete(
                responses::ResponseIncompleteEvent { response, .. },
            ) => Self::Failed(failure_message(response)),
            responses::ResponseStreamEvent::ResponseOutputItemDone(
                responses::ResponseOutputItemDoneEvent { item, .. },
            ) => Self::ItemDone(item.try_into()?),
            responses::ResponseStreamEvent::ResponseOutputItemAdded(
                responses::ResponseOutputItemAddedEvent { item, .. },
            ) => {
                if let responses::OutputItem::FunctionCall(_) = item {
                    Self::Ignore
                } else {
                    Self::ItemAdded(item.try_into()?)
                }
            }
            responses::ResponseStreamEvent::ResponseCompleted(event) => {
                Self::Completed(output_items(event.response.output)?)
            }
            _ => Self::Ignore,
        })
    }
}

impl From<ReasoningEffort> for responses::ReasoningEffort {
    fn from(effort: ReasoningEffort) -> Self {
        match effort {
            ReasoningEffort::None => Self::None,
            ReasoningEffort::Minimal => Self::Minimal,
            ReasoningEffort::Low => Self::Low,
            ReasoningEffort::Medium => Self::Medium,
            ReasoningEffort::High => Self::High,
            ReasoningEffort::Xhigh => Self::Xhigh,
        }
    }
}
