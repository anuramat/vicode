//! structural purity guard (§5): `AgentCore` decides, `Agent` acts. The
//! core produces `Effect`s and must never perform I/O itself — no async, no
//! awaits, no process/tokio/fs calls. Scanned as source text (kept in a
//! separate file so the forbidden literals here don't match themselves) so a
//! regression fails the build, not just review.
#![cfg(test)]

#[test]
fn core_is_pure() {
    let src = include_str!("core.rs");
    for forbidden in [
        "async fn",
        ".await",
        "tokio::",
        "Command::",
        "std::process",
        "fs::",
    ] {
        assert!(
            !src.contains(forbidden),
            "src/agent/core.rs violates §5 purity: found `{forbidden}` — side \
             effects belong in `Agent`, expressed as Effects"
        );
    }
}
