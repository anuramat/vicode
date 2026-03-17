# core

- subagents
  - two types
    - parallel -- each owns a workdir; same as "replica" in best-of-n
    - blocking -- one at a time, shares workdir with parent, probably gets git access
- compact
  - would be cool if we could mark specific messages for compaction
- skills
- define multiple backends in config
  - smart routing in parallel workflows, so that we don't get ratelimited
- auto-rename thread with proompting
- edit tool -- expand to create file, replace file, append, insert at

# ui

- agent stats: tool call count/status (including subagents), turn duration, tokens used
- show latest todo in info pane
- backend switcher
- edit msg
- retry key
- visible progress (throbber?)
- UI thingie for errors/warnings
- visible replica/subagent progress
- rename thread

# chores

- grep for unwraps
- grep for TODO etc
- make sure our prompting makes sense (check for inconsistencies)
- cleanup logic for stale agents/worktrees/etc
- make sure tracing::debug! doesn't get into release build
- refactor: `if let Some(x) = ... { x } else { return }` into `let Some(x) = ...`

# maybe

- tool streaming?
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
