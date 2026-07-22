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

## code

- instead of `pub(crate)`, you MUST use `pub`

## tests

- you SHOULD try using snapshot tests using `insta` instead of manual assertions
  - if some parts of the snapshot is unstable (e.g. timestamps), use redactions;
  - prefer `assert_yaml_snapshot!` over `assert_json_snapshot!`, unless we're specifically testing serialization to json
  - prefer inline snapshots over snapshot files
- if snapshots are a bad fit, instead of built-in `assert_eq!()` macro you MUST
  use `similar_asserts::assert_eq!()`; when possible, you MUST compare the
  entire struct at once using assert_eq, instead of checking field by field
- test-only files MUST instead start with an inner `#![cfg(test)]`; the `mod`
  declaration in the parent stays unconditional
- a normal file MUST contain at most one `#[cfg(test)]` item: a `mod tests`
  block at the end of the file
  - exception: test-only enum variants and their match arms are gated in place

## build

- when building/testing, you MUST use `cargo ...` directly; if not available,
  you MUST fall back to `nix develop -c '...'`

## todo files

directory `./todo/` contains markdown files that describe (potential) future
changes:

- `backlog.md`: "must have" features/changes/fixes/improvements
- `wip_*.md`: WIP spec drafts for new complex features. details might change,
  but the basic idea will remain, thus these specs should already inform the
  architectural decisions when we implement other features.
- `maybe.md`: nice to have, but not sure if worth the effort; requires further analysis
