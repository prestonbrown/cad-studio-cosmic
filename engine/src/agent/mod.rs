//! Agent-side modules: executable discovery and spawn environment
//! ([`locator`]), tool-use summaries ([`tool_summary`]), and stream-event
//! decoding for agent JSONL output ([`stream_event`]). The claude runner and
//! the session driver arrive with the later M2 tasks.

pub mod locator;
pub mod stream_event;
pub mod tool_summary;
