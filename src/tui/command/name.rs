use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use strum::EnumIter;

// TODO expose usage in completion menu using https://docs.rs/strum/latest/strum/derive.EnumMessage.html

serde_plain::derive_display_from_serialize!(CommandName);
serde_plain::derive_fromstr_from_deserialize!(CommandName);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, EnumIter, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    /// select next assistant in the currently selected tab
    AssistantNext,
    /// select previous assistant in the currently selected tab
    AssistantPrev,
    /// switch to cmdline
    CmdlineEnter,
    /// compact the conversation
    Compact,
    /// cancel completion and hide the menu
    CompletionCancel,
    /// select next completion result
    CompletionNext,
    /// select previous completion result
    CompletionPrev,
    /// exit insert mode/cmdline
    InputExit,
    /// submit the message/command
    InputSubmit,
    /// switch to insert mode
    InsertEnter,
    /// paste args into focused input
    InsertPaste,
    /// remove last message
    MsgUndo,
    /// remove messages starting from last user message
    MsgUndoUser,
    /// quit the app
    Quit,
    /// refresh the info pane
    RefreshInfo,
    /// scroll the focused pane
    Scroll,
    /// set multiplier for the next prompt
    SetMultiplier,
    /// archive the currently selected tab (deleted later by `vc cleanup`)
    TabArchive,
    /// duplicate the currently selected tab
    TabDuplicate,
    /// open new tab
    TabNew,
    /// switch to the next tab
    TabNext,
    /// switch to the previous tab
    TabPrev,
    /// select tab by index
    TabSelect,
    /// show/hide developer messages
    ToggleDeveloper,
    /// toggle focus/visibility of the info pane
    ToggleInfo,
    /// toggle markdown rendering
    ToggleMarkdown,
    /// show/hide reasoning
    ToggleReasoning,
    /// show/hide tab pane
    ToggleTabs,
    /// show/hide tool calls
    ToggleTools,
    /// abort current turn
    TurnAbort,
    /// retry turn/compact
    TurnRetry,
    /// dummy command to unmap keys
    #[serde(alias = "")]
    None,
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    #[test]
    fn command_names_display_in_config_format() {
        assert_eq!(CommandName::CompletionNext.to_string(), "completion_next");
    }
}
