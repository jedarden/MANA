//! Hook handlers for Claude Code integration
//!
//! Pre-hooks inject context from ReasoningBank before tool execution.
//! Session-end hooks trigger learning when threshold is met.

use anyhow::Result;
use tracing::debug;

mod context_injection;
mod session_end_handler;

pub use context_injection::inject_context;
pub use session_end_handler::session_end;
