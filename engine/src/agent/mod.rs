//! Agent-side modules: executable discovery and spawn environment
//! ([`locator`]), tool-use summaries ([`tool_summary`]), the claude runner's
//! argv and prompt assembly ([`claude`]). Stream decoding and the session
//! driver arrive with the later M2 tasks.

pub mod claude;
pub mod locator;
pub mod tool_summary;
