# vicode

[![built with garnix](https://img.shields.io/endpoint.svg?url=https%3A%2F%2Fgarnix.io%2Fapi%2Fbadges%2Fanuramat%2Fvicode%3Fbranch%3Dmain)](https://garnix.io/repo/anuramat/vicode)

coding agent with tabs/subagents running in overlayfs-backed worktrees:
create/fork tabs to work on multiple features/implementations, while subagents
work in parallel without conflicts; share the compilation cache between agents,
so that `cargo check` doesn't take minutes

since it's all just mounts, vicode works as a worktree manager as well: select a
vicode tab, open a new terminal tab/window, and run claude/codex inside

https://github.com/user-attachments/assets/e481101e-1dd6-4161-a345-2fe4ce4eacf6

## features

- multi-agent workflows: tabs, parallel subagents, best-of-n
- per-agent fs isolation: git worktrees + fuse-overlayfs + bindfs ([details](src/project/README.md))
- sandboxing: bwrap/sandbox-exec
- API support: responses, chat completions, ChatGPT subscription login
- OSC7: new terminal windows land in the worktree of the selected tab

## getting started

```bash
nix run github:anuramat/vicode
```

or with cargo (assuming you have bindfs, fuse-overlayfs, and bwrap in PATH):

```
git clone https://github.com/anuramat/vicode
cd vicode
cargo run --release
```

example config in `./default/config.toml`

## disclaimer

- linux only atm, mac build WIP
- no backwards-compat guarantees
- some modules were vibecoded -- grep for `SLOP`
- unwraps
