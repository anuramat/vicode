# multiagent features

core:
- let primary send a follow-up after subagent finished original request
- async subagents: await/abort
- automatically show subagents what other subagents are working on: dev msg with short summaries
  - e.g. at least when main agent spawns N subagents, they should see each others prompts
- let children talk to parents (ask for clarification)
- let primary create new primaries
  - usecases:
    - in the middle of a convo, you discover a new task you want to work on --
      prompt "launch a primary agent to take care of X, and keep working on Y"
    - you want to work on 5 similar things in 5 tabs
- let user observe+steer subagents

not sure:
- maybe let the parent keep the control over its children primaries?
- let primary agents share a pool of subagents? can't think of any usecases that
  aren't covered by agent-generated skills though
- let subagents talk to each other, again -- not sure I see the usecase
- unlimited recursion depth, but limited total number of agents for each primary agent
- let primary agents see each other?

note -- when we say "agent Alpha sends a message to agent Beta", it's not
necessarily a literal prompt in Beta's loop: if Alpha just needs extra context,
we can (should) do a single turn in a separate task with no tool calls, i.e. in
parallel to Beta. We could even expose a stripped down message history of Beta
as a file using fuse, so that Alpha can just grep for whatever it needs.
