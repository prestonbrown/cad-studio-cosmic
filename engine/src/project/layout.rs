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

impl LayoutNode {
    /// Upstream `LayoutNode.chatLeading` (PanelLayout.swift:26-31): chat on
    /// the leading third, 3D over schematic to its right.
    pub fn chat_leading() -> Self {
        Self::split(
            Axis::Row,
            Self::Pane(PaneKind::Chat),
            Self::split(
                Axis::Column,
                Self::Pane(PaneKind::Model3D),
                Self::Pane(PaneKind::Schematic),
                0.6,
            ),
            0.34,
        )
    }

    /// Upstream `LayoutNode.chatTrailing` (PanelLayout.swift:33-38).
    pub fn chat_trailing() -> Self {
        Self::split(
            Axis::Row,
            Self::split(
                Axis::Column,
                Self::Pane(PaneKind::Model3D),
                Self::Pane(PaneKind::Schematic),
                0.6,
            ),
            Self::Pane(PaneKind::Chat),
            0.66,
        )
    }

    /// Upstream `LayoutNode.chatBottom` (PanelLayout.swift:40-45).
    pub fn chat_bottom() -> Self {
        Self::split(
            Axis::Column,
            Self::split(
                Axis::Row,
                Self::Pane(PaneKind::Model3D),
                Self::Pane(PaneKind::Schematic),
                0.6,
            ),
            Self::Pane(PaneKind::Chat),
            0.62,
        )
    }

    /// Upstream `LayoutNode.threeColumns` (PanelLayout.swift:47-52).
    pub fn three_columns() -> Self {
        Self::split(
            Axis::Row,
            Self::Pane(PaneKind::Chat),
            Self::split(
                Axis::Row,
                Self::Pane(PaneKind::Model3D),
                Self::Pane(PaneKind::Schematic),
                0.55,
            ),
            0.28,
        )
    }

    fn split(axis: Axis, first: LayoutNode, second: LayoutNode, fraction: f64) -> Self {
        Self::Split {
            axis,
            first: Box::new(first),
            second: Box::new(second),
            fraction,
        }
    }
}

/// Upstream `PaneSplitViewController.minimumLength(of:axis:)`
/// (PaneSplitViewController.swift:71-76), evaluated per split as the tree is
/// descended: a bare chat pane on a row split keeps 300 px, any other child
/// keeps 240 px in rows and 140 px in columns.
pub fn min_thickness(child: &LayoutNode, axis: Axis) -> f64 {
    match (child, axis) {
        (LayoutNode::Pane(PaneKind::Chat), Axis::Row) => 300.0,
        (_, Axis::Row) => 240.0,
        (_, Axis::Column) => 140.0,
    }
}
