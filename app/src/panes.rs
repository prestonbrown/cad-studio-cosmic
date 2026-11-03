//! Pane-grid ↔ layout-tree conversion, resize clamping, and the layout
//! persistence policy. Everything except the [`pane_grid::State`] conversion
//! is plain functions over engine types, so the behavior is testable without a
//! window; Task 7's window assembly is a thin caller of this module.

use cosmic::widget::pane_grid::{self, Configuration, Node, Split, State};
use engine::project::layout::{self, Axis, LayoutNode, PaneKind};
use engine::project::store::{ProjectMetadata, ProjectStoreError, load_metadata, save_metadata};
use engine::settings::{Settings, SettingsError};

use crate::settings_store::SettingsStore;
use std::path::Path;

/// Convert a persisted layout tree into PaneGrid state.
pub fn from_layout(node: &LayoutNode) -> State<PaneKind> {
    State::with_configuration(to_configuration(node))
}

/// Convert PaneGrid state back into the persisted tree shape.
pub fn to_layout(state: &State<PaneKind>) -> LayoutNode {
    node_to_layout(state.layout(), state)
}

/// The layout a new project opens with: the stored default layout, or
/// chatLeading when nothing (or something undecodable) is stored
/// (AppSettings.swift:67-71).
pub fn stored_layout(settings: &Settings) -> LayoutNode {
    settings
        .default_layout
        .as_ref()
        .and_then(|v| serde_json::from_value::<LayoutNode>(v.clone()).ok())
        .unwrap_or_else(LayoutNode::chat_leading)
}

/// Upstream `LayoutNode.removing(_:)` (PanelLayout.swift:74-82): the tree
/// without `pane`. A split whose child vanishes collapses to its other side;
/// `None` only when the whole tree was that pane (the caller keeps the full
/// layout then — upstream `displayedLayout`'s `?? layout`).
pub fn removing(node: &LayoutNode, pane: PaneKind) -> Option<LayoutNode> {
    match node {
        LayoutNode::Pane(current) => {
            if *current == pane {
                None
            } else {
                Some(node.clone())
            }
        }
        LayoutNode::Split {
            axis,
            first,
            second,
            fraction,
        } => match (removing(first, pane), removing(second, pane)) {
            (Some(first), Some(second)) => Some(LayoutNode::Split {
                axis: *axis,
                first: Box::new(first),
                second: Box::new(second),
                fraction: *fraction,
            }),
            (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
            (None, None) => None,
        },
    }
}

/// Upstream `LayoutNode.replacing(_:with:)` (PanelLayout.swift:117-126).
fn replacing(node: &LayoutNode, target: PaneKind, with: &LayoutNode) -> LayoutNode {
    match node {
        LayoutNode::Pane(current) => {
            if *current == target {
                with.clone()
            } else {
                node.clone()
            }
        }
        LayoutNode::Split {
            axis,
            first,
            second,
            fraction,
        } => LayoutNode::Split {
            axis: *axis,
            first: Box::new(replacing(first, target, with)),
            second: Box::new(replacing(second, target, with)),
            fraction: *fraction,
        },
    }
}

/// Where a dragged pane can land relative to a target (upstream `DropEdge`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropEdge {
    Leading,
    Trailing,
    Top,
    Bottom,
}

