# refactor

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

# core

- file completion on @
- mac support: no overlays, sandbox-exec instead of bwrap
- :help
- show bindings/usage for commands in cmpmenu
- '!...' command -- run bash in current tab, and append a developer message equivalent to bash tool output, with equivalent rendering
- markdown rendering in input field
- subagents
  - two types
    - parallel -- each owns a workdir; same as "replica" in best-of-n
    - blocking -- one at a time, shares workdir with parent, probably gets git access
- autocompact on threshold
- alternative argument schemas for user compact command
- skills
- retries after abort/failure should append devmsg eg "assistants turn was interrupted by the user/unexpected error"
- mcp
- float window fzf-lua style
  - past errors (ones from stl notifications)
  - logs
  - message picker for undo/compact/jump
- streaming replica progress/tool outputs

# backlog

- refactor: `if let Some(x) = ... { x } else { return }` into `let Some(x) = ...`
- move ALL prompting into separate files with suffix like `.prompt.md`
- lua scripting/configs with mlua
- yml configs with serde_yml
- try reading token usage field in response instead of estimating with tiktoken?
- show combined elapsed time for multiturn (ie multiple assistant message)
- show TTFT?
- gitless scenario -- project dir instead of snapshots for overlays
- rename thread
  - custom name is purely for tab list, status line should still show tab id
  - on duplication, it should get a `(2)` or something like that
- add passthrough params; cerebras-specific -- clear_thinking: false
- better strategy than round robin for providers?
  - load balancing is actually out of scope (probably)
  - sampling with relative weights might be useful for diversity in best-of-n though
- plan mode, "question" tool, plan files
- bash commands/includes in context files and user prompts
- tool call "intent" -- dummy argument for better ui
- aggressively proompt to "use as many tools in parallel as possible"
- let agents read compacted messages with fuse
- let agents leave breadcrumb memories to read later with fuse
- hooks?
- when main agent spawns subagents, show all prompts to all subagents (to
  minimize overlap)
  - cross-agent comms?
- if agent didn't inherit context, provide a tool to retroactively force inherit_context=true
- aggressively spawn agents with inherit=true, should be almost always a good idea
- cli
  - `vc -h/--help`
  - `vc config show` -- show effective config
  - project commands, `-a/--all` -- apply to all projects
    - `vc nuke mounts` -- unmount all in this project
    - `vc nuke data` -- wipe data and logs for this project
