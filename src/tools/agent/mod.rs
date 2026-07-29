//! the inter-agent tools (§2.3): ordinary tools riding `Effect::RunTool` →
//! `call(ctx)`, each a thin client of the matching `ctx.router` op. Caller
//! identity comes from the runtime context, never a tool argument.

pub mod archive;
pub mod inspect;
pub mod list;
pub mod send;
pub mod spawn;
pub mod wait;
