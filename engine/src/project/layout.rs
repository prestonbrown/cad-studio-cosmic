//! Pane layout tree (upstream LayoutNode). The JSON shape is load-bearing for
//! macOS interop: `{"pane":"chat"}` and
//! `{"split":{"axis":"row","first":…,"second":…,"fraction":…}}`, with
//! `PaneKind::Model3D` spelled exactly `"model3D"`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneKind {
    #[serde(rename = "chat")]
    Chat,
    #[serde(rename = "model3D")]
    Model3D,
    #[serde(rename = "schematic")]
    Schematic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    #[serde(rename = "row")]
    Row,
    #[serde(rename = "column")]
    Column,
}

/// Externally tagged with lowercased variant names: the newtype variant gives
/// `{"pane":<kind>}` and the struct variant's fields land inside the tag,
/// matching the upstream encoder without hand-written serde.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutNode {
    Pane(PaneKind),
    Split {
        axis: Axis,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
        fraction: f64,
    },
}
