data_dir := env_var_or_default(
  "XDG_DATA_HOME",
  env_var("HOME") / ".local/share"
) / "vicode"
state_dir := env_var_or_default(
  "XDG_STATE_HOME",
  env_var("HOME") / ".local/state"
) / "vicode"

default: run

run:
    RUST_LOG=debug RUST_BACKTRACE=full cargo run

# hacky, we should add a command to the app instead
umount:
    #!/usr/bin/env bash
    [ -d '{{ data_dir }}' ] || exit 0
    # agent overlays
    fd --glob workdir --exact-depth 4 --search-path '{{ data_dir }}' --mount -x umount || true
    # shared lowerdirs; note that this assumes that moutns are in the root of shared dir
    fd --glob shared --exact-depth 2 --search-path '{{ data_dir }}' --mount -a | xargs -I{} fd . --exact-depth 1 --search-path '{}' --mount -a -x umount || true

clean_data: umount
    #!/usr/bin/env bash
    [ -d '{{ data_dir }}' ] || exit 0
    find '{{ data_dir }}' -mindepth 1 -mount -delete

clean_state:
    [ -d '{{ state_dir }}' ] || exit 0
    find '{{ state_dir }}' -mindepth 1 -delete

clean_git:
    # TODO delete worktrees aggressively
    git worktree prune
    git branch --format='%(refname:short)' | grep '^vc-' | xargs -I{} git branch -D {}

clean: clean_state clean_data clean_git

profile:
    flamegraph -- ./target/release/vc

build:
    RUSTFLAGS="-C debuginfo=1" cargo build --release

test:
    cargo test

fmt:
    nix fmt