/// Upstream `LayoutNode.moving(_:to:of:)` (PanelLayout.swift:84-98): pull
/// `pane` out, then re-insert it on `edge` of `target` — the dragged chat
/// pane takes a 0.34 share, anything else 0.5.
pub fn moving(node: &LayoutNode, pane: PaneKind, edge: DropEdge, target: PaneKind) -> LayoutNode {
    if pane == target {
        return node.clone();
    }
    let Some(remaining) = removing(node, pane) else {
        return node.clone();
    };
    let places_first = matches!(edge, DropEdge::Leading | DropEdge::Top);
    let axis = match edge {
        DropEdge::Leading | DropEdge::Trailing => Axis::Row,
        DropEdge::Top | DropEdge::Bottom => Axis::Column,
    };
    let moved = LayoutNode::Pane(pane);
    let stays = LayoutNode::Pane(target);
    let moved_share = if pane == PaneKind::Chat { 0.34 } else { 0.5 };
    let split = LayoutNode::Split {
        axis,
        first: Box::new(if places_first {
            moved.clone()
        } else {
            stays.clone()
        }),
        second: Box::new(if places_first {
            stays.clone()
        } else {
            moved.clone()
        }),
        fraction: if places_first {
            moved_share
        } else {
            1.0 - moved_share
        },
    };
    replacing(&remaining, target, &split)
}

/// The tree the grid shows (upstream `displayedLayout`,
/// Project.swift:132-134): the full layout, minus the chat pane while it is
/// hidden — falling back to the full layout when chat is the only pane
/// left, which cannot happen with the three-pane presets but keeps the
/// invariant total.
pub fn displayed(node: &LayoutNode, chat_visible: bool) -> LayoutNode {
    if chat_visible {
        node.clone()
    } else {
        removing(node, PaneKind::Chat).unwrap_or_else(|| node.clone())
    }
}

/// Clamp a drag ratio so both sides of `split` keep their upstream minimum
/// thickness (PaneSplitViewController.swift:71-76). `width`/`height` are the
/// PaneGrid's pixel size and `spacing` its divider thickness. A region too
/// small to satisfy both minimums returns the split's current ratio unchanged,
/// refusing the drag. iced's PaneGrid enforces only a single global
/// `min_size` and `State::resize` stores the raw event ratio, so this clamp
/// is the only enforcement of the per-split minimums: a resize handler must
/// pass every drag ratio through this before calling `State::resize`.
pub fn clamp_resize(
    state: &State<PaneKind>,
    split: &Split,
    proposed: f32,
    width: f32,
    height: f32,
    spacing: f32,
) -> f32 {
    let Some((found, region_w, region_h)) =
        find_split(state.layout(), split, width, height, spacing)
    else {
        return proposed;
    };
    let axis = from_grid_axis(found.axis);
    let extent = match found.axis {
        pane_grid::Axis::Horizontal => f64::from(region_h),
        pane_grid::Axis::Vertical => f64::from(region_w),
    };
    let first = node_to_layout(found.a, state);
    let second = node_to_layout(found.b, state);
    let half = f64::from(spacing) / 2.0;
    let lower = (layout::min_thickness(&first, axis) + half) / extent;
    let upper = 1.0 - (layout::min_thickness(&second, axis) + half) / extent;
    if lower > upper {
        return found.ratio;
    }
    f64::from(proposed).clamp(lower, upper) as f32
}

