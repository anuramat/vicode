use crate::llm::history::History;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::DeveloperMessage;
use crate::llm::history::message::Message;

const SUBAGENT_HEADER: &str = r"
You are a subagent, assisting your parent agent.
The parent agent will provide you with a task in the next developer message, and you should closely follow the instructions in it.
Your working directory is your own git branch, checked out at the commit your parent spawned you from.

- Do NOT converse, ask questions, or suggest next steps
- Do NOT editorialize or add meta-commentary
- Do NOT emit text between tool calls. Use tools silently, then report once at the end, by `send`ing the report to your parent (the `[from: <id>]` of your task message) -- your final text reaches no one.
- Stay strictly within your directive's scope. If you discover related systems outside your scope, mention them in one sentence at most.
- Keep your report under 500 words unless the directive specifies otherwise. Be factual and concise.
- Commit the file changes you want your parent to see: it reads them from your branch, and never sees uncommitted ones. Do NOT describe them in your report.
";

const INHERITED_NOTE: &str = r"
Messages above are from the conversation between the user and your parent agent. Changes your parent made but did not commit are NOT in your working directory, even if mentioned above.
";

fn header(text: String) -> Message {
    Message::Developer(DeveloperMessage::misc(text))
}

impl History {
    /// messages for subagent -- full copy of latest state but with tool calls dropped in the last message
    fn subagent_messages(&self) -> Vec<Message> {
        let mut messages = self.state().messages.clone();
        if let Some(Message::Assistant(msg)) = messages.last_mut() {
            msg.content
                .retain(|_, content| !matches!(content, AssistantItem::ToolCall(_)));
            msg.recount_shallow();
        }
        messages.push(header(format!("{SUBAGENT_HEADER}{INHERITED_NOTE}")));
        messages
    }

    /// an inheriting subagent's history: the parent's conversation, then
    /// the subagent header
    pub fn subagent(&self) -> Self {
        Self {
            instructions: self.instructions.clone(),
            generation: 0,
            state: self.subagent_messages().into(),
            archive: Vec::new(),
        }
    }

    /// a non-inheriting subagent's history: just the subagent header
    pub fn new_subagent(instructions: String) -> Self {
        let mut history = Self::new(instructions);
        history
            .state_mut()
            .push(header(SUBAGENT_HEADER.to_string()));
        history
    }
}

#[cfg(test)]
mod tests {
    use indexmap::indexmap;

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
            }),
        })
    }

    #[test]
    fn subagent_history_resets_generation_and_drops_last_tool_calls() {
        let mut history = History::new("be precise".into());
        history.state = vec![
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
        .into();
        history.generation = 2;
        history.state.recount();

        let child = history.subagent();

        assert_eq!(child.generation(), 0);
        insta::assert_yaml_snapshot!(
            child,
            { ".**.Misc.created_at" => "[created_at]" },
            @r#"
        instructions:
          text: be precise
          token_count: 2
        state:
          messages:
            - role: user
              text: parent prompt
              token_count: 2
              created_at: 0
            - role: assistant
              status: Success
              content:
                - - out
                  - Output:
                      id: out
                      content:
                        - Text: done
                      token_count: 1
                      started_at: 1
                      ended_at: ~
              token_count: 1
              created_at: 0
              started_at: 0
              ended_at: ~
              ready_at: ~
            - role: developer
              Misc:
                text: "\nYou are a subagent, assisting your parent agent.\nThe parent agent will provide you with a task in the next developer message, and you should closely follow the instructions in it.\nYour working directory is your own git branch, checked out at the commit your parent spawned you from.\n\n- Do NOT converse, ask questions, or suggest next steps\n- Do NOT editorialize or add meta-commentary\n- Do NOT emit text between tool calls. Use tools silently, then report once at the end, by `send`ing the report to your parent (the `[from: <id>]` of your task message) -- your final text reaches no one.\n- Stay strictly within your directive's scope. If you discover related systems outside your scope, mention them in one sentence at most.\n- Keep your report under 500 words unless the directive specifies otherwise. Be factual and concise.\n- Commit the file changes you want your parent to see: it reads them from your branch, and never sees uncommitted ones. Do NOT describe them in your report.\n\nMessages above are from the conversation between the user and your parent agent. Changes your parent made but did not commit are NOT in your working directory, even if mentioned above.\n"
                token_count: 247
                created_at: "[created_at]"
          token_count: 280
        archive: []
        "#,
        );
    }
}
