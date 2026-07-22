# AGENTS.md

note: `./CLAUDE.md` is a symlink to `./AGENTS.md`

## design goals

elevator pitch: assume unlimited tokens/compute, maximize quality/quantity of
agent output per human hour

basic ideas:

- minimal user input -- the only required user input is the initial task prompt,
  memory/agent/worktree management should just work OOTB
- tokenmaxxing -- we try to maximize per-prompt success rate at any cost
- principled memory -- propose memories that could help, benchmark on past
  sessions, keep only what was proven to be useful
- performance -- cheap agent isolation, unconditionally fast UI
- no feature creep -- codebase should be human maintainable, so complex
  convenience features are mostly out of scope (except for the absolute
  necessities)
- no hacks -- agent architecture should be clean and general, it should not
  impose arbitrary restrictions or hardcode specific workflows: frontier models
  are smart, we should let them loose (with some exceptions, notably -- memory
  system); in other words, focus on simplest possible version that could work
  well, optimize for the best case scenario

## todo files

directory `./todo/` contains markdown files that describe (potential) future
changes:

- `backlog.md`: "must have" features/changes/fixes/improvements
- `wip_*.md`: WIP spec drafts for new complex features. details might change,
  but the basic idea will remain, thus these specs should already inform the
  architectural decisions when we implement other features.
- `maybe.md`: nice to have, but not sure if worth the effort; requires further analysis
