//! the inter-agent tools: ordinary tools dispatched by the agent's
//! `run_tool_call` → `call(ctx)`, each a thin client of the matching `ctx.router` op. Caller
//! identity comes from the runtime context, never a tool argument.

pub mod archive;
pub mod list;
pub mod send;
pub mod spawn;
