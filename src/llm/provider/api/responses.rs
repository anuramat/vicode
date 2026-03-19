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

use crate::agent::tool::registry::ToolSchemas;
use crate::config::ApiCompatConfig;
use crate::config::ModelConfig;
use crate::config::ProviderConfig;
use crate::llm::history::History;
use crate::llm::message::*;
use crate::llm::provider::api::Api;
use crate::llm::provider::api::StartedAssistantStream;
use crate::llm::provider::event::StreamEvent;

pub struct ResponsesApi {
    client: Client<OpenAIConfig>,
    compat: ApiCompatConfig,
}

impl ResponsesApi {
    pub fn new(
        client: Client<OpenAIConfig>,
        config: ProviderConfig,
    ) -> Self {
        Self {
            client,
            compat: config.compat,
        }
    }
}

#[async_trait]
impl Api for ResponsesApi {
    async fn stream(
        &self,
        permit: OwnedSemaphorePermit,
        model: ModelConfig,
        instructions: String,
        history: History,
        tools: ToolSchemas,
    ) -> Result<StartedAssistantStream> {
        let request = request(model, instructions, history, tools, &self.compat)?;
        let started_at_ms = now_ms();
        let inner = self.client.responses().create_stream(request).await?;
        Ok(StartedAssistantStream {
            started_at_ms,
            stream: Box::pin(ResponsesStream {
                inner,
                _permit: permit,
            }),
        })
    }
}

fn request(
    model: ModelConfig,
    instructions: String,
    history: History,
    tools: ToolSchemas,
    compat: &ApiCompatConfig,
) -> Result<responses::CreateResponse> {
    let mut builder = responses::CreateResponseArgs::default();
    builder
        .model(&model.model)
        .parallel_tool_calls(true)
        .reasoning(responses::Reasoning {
            effort: model.effort,
            summary: Some(responses::ReasoningSummary::Detailed),
        })
        .include(vec![responses::IncludeEnum::ReasoningEncryptedContent])
        .store(false);
    let mut messages = history.messages();

    // NOTE here order is important -- message with instructions (if any) should stay a developer message regardless
    if compat.developer_as_user {
        messages.iter_mut().for_each(|message| {
            if let Message::Developer(dev_msg) = message {
                *message = Message::User(UserMessage {
                    text: dev_msg.text.clone(),
                })
            }
        });
    }

    if compat.instructions_as_message {
        let msg = Message::Developer(DeveloperMessage { text: instructions });
        messages.insert(0, msg);
    } else {
        builder.instructions(instructions);
    }

    if let Some(tag) = compat.reasoning_as_output.clone() {
        messages.iter_mut().for_each(move |message| {
            crate::llm::provider::compat::reasoning_to_output(&tag, message)
        });
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
    if !tools.0.is_empty() {
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

impl From<ToolSchemas> for Vec<responses::Tool> {
    fn from(schema: ToolSchemas) -> Self {
        schema
            .0
            .into_iter()
            .map(|tool| {
                responses::Tool::Function(responses::FunctionTool {
                    name: tool.name,
                    parameters: Some(tool.parameters),
                    strict: Some(true),
                    description: Some(tool.description),
                })
            })
            .collect::<Vec<_>>()
    }
}

fn failure_message(response: responses::Response) -> String {
    let mut combined_error_message = String::new();
    if let Some(error) = response.error {
        combined_error_message.push_str(
            format!("error; code={}, message: {};\n", error.code, error.message).as_str(),
        );
    };
    if let Some(inc) = response.incomplete_details {
        combined_error_message
            .push_str(format!("incomplete response; reason: {};\n", inc.reason).as_str());
    };
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
                StreamEvent::Delta(event.into())
            }
            responses::ResponseStreamEvent::ResponseReasoningTextDelta(event) => {
                StreamEvent::Delta(event.into())
            }
            responses::ResponseStreamEvent::ResponseReasoningSummaryTextDelta(event) => {
                StreamEvent::Delta(event.into())
            }
            responses::ResponseStreamEvent::ResponseFailed(responses::ResponseFailedEvent {
                response,
                ..
            })
            | responses::ResponseStreamEvent::ResponseIncomplete(
                responses::ResponseIncompleteEvent { response, .. },
            ) => StreamEvent::Failed(failure_message(response)),
            responses::ResponseStreamEvent::ResponseOutputItemDone(
                responses::ResponseOutputItemDoneEvent { item, .. },
            ) => StreamEvent::ItemDone(item.try_into()?),
            responses::ResponseStreamEvent::ResponseOutputItemAdded(
                responses::ResponseOutputItemAddedEvent { item, .. },
            ) => {
                if let responses::OutputItem::FunctionCall(_) = item {
                    StreamEvent::Ignore
                } else {
                    StreamEvent::ItemAdded(item.try_into()?)
                }
            }
            responses::ResponseStreamEvent::ResponseCompleted(event) => {
                StreamEvent::Completed(output_items(event.response.output)?)
            }
            _ => StreamEvent::Ignore,
        })
    }
}
