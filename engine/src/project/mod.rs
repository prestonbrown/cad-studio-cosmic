//! Project types (spec §4.2): the manifest decoder lives in [`manifest`],
//! the pane layout tree in [`layout`], persistence in [`store`], the
//! parameter panel's model.py rewrite in [`parameters`].

pub mod layout;
pub mod manifest;
pub mod outputs;
pub mod parameters;
pub mod store;

pub use manifest::{BBox, Drawing, Manifest, PartRef};
pub use outputs::MissingCtx;
pub use store::{
    Action, ChatMessage, ProjectMetadata, ProjectNameError, ProjectStoreError, RecentProjects,
    Role, ToolActivity, ToolState, create_project, load_chat, load_metadata, save_chat,
    save_metadata, validate_project_name,
};