#[derive(Debug, thiserror::Error)]
pub enum PersistLayoutError {
    #[error(transparent)]
    Store(#[from] ProjectStoreError),
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// A fraction (drag-resize) change: stage the new tree in memory only
/// (upstream `setFraction`, Project.swift:142-146). The disk is untouched —
/// the established save points (window close, turn-end saves) flush it later.
pub fn set_fraction(metadata: &mut ProjectMetadata, node: &LayoutNode) {
    metadata.layout = Some(node.clone());
}

/// Persist a structure change (preset switch, pane move, visibility) the
/// moment it happens (upstream `setLayout` → `persistLayout`,
/// Project.swift:139, 163-167): the project manifest's `layout` and the
/// global default layout both update, and project.json is rewritten
/// immediately.
pub fn persist_layout(
    project_root: &Path,
    settings: &SettingsStore,
    node: &LayoutNode,
) -> Result<(), PersistLayoutError> {
    let mut metadata = load_metadata(project_root)?;
    metadata.layout = Some(node.clone());
    save_metadata(project_root, &metadata)?;
    let layout = serde_json::to_value(node)?;
    settings.update(|settings| settings.default_layout = Some(layout))?;
    Ok(())
}

/// Upstream `row` places panes side by side (a vertical divider); iced's
/// `Axis::Vertical` is the axis that divides width — so the names invert
/// across the seam.
fn to_grid_axis(axis: Axis) -> pane_grid::Axis {
    match axis {
        Axis::Row => pane_grid::Axis::Vertical,
        Axis::Column => pane_grid::Axis::Horizontal,
    }
}

fn from_grid_axis(axis: pane_grid::Axis) -> Axis {
    match axis {
        pane_grid::Axis::Vertical => Axis::Row,
        pane_grid::Axis::Horizontal => Axis::Column,
    }
}

fn to_configuration(node: &LayoutNode) -> Configuration<PaneKind> {
    match node {
        LayoutNode::Pane(kind) => Configuration::Pane(*kind),
        LayoutNode::Split {
            axis,
            first,
            second,
            fraction,
        } => Configuration::Split {
            axis: to_grid_axis(*axis),
            ratio: *fraction as f32,
            a: Box::new(to_configuration(first)),
            b: Box::new(to_configuration(second)),
        },
    }
}

fn node_to_layout(node: &Node, state: &State<PaneKind>) -> LayoutNode {
    match node {
        Node::Pane(pane) => LayoutNode::Pane(
            *state
                .panes
                .get(pane)
                .expect("every pane id in the tree has state"),
        ),
        Node::Split {
            axis, ratio, a, b, ..
        } => LayoutNode::Split {
            axis: from_grid_axis(*axis),
            first: Box::new(node_to_layout(a, state)),
            second: Box::new(node_to_layout(b, state)),
            fraction: ratio_as_fraction(*ratio),
        },
    }
}

/// The widget stores ratios as f32 while the persisted tree carries f64; a
/// value that survives untouched must keep its short decimal form (0.34, not
/// 0.3400000035762787), so ratios are rounded to 7 significant digits — an
/// f32 carries ~7 — on the way out. That costs sub-pixel precision on real
/// drags and nothing else.
fn ratio_as_fraction(ratio: f32) -> f64 {
    let x = f64::from(ratio);
    if x == 0.0 {
        return 0.0;
    }
    let magnitude = x.abs().log10().floor() as i32;
    let factor = 10f64.powi(7 - 1 - magnitude);
    (x * factor).round() / factor
}

struct SplitRef<'a> {
    axis: pane_grid::Axis,
    ratio: f32,
    a: &'a Node,
    b: &'a Node,
}

/// Find `target` in the tree and report its region size. Descending follows
/// iced's split arithmetic: each side of a split loses half the divider
/// spacing along the split's axis.
fn find_split<'a>(
    node: &'a Node,
    target: &Split,
    width: f32,
    height: f32,
    spacing: f32,
) -> Option<(SplitRef<'a>, f32, f32)> {
    match node {
        Node::Pane(_) => None,
        Node::Split {
            id,
            axis,
            ratio,
            a,
            b,
        } => {
            if id == target {
                return Some((
                    SplitRef {
                        axis: *axis,
                        ratio: *ratio,
                        a,
                        b,
                    },
                    width,
                    height,
                ));
            }
            let half = spacing / 2.0;
            let ((wa, ha), (wb, hb)) = match axis {
                pane_grid::Axis::Horizontal => (
                    (width, height * ratio - half),
                    (width, height * (1.0 - ratio) - half),
                ),
                pane_grid::Axis::Vertical => (
                    (width * ratio - half, height),
                    (width * (1.0 - ratio) - half, height),
                ),
            };
            find_split(a, target, wa, ha, spacing)
                .or_else(|| find_split(b, target, wb, hb, spacing))
        }
    }
}

// ─── pane drag (PaneContainer.swift + PaneSplitViewController.locate) ──────

/// An in-flight pane drag (upstream `PaneDrag`): the pane being dragged and,
/// once the cursor is over ANOTHER pane, that target and its nearest edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneDrag {
    pub pane: PaneKind,
    pub target: Option<PaneKind>,
    pub edge: Option<DropEdge>,
}

