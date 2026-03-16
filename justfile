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

# nuke app data
clean:
    [ -d '{{ data_dir }}' ] && fd --glob workdir --exact-depth 4 --search-path '{{ data_dir }}' -x umount || true
    rm -rf '{{ data_dir }}'
    rm '{{ state_dir }}'/* || true
    git worktree prune
    git branch --format='%(refname:short)' | grep '^vc-' | xargs -I{} git branch -D {}

profile:
    flamegraph -- ./target/release/vc

build:
    RUSTFLAGS="-C debuginfo=1" cargo build --release

# init the config
config:
    mkdir -p ~/.config/vicode
    cp config.toml ~/.config/vicode/config.toml

test:
    cargo test

fmt:
    nix fmt
