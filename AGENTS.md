# code

- instead of `pub(crate)`, you MUST use `pub`

# tests

- instead of built-in `assert_eq!()` macro you MUST use `similar_asserts::assert_eq!()`
- when possible, you MUST compare the entire struct at once using assert_eq, instead of checking field by field

# build

- when building/testing, you MUST use `cargo ...` directly; if not available, you MUST fall back to `nix develop -c '...'`

# misc

- you MUST NOT remove existing comments, unless they're outdated. if you do, you MUST inform the user.
