use crate::llm::history::Activity;
use crate::llm::history::History;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::Message;

const SUBAGENT_HEADER: &str = r"
You are a subagent, assisting your parent agent.
Messages above are from the conversation between the user and your parent agent.
The parent agent will provide you with a task in the next user message, and you should closely follow the instructions in it.

- Do NOT converse, ask questions, or suggest next steps
- Do NOT editorialize or add meta-commentary
- Do NOT emit text between tool calls. Use tools silently, then report once at the end.
- Stay strictly within your directive's scope. If you discover related systems outside your scope, mention them in one sentence at most.
- Keep your report under 500 words unless the directive specifies otherwise. Be factual and concise.
- Do NOT describe the file changes you made in your report -- parent agent will receive the file diffs separately.
";

impl History {
    /// messages for subagent -- full copy of latest state but with tool calls dropped in the last message
    fn subagent_messages(&self) -> Vec<Message> {
        let mut messages = self.state().messages.clone();
        if let Some(Message::Assistant(msg)) = messages.last_mut() {
            msg.content
                .retain(|_, content| !matches!(content, AssistantItem::ToolCall(_)));
            msg.recount_shallow();
        }
        messages.push(Message::Developer(DeveloperMessage::misc(
            SUBAGENT_HEADER.to_string(),
        )));
        messages
    }

    pub fn subagent(
        &self,
        inherit_context: bool,
    ) -> Self {
        let messages = if inherit_context {
            self.subagent_messages()
        } else {
            Vec::new()
        };
        Self {
            instructions: self.instructions.clone(),
            generation: 0,
            activity: Activity::Normal {
                state: messages.into(),
            },
            archive: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use indexmap::indexmap;

    use crate::llm::history::Activity;
    use crate::llm::history::History;
    use crate::llm::history::message::AssistantItem;
    use crate::llm::history::message::AssistantMessage;
    use crate::llm::history::message::AssistantStatus;
    use crate::llm::history::message::Message;
    use crate::llm::history::message::OutputContent;
    use crate::llm::history::message::OutputItem;
    use crate::llm::history::message::ToolCallItem;
    use crate::llm::history::message::UserMessage;
    use crate::llm::history::tokens::TokenCount;
    use crate::tools::bash::BashArguments;
    use crate::tools::bash::BashCall;

    fn tool_call(id: &str) -> AssistantItem {
        AssistantItem::ToolCall(ToolCallItem {
            id: Some(id.into()),
            call_id: id.into(),
            started_at: 1,
            ended_at: None,
            ready_at: None,
            token_count: 0,
            task: Box::new(BashCall {
                arguments: Some(BashArguments {
                    command: "echo hello".into(),
                }),
                output: None,
                meta: None,
                context: None,
            }),
        })
    }

    #[test]
    fn subagent_history_resets_generation_and_drops_last_tool_calls() {
        let mut history = History::new("be precise".into());
        history.activity = crate::llm::history::Activity::Normal {
            state: vec![
                Message::User(UserMessage {
                    text: "parent prompt".into(),
                    created_at: 0,
                    token_count: 0,
                }),
                Message::Assistant(AssistantMessage {
                    status: AssistantStatus::Success,
                    content: indexmap! {
                        "out".into() => AssistantItem::Output(OutputItem {
                            id: "out".into(),
                            started_at: 1,
                            ended_at: None,
                            token_count: 0,
                            content: vec![OutputContent::Text("done".into())],
                        }),
                        "call_1".into() => tool_call("call_1"),
                    },
                    created_at: 0,
                    started_at: Some(0),
                    ended_at: None,
                    ready_at: None,
                    token_count: 0,
                }),
            ]
            .into(),
        };
        history.generation = 2;
        if let Activity::Normal { state } = &mut history.activity {
            state.recount();
        } else {
            panic!("expected normal turn");
        }

        let child = history.subagent(true);

        assert_eq!(child.generation(), 0);
        insta::assert_json_snapshot!(
            serde_json::to_value(&child).unwrap(),
            { ".state.messages[2].Misc.queued_ms" => "[queued_ms]" },
            @r#"
        {
          "archive": [],
          "compact": null,
          "instructions": {
            "text": "be precise",
            "token_count": 2
          },
          "state": {
            "messages": [
              {
                "created_at": 0,
                "role": "user",
                "text": "parent prompt",
                "token_count": 2
              },
              {
                "content": [
                  [
                    "out",
                    {
                      "Output": {
                        "content": [
                          {
                            "Text": "done"
                          }
                        ],
                        "ended_at": null,
                        "id": "out",
                        "started_at": 1,
                        "token_count": 1
                      }
                    }
                  ]
                ],
                "created_at": 0,
                "ended_at": null,
                "ready_at": null,
                "role": "assistant",
                "started_at": 0,
                "status": "Success",
                "token_count": 1
              },
              {
                "Misc": {
                  "created_at": 1777264279004,
                  "text": "\nYou are a subagent, assisting your parent agent.\nMessages above are from the conversation between the user and your parent agent.\nThe parent agent will provide you with a task in the next user message, and you should closely follow the instructions in it.\n\n- Do NOT converse, ask questions, or suggest next steps\n- Do NOT editorialize or add meta-commentary\n- Do NOT emit text between tool calls. Use tools silently, then report once at the end.\n- Stay strictly within your directive's scope. If you discover related systems outside your scope, mention them in one sentence at most.\n- Keep your report under 500 words unless the directive specifies otherwise. Be factual and concise.\n- Do NOT describe the file changes you made in your report -- parent agent will receive the file diffs separately.\n",
                  "token_count": 163
                },
                "role": "developer"
              }
            ],
            "token_count": 196
          }
        }
        "#,
        );
    }
}
