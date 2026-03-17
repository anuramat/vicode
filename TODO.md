# core

- edit tool -- expand to create file, replace file, append, insert at
- subagents
  - two types
    - parallel -- each owns a workdir; same as "replica" in best-of-n
    - blocking -- one at a time, shares workdir with parent, probably gets git access
- compact
  - would be cool if we could mark specific messages for compaction
- skills

# ui

- visible progress:
  - every assistant message should show elapsed time
    - so, record start time, enum in_progress|done(end_time)
  - last line of scroll should show status:
    - idle
    - generating
    - %d subagents, %d tool calls
       - should we count those recursively?
    - failed
  - if not too hard: in progress assistant message should have a cursor block after the last char
    - probably should be done through custom render logic on message widget
  - ideally: in a multiturn, show combined time only
- retry key
  - should restart agent and re-attach if agent task is dead
- token usage/context window free %
  - need to add config value "max_tokens" per assistant
  - mvp: `tokens = history.serialize().len() / 3`
  - later try reading token usage field in response
- undo msg
  - just wipe to last user msg inclusive, and fill input field with the contents
- rename thread
  - custom name is purely for tab list, status line should still show tab id
  - on duplication, it should get a `(2)` or something like that

- float window with past errors (ones from stl notifications)
- float window with logs

- visible replica progress

- render todo tool like any other widget
- streaming tool outputs

# chores

- grep for unwraps
- grep for TODO etc
- make sure our prompting makes sense (check for inconsistencies)
- cleanup logic for stale agents/worktrees/etc
- make sure tracing::debug! doesn't get into release build
- refactor: `if let Some(x) = ... { x } else { return }` into `let Some(x) = ...`

# maybe

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
