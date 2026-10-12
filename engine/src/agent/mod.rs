//! Agent-side modules: executable discovery and spawn environment
//! ([`locator`]), tool-use summaries ([`tool_summary`]), stream-event decoding
//! for agent JSONL output ([`stream_event`]), and the claude runner's argv and
//! prompt assembly ([`claude`]). The session driver arrives with the last M2
//! task.

pub mod claude;
pub mod locator;
pub mod stream_event;
pub mod tool_summary;
