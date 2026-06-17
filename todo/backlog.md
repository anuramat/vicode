# backlog

## fix/refactor/perf

- put assistant pool into router (?)
- PERF in persistence:
  - set Durability=None for most agent saves to write less
  - debounce per-agent writes to serialize less
- unwraps
- grep for TODO XXX PERF TEST
- better prompting in tool descriptions, check for inconsistencies

## UI

- message spacing
- `:help` and cmdline completions with command:description:key
- new info pane elements:
  - customizable bash status (git status by default) instead of current
    hardcoded script
  - errors
  - compact in progress
  - tools/subagents in progress
- markdown rendering in input field
- streaming subagent progress/tool outputs
- add steering submit mode -- queue prompt after tool call finishes
- show combined elapsed time for multiturn (ie multiple assistant message)
- tool call "intent" -- dummy argument for better ui, show for collapsed tool calls
- float picker for undo/compact/jump

## core features & QoL

- question tool
- abort individual tool calls (without aborting the turn)
- when bash tool is aborted, it should send partial results to the assistant
- let user execute bash commands in current tab with `!...`
  - append a developer message equivalent to bash tool output, with equivalent rendering
- autocompact on threshold
- alternative argument schemas for user compact command
- retries after abort/failure should append devmsg eg "assistants turn was interrupted by the user/unexpected error"
- skills
  - with `$` completion
  - subagent tool gets `skills: Vec<String>` field

## lower priority

- ratatui::backend::TestBackend tests
- debug socket so agents can debug by sending event jsons
- VHS for testing and demos
- mcp client
- read token usage field in response instead of estimating with tiktoken
- add passthrough params; cerebras-specific -- clear_thinking: false
- AGENTS.sh
  - execute once or for every agent?
- customizable app theme
- customizable syntax hl theme