/// The pane under `point` and the edge nearest it
/// (PaneSplitViewController.swift:38-51): the smallest of the normalized
/// distances to leading, trailing, top, bottom wins, in that order on ties.
pub fn locate<K: Copy>(
    regions: impl IntoIterator<Item = (K, cosmic::iced::Rectangle)>,
    point: cosmic::iced::Point,
) -> Option<(K, DropEdge)> {
    regions.into_iter().find_map(|(kind, bounds)| {
        if !bounds.contains(point) || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return None;
        }
        let x = (point.x - bounds.x) / bounds.width;
        let y = (point.y - bounds.y) / bounds.height;
        let distances = [
            (DropEdge::Leading, x),
            (DropEdge::Trailing, 1.0 - x),
            (DropEdge::Top, y),
            (DropEdge::Bottom, 1.0 - y),
        ];
        let edge = distances
            .iter()
            .fold(None::<(DropEdge, f32)>, |best, &(edge, d)| match best {
                Some((_, b)) if b <= d => best,
                _ => Some((edge, d)),
            })
            .map_or(DropEdge::Trailing, |(edge, _)| edge);
        Some((kind, edge))
    })
}

/// Upstream `dragPane(_:to:)` (Project.swift:148-154): hovering the dragged
/// pane itself (or no pane) leaves no target.
pub fn drag_to(pane: PaneKind, hit: Option<(PaneKind, DropEdge)>) -> PaneDrag {
    match hit {
        Some((target, edge)) if target != pane => PaneDrag {
            pane,
            target: Some(target),
            edge: Some(edge),
        },
        _ => PaneDrag {
            pane,
            target: None,
            edge: None,
        },
    }
}

