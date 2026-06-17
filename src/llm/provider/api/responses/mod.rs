mod delta;
mod message;
mod output;
mod reasoning;
mod toolcall;

use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::responses;
use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::OwnedSemaphorePermit;

use crate::agent::tool::registry::ToolRegistry;
use crate::config::ApiCompatConfig;
use crate::config::ModelConfig;
use crate::llm::history::AssistantEvent;
use crate::llm::history::message::AsMessageText;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::Message;
use crate::llm::history::message::UserMessage;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::StartedAssistantStream;
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
        started_at: now(),
        stream: Box::pin(inner.filter_map(move |event| {
            // reference the permit so it's captured and held for the stream's lifetime
            let _permit = &permit;
            futures::future::ready(match event {
                Ok(event) => assistant_event(event).transpose(),
                Err(err) => Some(Err(err.into())),
            })
        })),
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
                *message = Message::User(UserMessage::new(
                    dev_msg.as_message_text().to_string(),
                    now(),
                ));
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

fn assistant_event(event: responses::ResponseStreamEvent) -> Result<Option<AssistantEvent>> {
    Ok(match event {
        responses::ResponseStreamEvent::ResponseOutputTextDelta(event) => {
            Some(AssistantEvent::Delta(event.into()))
        }
        responses::ResponseStreamEvent::ResponseReasoningTextDelta(event) => {
            Some(AssistantEvent::Delta(event.into()))
        }
        responses::ResponseStreamEvent::ResponseReasoningSummaryTextDelta(event) => {
            Some(AssistantEvent::Delta(event.into()))
        }
        responses::ResponseStreamEvent::ResponseFailed(responses::ResponseFailedEvent {
            response,
            ..
        })
        | responses::ResponseStreamEvent::ResponseIncomplete(
            responses::ResponseIncompleteEvent { response, .. },
        ) => anyhow::bail!(failure_message(response)),
        responses::ResponseStreamEvent::ResponseOutputItemDone(
            responses::ResponseOutputItemDoneEvent { item, .. },
        ) => Some(AssistantEvent::item_done(item.try_into()?)),
        responses::ResponseStreamEvent::ResponseOutputItemAdded(
            responses::ResponseOutputItemAddedEvent { item, .. },
        ) => {
            if let responses::OutputItem::FunctionCall(_) = item {
                None
            } else {
                Some(AssistantEvent::Item(Box::new(item.try_into()?)))
            }
        }
        responses::ResponseStreamEvent::ResponseCompleted(_) => Some(AssistantEvent::completed()),
        _ => None,
    })
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
