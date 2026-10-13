//! Project types (spec §4.2): the manifest decoder lives in [`manifest`],
//! the pane layout tree in [`layout`], persistence in [`store`], the
//! version snapshots in [`versions`].

pub mod layout;
pub mod manifest;
pub mod outputs;
pub mod store;
pub mod versions;

pub use manifest::{BBox, Drawing, Manifest, PartRef};
pub use outputs::MissingCtx;
pub use store::{
    Action, ChatMessage, ProjectMetadata, ProjectNameError, ProjectStoreError, RecentProjects,
    Role, ToolActivity, ToolState, create_project, load_chat, load_metadata, save_chat,
    save_metadata, validate_project_name,
};
