//! Pane-grid ↔ layout-tree conversion, resize clamping, and the layout
//! persistence policy. Everything except the [`pane_grid::State`] conversion
//! is plain functions over engine types, so the behavior is testable without a
//! window; Task 7's window assembly is a thin caller of this module.

use cosmic::widget::pane_grid::{self, Configuration, Node, Split, State};
use engine::project::layout::{self, Axis, LayoutNode, PaneKind};
use engine::project::store::{
    load_metadata, save_metadata, ProjectMetadata, ProjectStoreError,
};
use engine::settings::{Settings, SettingsError};
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

/// Clamp a drag ratio so both sides of `split` keep their upstream minimum
/// thickness (PaneSplitViewController.swift:71-76). `width`/`height` are the
/// PaneGrid's pixel size and `spacing` its divider thickness. A region too
/// small to satisfy both minimums returns the split's current ratio unchanged,
/// refusing the drag. iced's PaneGrid enforces only a single global
///  and  stores the raw event ratio, so this clamp
/// is the only enforcement of the per-split minimums: a resize handler must
/// pass every drag ratio through this before calling .
pub fn clamp_resize(
    state: &State<PaneKind>,
    split: &Split,
    proposed: f32,
    width: f32,
    height: f32,
    spacing: f32,
) -> f32 {
    let Some((found, region_w, region_h)) = find_split(state.layout(), split, width, height, spacing)
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
    settings_path: &Path,
    node: &LayoutNode,
) -> Result<(), PersistLayoutError> {
    let mut metadata = load_metadata(project_root)?;
    metadata.layout = Some(node.clone());
    save_metadata(project_root, &metadata)?;
    let mut settings = Settings::load(settings_path)?;
    settings.default_layout = Some(serde_json::to_value(node)?);
    settings.save(settings_path)?;
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
        LayoutNode::Split { axis, first, second, fraction } => Configuration::Split {
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
        Node::Split { axis, ratio, a, b, .. } => LayoutNode::Split {
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
        Node::Split { id, axis, ratio, a, b } => {
            if id == target {
                return Some((
                    SplitRef { axis: *axis, ratio: *ratio, a, b },
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
            LayoutNode::Split { axis, first, second, .. } => LayoutNode::Split {
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

        assert_eq!(metadata.layout.as_ref(), Some(&resized), "model sees the drag");
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
        assert_eq!(load_metadata(&root).unwrap().layout.as_ref(), Some(&resized));
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
            persist_layout(&root, &settings_path, &node).unwrap();

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
}
