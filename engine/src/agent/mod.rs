//! Agent-side modules: executable discovery and spawn environment
//! ([`locator`]), tool-use summaries ([`tool_summary`]), stream-event decoding
//! for agent JSONL output ([`stream_event`]), the claude runner's argv and
//! prompt assembly ([`claude`]), and the single-turn session state machine
//! ([`session`]). The busy/missing/auth lifecycle dance around it arrives with
//! the next M2 task.

pub mod claude;
pub mod locator;
pub mod session;
pub mod stream_event;
pub mod tool_summary;
