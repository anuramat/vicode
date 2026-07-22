# multiagent

## features

core:
- async subagents, with a separate tool for blocking until subagent is idle
- inter-agent comms:
  - let primary send a follow-up after subagent finished original request, or
    steer it in the middle of its turn
  - let children talk to parents (ask for clarification etc.)
- cross-agent visibility: subagents should know what their siblings are working
  on, e.g. we could auto-inject dev msg with short summaries
- let primary create new primaries (should probably require manual approval from the user)
  - usecases:
    - in the middle of a convo, you discover a new task you want to work on --
      prompt "launch a primary agent to take care of X, and keep working on Y"
    - you want to work on N similar things in N separate tabs -- prompt "create
      N new tabs for each of these"
- UI: let user observe+steer subagents

not sure:
- let primary agents see each other?
- how exactly should we limit subagent recursion?
- let agents ask each other "btw" questions? i.e. without mutating the
  conversation history, usecase -- "what are you working on? any progress?"
