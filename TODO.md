# fix/refactor/perf

- agent init is kinda fucked up, need clear boundaries on init status: when
  overlay is mounted, when state is written to file
- fix agent state persistence
  - use jsonl for storage
  - debounce file writes
- unwraps
- grep for TODO XXX PERF TEST
- make sure our prompting makes sense (check for inconsistencies)
- cleanup logic for stale agents/worktrees/etc
- check that tool call timing records execution start+end
- count tool schema in the token usage

# UI

- tab pane rework: hideable/scrollable just like the info pane
- j/k by default scroll by user supplied percent of height
- message spacing
- `:help` and cmdline completions with command:description:key
- sidebar elements:
  - customizable bash status (git status by default)
  - errors
  - compact in progress
  - tools/subagents? although we could just show it in the message history? 
    - I guess we could show more data, compared to history
- markdown rendering in input field
- streaming subagent progress/tool outputs
- add steering submit mode -- queue prompt after tool call finishes

# core features & QoL

- let tools take an optional `stage` param: signed int, 0 by default
  - tools in a single stage execute in parallel, stages execute sequentially
- better prompting in tool descriptions
- mac support
  - add a default mac sandbox profile
  - detect the platorm and store somewhere
  - default to mac sandbox on mac (for now we're hardcoding bwrap)
  - add a wrapped nix package for mac, reuse in arx
- when bash tool is aborted, it should send partial results to the assistant
- let user execute bash commands in current tab with `!...`
  - append a developer message equivalent to bash tool output, with equivalent rendering
- autocompact on threshold
- alternative argument schemas for user compact command
- retries after abort/failure should append devmsg eg "assistants turn was interrupted by the user/unexpected error"

# backlog

- customizable syntax highlighting colorscheme
- float window fzf-lua style
  - logs
  - message picker for undo/compact/jump
- try reading token usage field in response instead of estimating with tiktoken?
- show combined elapsed time for multiturn (ie multiple assistant message)
- add passthrough params; cerebras-specific -- clear_thinking: false
- bash commands/includes in context files and user prompts
- tool call "intent" -- dummy argument for better ui, show for collapsed tool calls
- fuse to provide extra optional context:
  - compacted history
  - let agent create skill-memories
    - global/per-project/maybe even per-agent
- hooks?
- if agent didn't inherit context, provide a tool to retroactively force inherit_context=true
- cli
  - project commands, `-a/--all` -- apply to all projects
    - `vc nuke mounts` -- unmount all in this project
    - `vc nuke data` -- wipe data and logs for this project