/// Upstream `dropPane()` (Project.swift:156-160): the layout a release
/// lands, or `None` when the drag has no target.
pub fn drop(node: &LayoutNode, drag: &PaneDrag) -> Option<LayoutNode> {
    let (target, edge) = (drag.target?, drag.edge?);
    Some(moving(node, drag.pane, edge, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::project::store::{load_metadata, save_metadata};
    use std::path::PathBuf;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cad-panes-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Hiding chat collapses the tree to the 3D-over-schematic split with
    /// the stored fraction (upstream `removing(.chat)` +
    /// `displayedLayout`); re-showing restores the full layout untouched.
    #[test]
    fn hiding_chat_collapses_the_tree() {
        let layout = LayoutNode::chat_leading();
        let shown = displayed(&layout, true);
        assert_eq!(shown, layout);

        let hidden = displayed(&layout, false);
        assert_eq!(
            hidden,
            LayoutNode::Split {
                axis: Axis::Column,
                first: Box::new(LayoutNode::Pane(PaneKind::Model3D)),
                second: Box::new(LayoutNode::Pane(PaneKind::Schematic)),
                fraction: 0.6,
            }
        );
        // a bare chat pane has nothing to fall back to: the full layout stays
        let only = LayoutNode::Pane(PaneKind::Chat);
        assert_eq!(displayed(&only, false), only);
    }

    /// A pane move pulls the pane out and re-inserts it against its target
    /// with the moved share (0.34 for chat, 0.5 otherwise), placing it
    /// first on leading/top edges (upstream `moving`, PanelLayout.swift:84-98).
    #[test]
    fn moving_reinserts_on_the_target_edge() {
        let layout = LayoutNode::chat_leading();

        // schematic to the chat pane's leading edge: row split, schematic
        // first with the non-chat 0.5 share
        let moved = moving(
            &layout,
            PaneKind::Schematic,
            DropEdge::Leading,
            PaneKind::Chat,
        );
        assert_eq!(
            moved,
            LayoutNode::Split {
                axis: Axis::Row,
                first: Box::new(LayoutNode::Split {
                    axis: Axis::Row,
                    first: Box::new(LayoutNode::Pane(PaneKind::Schematic)),
                    second: Box::new(LayoutNode::Pane(PaneKind::Chat)),
                    fraction: 0.5,
                }),
                second: Box::new(LayoutNode::Pane(PaneKind::Model3D)),
                fraction: 0.34,
            }
        );

        // chat under the 3D pane: the new column split REPLACES the 3D pane
        // where it stood, nested under the old 3D-over-schematic split
        // (upstream does not flatten), chat last with the 0.66 tail
        let moved = moving(&layout, PaneKind::Chat, DropEdge::Bottom, PaneKind::Model3D);
        assert_eq!(
            moved,
            LayoutNode::Split {
                axis: Axis::Column,
                first: Box::new(LayoutNode::Split {
                    axis: Axis::Column,
                    first: Box::new(LayoutNode::Pane(PaneKind::Model3D)),
                    second: Box::new(LayoutNode::Pane(PaneKind::Chat)),
                    fraction: 1.0 - 0.34,
                }),
                second: Box::new(LayoutNode::Pane(PaneKind::Schematic)),
                fraction: 0.6,
            }
        );

        // dropping a pane on itself is a no-op
        assert_eq!(
            moving(&layout, PaneKind::Chat, DropEdge::Top, PaneKind::Chat),
            layout
        );
    }

    #[test]
    fn from_to_layout_round_trip_preserves_fractions() {
        for preset in [
            LayoutNode::chat_leading(),
            LayoutNode::chat_trailing(),
            LayoutNode::chat_bottom(),
            LayoutNode::three_columns(),
        ] {
            let state = from_layout(&preset);
            assert_eq!(state.len(), 3, "three panes in {preset:?}");
            assert_eq!(
                to_layout(&state),
                preset,
                "the f32 pane-grid seam must not smear preset fractions"
            );
        }
    }

    #[test]
    fn grid_axis_inverts_row_column() {
        // Upstream `row` places panes side by side (a vertical divider);
        // iced's `Axis::Vertical` is the one that divides width — so the
        // names invert across the seam.
        let chat_leading = from_layout(&LayoutNode::chat_leading());
        match chat_leading.layout() {
            Node::Split { axis, .. } => assert_eq!(*axis, pane_grid::Axis::Vertical),
            _ => panic!("chatLeading root is a split"),
        }
        let chat_bottom = from_layout(&LayoutNode::chat_bottom());
        match chat_bottom.layout() {
            Node::Split { axis, .. } => assert_eq!(*axis, pane_grid::Axis::Horizontal),
            _ => panic!("chatBottom root is a split"),
        }
    }

    #[test]
    fn resize_clamps_min_thickness() {
        let spacing = 8.0_f32;
        let half = f64::from(spacing) / 2.0;

        // Root split of chatLeading: chat (min 300) | 3D-over-schematic (min 240).
        let root = match from_layout(&LayoutNode::chat_leading()).layout() {
            Node::Split { id, .. } => *id,
            _ => panic!("chatLeading root is a split"),
        };

        let mut state = from_layout(&LayoutNode::chat_leading());
        let clamped = clamp_resize(&state, &root, 0.0, 1400.0, 800.0, spacing);
        let lo = (300.0 + half) / 1400.0;
        assert!(
            f64::from(clamped) >= lo - 1e-6,
            "chat below its 300px minimum: {clamped} < {lo}"
        );
        state.resize(root, clamped);
        match to_layout(&state) {
            LayoutNode::Split { fraction, .. } => assert!(
                1400.0 * fraction - half >= 300.0 - 1e-3,
                "chat pane below 300: {fraction}"
            ),
            _ => panic!("root stays a split"),
        }

        let mut state = from_layout(&LayoutNode::chat_leading());
        let clamped = clamp_resize(&state, &root, 1.0, 1400.0, 800.0, spacing);
        let hi = 1.0 - (240.0 + half) / 1400.0;
        assert!(
            f64::from(clamped) <= hi + 1e-6,
            "right side below its 240px minimum: {clamped} > {hi}"
        );
        state.resize(root, clamped);
        match to_layout(&state) {
            LayoutNode::Split { fraction, .. } => assert!(
                1400.0 * (1.0 - fraction) - half >= 240.0 - 1e-3,
                "split side below 240: {fraction}"
            ),
            _ => panic!("root stays a split"),
        }

        // Nested column split (3D over schematic, both min 140): the root row
        // split divides only width, so the inner split's region is the full
        // 800px tall and its extent along the column axis is 800.
        let mut state = from_layout(&LayoutNode::chat_leading());
        let inner = match state.layout() {
            Node::Split { b, .. } => match &**b {
                Node::Split { id, .. } => *id,
                _ => panic!("chatLeading second child is a split"),
            },
            _ => panic!("root is a split"),
        };
        let clamped = clamp_resize(&state, &inner, 0.0, 1400.0, 800.0, spacing);
        let lo = (140.0 + half) / 800.0;
        assert!(
            f64::from(clamped) >= lo - 1e-6,
            "3D below its 140px minimum: {clamped} < {lo}"
        );
        state.resize(inner, clamped);
        let inner_fraction = match to_layout(&state) {
            LayoutNode::Split { second, .. } => match &*second {
                LayoutNode::Split { fraction, .. } => *fraction,
                _ => panic!("inner stays a split"),
            },
            _ => panic!("root stays a split"),
        };
        assert!(
            800.0 * inner_fraction - half >= 140.0 - 1e-3,
            "3D pane below 140: {inner_fraction}"
        );

        // A region too small for both minimums refuses the drag entirely.
        let state = from_layout(&LayoutNode::chat_leading());
        let kept = clamp_resize(&state, &root, 0.9, 500.0, 400.0, spacing);
        assert!(
            (f64::from(kept) - 0.34).abs() < 1e-6,
            "undersized region keeps the current ratio, got {kept}"
        );
    }

    /// Rebuild `base` with a new root fraction — what a drag produces.
    fn with_root_fraction(base: &LayoutNode, fraction: f64) -> LayoutNode {
        match base {
            LayoutNode::Split {
                axis,
                first,
                second,
                ..
            } => LayoutNode::Split {
                axis: *axis,
                first: first.clone(),
                second: second.clone(),
                fraction,
            },
            pane => pane.clone(),
        }
    }

    #[test]
    fn fraction_change_stays_in_memory_until_established_save() {
        let root = tmp_dir("fraction");
        let mut metadata = ProjectMetadata::new("0.1.0");
        let base = LayoutNode::chat_leading();
        metadata.layout = Some(base.clone());
        save_metadata(&root, &metadata).unwrap();
        let settings_path = root.join("settings.json");

        let resized = with_root_fraction(&base, 0.45);
        set_fraction(&mut metadata, &resized);

        assert_eq!(
            metadata.layout.as_ref(),
            Some(&resized),
            "model sees the drag"
        );
        assert_eq!(
            load_metadata(&root).unwrap().layout.as_ref(),
            Some(&base),
            "no disk write on a fraction change (Project.swift:142-146)"
        );
        assert!(
            !settings_path.exists(),
            "fraction changes do not touch the default layout"
        );

        // The established save points (window close, turn-end saves) flush it.
        save_metadata(&root, &metadata).unwrap();
        assert_eq!(
            load_metadata(&root).unwrap().layout.as_ref(),
            Some(&resized)
        );
    }

    #[test]
    fn structure_change_persists_eagerly() {
        let root = tmp_dir("structure");
        let metadata = ProjectMetadata::new("0.1.0");
        save_metadata(&root, &metadata).unwrap();
        let settings_path = root.join("settings.json");

        // Pane moves and preset switches land as different structures.
        for node in [
            LayoutNode::chat_trailing(),
            LayoutNode::three_columns(),
            LayoutNode::chat_bottom(),
        ] {
            persist_layout(&root, &SettingsStore::at(&settings_path), &node).unwrap();

            let reloaded = load_metadata(&root).unwrap();
            assert_eq!(
                reloaded.layout.as_ref(),
                Some(&node),
                "structure change must be on disk immediately"
            );
            // project.json stores whole-second timestamps; anything within
            // that truncation means created_at survived the load-modify-save.
            let drift = reloaded.created_at - metadata.created_at;
            assert!(
                drift.whole_milliseconds().abs() < 1000,
                "load-modify-save keeps the rest of the metadata"
            );

            let settings = Settings::load(&settings_path).unwrap();
            let stored = settings
                .default_layout
                .as_ref()
                .and_then(|v| serde_json::from_value::<LayoutNode>(v.clone()).ok());
            assert_eq!(stored.as_ref(), Some(&node), "default layout follows");
        }
    }

    #[test]
    fn stored_layout_falls_back_to_chat_leading() {
        let mut settings = Settings::default();
        assert_eq!(stored_layout(&settings), LayoutNode::chat_leading());

        settings.default_layout = Some(serde_json::json!({"pane": "schematic"}));
        assert_eq!(
            stored_layout(&settings),
            LayoutNode::Pane(PaneKind::Schematic)
        );

        settings.default_layout = Some(serde_json::json!({"bogus": true}));
        assert_eq!(
            stored_layout(&settings),
            LayoutNode::chat_leading(),
            "undecodable -> chatLeading (upstream `try? ?? .chatLeading`)"
        );
    }
    /// The nearest-edge rule (PaneSplitViewController.locate): normalized
    /// distances pick the edge; points outside every region hit nothing.
    #[test]
    fn locate_picks_the_pane_and_nearest_edge() {
        use cosmic::iced::{Point, Rectangle};
        let regions = [
            (
                PaneKind::Chat,
                Rectangle::new(Point::new(0.0, 0.0), (400.0, 800.0).into()),
            ),
            (
                PaneKind::Model3D,
                Rectangle::new(Point::new(408.0, 0.0), (600.0, 400.0).into()),
            ),
        ];
        assert_eq!(
            locate(regions, Point::new(20.0, 400.0)),
            Some((PaneKind::Chat, DropEdge::Leading))
        );
        assert_eq!(
            locate(regions, Point::new(390.0, 400.0)),
            Some((PaneKind::Chat, DropEdge::Trailing))
        );
        assert_eq!(
            locate(regions, Point::new(700.0, 10.0)),
            Some((PaneKind::Model3D, DropEdge::Top))
        );
        assert_eq!(
            locate(regions, Point::new(700.0, 390.0)),
            Some((PaneKind::Model3D, DropEdge::Bottom))
        );
        // the split gap and the space below the 3D pane hit nothing
        assert_eq!(locate(regions, Point::new(404.0, 100.0)), None);
        assert_eq!(locate(regions, Point::new(700.0, 600.0)), None);
    }

    /// dragPane/dropPane: over itself → no target, no drop; over another
    /// pane → the move lands on that pane's edge.
    #[test]
    fn drag_over_another_pane_drops_on_its_edge() {
        let layout = LayoutNode::chat_leading();
        let own = drag_to(PaneKind::Chat, Some((PaneKind::Chat, DropEdge::Bottom)));
        assert_eq!(own.target, None);
        assert_eq!(drop(&layout, &own), None);
        assert_eq!(drop(&layout, &drag_to(PaneKind::Chat, None)), None);

        let over = drag_to(
            PaneKind::Chat,
            Some((PaneKind::Schematic, DropEdge::Trailing)),
        );
        assert_eq!(over.target, Some(PaneKind::Schematic));
        let moved = drop(&layout, &over).expect("a targeted drag drops");
        assert_eq!(
            moved,
            moving(
                &layout,
                PaneKind::Chat,
                DropEdge::Trailing,
                PaneKind::Schematic
            )
        );
        assert_ne!(moved, layout);
    }
}
