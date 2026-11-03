//! Pane layout tree (upstream LayoutNode). The JSON shape is load-bearing for
//! macOS interop — see [`LayoutNode`] — with `PaneKind::Model3D` spelled
//! exactly `"model3D"`.

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

/// The pane layout tree. Serialized the way Swift's synthesized `Codable`
/// encodes upstream's `indirect enum LayoutNode { case pane(Pane); case
/// split(LayoutSplit) }`: an unlabeled associated value sits under the key
/// `_0`, so a leaf is `{"pane":{"_0":"chat"}}` and a split is
/// `{"split":{"_0":{"axis":…,"first":…,"second":…,"fraction":…}}}`.
/// Decoding also accepts the bare forms `{"pane":"chat"}` and
/// `{"split":{"axis":…}}` that earlier port builds wrote.
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutNode {
    Pane(PaneKind),
    Split {
        axis: Axis,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
        fraction: f64,
    },
}

#[derive(Serialize, Deserialize)]
struct SplitFields {
    axis: Axis,
    first: Box<LayoutNode>,
    second: Box<LayoutNode>,
    fraction: f64,
}

/// Swift's wrapper around an unlabeled associated value.
#[derive(Serialize, Deserialize)]
struct Swift<T> {
    #[serde(rename = "_0")]
    value: T,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Payload<T> {
    Swift(Swift<T>),
    Bare(T),
}

impl<T> Payload<T> {
    fn into_inner(self) -> T {
        match self {
            Payload::Swift(Swift { value }) | Payload::Bare(value) => value,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum WireOut {
    Pane(Swift<PaneKind>),
    Split(Swift<SplitFields>),
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireIn {
    Pane(Payload<PaneKind>),
    Split(Payload<SplitFields>),
}

impl Serialize for LayoutNode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self.clone() {
            LayoutNode::Pane(kind) => WireOut::Pane(Swift { value: kind }),
            LayoutNode::Split {
                axis,
                first,
                second,
                fraction,
            } => WireOut::Split(Swift {
                value: SplitFields {
                    axis,
                    first,
                    second,
                    fraction,
                },
            }),
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LayoutNode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match WireIn::deserialize(deserializer)? {
            WireIn::Pane(payload) => LayoutNode::Pane(payload.into_inner()),
            WireIn::Split(payload) => {
                let SplitFields {
                    axis,
                    first,
                    second,
                    fraction,
                } = payload.into_inner();
                LayoutNode::Split {
                    axis,
                    first,
                    second,
                    fraction,
                }
            }
        })
    }
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
