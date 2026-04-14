# fix/refactor/perf

- agent init is kinda fucked up, need clear boundaries on init status: when
  overlay is mounted, when state is written to file
- bash widget is a mess
- fix agent state persistence
  - use jsonl for storage
  - debounce file writes
- unwraps
- grep for TODO XXX PERF TEST
- make sure our prompting makes sense (check for inconsistencies)
- cleanup logic for stale agents/worktrees/etc
- `if let Some(x) = ... { x } else { return }` into `let Some(x) = ...`
  - maybe there's a clippy lint? or use astgrep
- check that tool call timing records execution start+end
- count tool schema in the token usage
- replace .map_err with .context()

# UI

- message spacing
- `:help` and cmdline completions with command:description:key
- sidebar
  - "focus_sidebar" command
  - responsiveness:
    - wide screen: always show, highlight when focused
    - narrow screen: show when focused
  - elements:
    - customizable bash status (git status by default)
    - errors
    - compact in progress
    - tools/subagents? although we could just show it in the message history? 
      - I guess we could show more data, compared to history
- markdown rendering in input field
- streaming subagent progress/tool outputs

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
- subagents
  - two types
    - parallel -- each owns a workdir; same as "replica" in best-of-n
    - blocking -- one at a time, shares workdir with parent, probably gets git access
- autocompact on threshold
- alternative argument schemas for user compact command
- retries after abort/failure should append devmsg eg "assistants turn was interrupted by the user/unexpected error"

# new features

- mcp
- acp
- skills
  - with `$` completion
  - reusable for subagents

# backlog

- let the user browse subagents like normal tabs, probably read-only though
  - if we let the user prompt subagents, main agents should be able to prompt them too
- float window fzf-lua style
  - past errors (ones from stl notifications)
  - logs
  - message picker for undo/compact/jump
- build prompt recursively from modules
- lua scripting using `mlua`
- try reading token usage field in response instead of estimating with tiktoken?
- show combined elapsed time for multiturn (ie multiple assistant message)
- show time to first token
- gitless scenario -- project dir instead of snapshots for overlays
- add passthrough params; cerebras-specific -- clear_thinking: false
- better strategy than round robin for providers?
  - load balancing is actually out of scope (probably)
  - sampling with relative weights might be useful for diversity in best-of-n though
- round-robin assistants each turn, as in https://www.swebench.com/SWE-bench/blog/2025/08/19/mini-roulette/
- question tool
- plan tool? basically subagent (inherit=false) with prompt=plan, but ask user for confirmation
- bash commands/includes in context files and user prompts
- tool call "intent" -- dummy argument for better ui, show for collapsed tool calls
- aggressive prompting:
  - use as many tools in parallel as possible
  - ofetn spawn agents (with inherit=true)
- fuse to provide extra optional context:
  - compacted history
  - let agent create skill-memories
    - global/per-project/maybe even per-agent
- hooks?
- when main agent spawns subagents, show all prompts to all subagents (to
  minimize overlap)
  - cross-agent comms?
- if agent didn't inherit context, provide a tool to retroactively force inherit_context=true
- cli
  - project commands, `-a/--all` -- apply to all projects
    - `vc nuke mounts` -- unmount all in this project
    - `vc nuke data` -- wipe data and logs for this project
