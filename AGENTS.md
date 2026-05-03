# code

- instead of `pub(crate)`, you MUST use `pub`

# tests

- you SHOULD try using snapshot tests using `insta` instead of manual assertions
  - if some parts of the snapshot is unstable (e.g. timestamps), use redactions;
  - prefer `assert_yaml_snapshot!` over `assert_json_snapshot!`, unless we're specifically testing serialization to json
  - prefer inline snapshots over snapshot files
- if snapshots are a bad fit, instead of built-in `assert_eq!()` macro you MUST
  use `similar_asserts::assert_eq!()`; when possible, you MUST compare the
  entire struct at once using assert_eq, instead of checking field by field

# build

- when building/testing, you MUST use `cargo ...` directly; if not available,
  you MUST fall back to `nix develop -c '...'`
