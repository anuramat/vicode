# phase 4: multiagent features on a fully pure core

Status: **planning input, not a final plan.** Start the next session with a
planning pass over this doc + `wip_multiagent.md`, resolve the open questions,
then write the implementation plan. Assumes the phase 1–3 architecture as
landed on `wip/testability` (commits `e0c14c4`, `1761b60`, `247aa82`) — that
branch is under user review and may get refactored/renamed before phase 4
starts, so **re-verify the file map below at session start**.

## where we are (phases 1–3, landed)

- `src/agent/core.rs` — pure `AgentCore::handle(now, CoreEvent, &mut Vec<Effect>)`:
  all loop decisions (turn starts, saves, done-oneshot choreography, status
  sync), time as data, no tokio/fs/clock. Tested with sync `#[test]`
  `(status, effects)` yaml snapshots via `AgentCore::fake()`.
- `src/agent/handle.rs` — wire types (`AgentEvent`/`ExternalEvent`/`ParentEvent`)
  + translate → core → **total in-order effects drain** (drain never skips
  effects, even on save failure — ledger/TUI-mirror consistency depends on it).
- `src/agent/task/ledger.rs` (pure, sequential `TaskId`) +
  `src/agent/task/executor.rs` (JoinSet shell).
- Multiplier-submit deadlock fixed (replica spawning runs inside an executor
  task); integration test through the real router in `loop_tests.rs`.

## what's still impure (the purification targets)

1. **Subagent lifecycle** — entirely outside the core:
   - `router/spawn.rs::spawn_subagent_async`: snapshot parent → build child
     state → duplicate workdir → spawn loop → register.
   - `subagent/mod.rs::spawn_and_submit` + `SubagentHandle::wait` (awaits the
     TurnResult oneshot, then router delete + unmount + diff).
   - `replica::run_replicas` + the `Effect::StartReplicas` interpreter body.
   - The parent core knows nothing about its children → no core-level
     await/abort/steer/follow-up is possible.
2. **Tool protocol** — `Effect::RunTool` runs `call.task.run(ctx)` opaquely;
   the tool mutates itself and the whole `ToolCallItem` round-trips as one
   Item event. The subagent tool (`tools/subagent.rs`) hides an entire child
   lifecycle inside `Function::call`, blocking one executor task per child —
   invisible to the core, not individually abortable, not steerable.
3. Minor: `Effect::Duplicate` → `try_duplicate` (shell), router actor state.

## features driving the design (`wip_multiagent.md` core list)

- async subagents: spawn returns immediately; await/abort as separate actions
- primary sends a follow-up after a subagent finishes the original request
- sibling awareness: dev msg showing what other subagents work on (≥ prompts)
- children ask parents for clarification (note: not necessarily a literal
  prompt into the parent loop — can be a parallel one-shot turn)
- primaries creating primaries
- user observe + steer subagents

Related backlog items that interact (decide in planning pass whether they ride
along): abort individual tool calls without aborting the turn; streaming
subagent progress in the TUI; "tools/subagents in progress" pane; steering
submit mode; subagent tool `skills` field.

## design direction (sketch — refine in planning pass)

Subagent lifecycle becomes core state + vocabulary, e.g.:

- `AgentCore` gains a child registry (child id, prompt, phase: spawning /
  running / done(result) / failed).
- New `CoreEvent`s: `ChildStarted`, `ChildDone(id, result)`, `ChildMessage(…)`.
- New `Effect`s: `SpawnChild { … }`, `SubmitChild { … }`, `AbortChild(id)`.
- The shell/router interprets spawn effects (workdir, registration stay IO);
  results flow back as events instead of being awaited inline.
- The subagent **tool** becomes a thin adapter over this protocol: its call
  resolves when the core sees `ChildDone`, instead of blocking a task on
  `spawn_and_submit().wait()`. Replicas (`StartReplicas`) re-expressed as
  spawn-N + a join condition in core, killing the special case.
- Unified tool protocol = long-running tools (subagent first, bash-abort
  later) report lifecycle through core events rather than one opaque
  run-to-completion future.

## open questions for the planning pass

1. **Feature slice**: which of the six features land in phase 4, which later?
2. **Persistence**: do children survive restart (child registry in persisted
   `AgentState`) or are they ephemeral (lost on crash, like tasks today)?
3. **Tool schema**: does `subagent` stay one blocking-looking tool backed by
   the new protocol, or split into spawn/await/abort tools (async subagents
   imply the latter or a `wait: bool` arg)?
4. **Ledger vs children**: reuse `TaskLedger` for child-bound work or a
   separate registry keyed by `AgentId`?
5. **Router's role**: stays the registry + spawn executor (core only emits
   effects), or shrinks further?
6. **Wire protocol**: what new `ParentEvent`s does observe/steer need, and how
   does the TUI mirror subagent state (per-tab? nested in parent tab?).

## invariants to preserve

- `ParentEvent` stream stays total and in-order; effects drained fully before
  the next event; one event per loop iteration.
- Core purity: `grep -n 'now()\|std::fs\|\.await\|tokio::' src/agent/core.rs
  src/agent/task/ledger.rs` stays empty; new decision logic gets sync snapshot
  tests (use the existing `(status, effects)` macros).
- Persisted `AgentState` changes only deliberately (question 2).
- Existing suite (169 tests) stays green; integration tests through the real
  router for anything touching spawn (multiplier test is the template).

## suggested sequencing (refine after questions resolved)

1. Planning pass → final implementation plan (plan mode).
2. Child registry + lifecycle events in core, subagent tool + replicas rewired
   on top — behavior-identical, no new features.
3. Async spawn/await/abort + follow-up-after-child-done.
4. Observe/steer: wire events + TUI.
5. Sibling awareness + parent clarifications.

## prerequisites

- User review of `wip/testability` (possible refactors) → merge into main.
