use engine::project::layout::{self, Axis, LayoutNode, PaneKind};

/// The exact bytes Swift's synthesized `Codable` emits for the four presets
/// (externally tagged enum, LayoutSplit fields in declaration order,
/// shortest-round-trip doubles).
const CHAT_LEADING_JSON: &str = r#"{"split":{"axis":"row","first":{"pane":"chat"},"second":{"split":{"axis":"column","first":{"pane":"model3D"},"second":{"pane":"schematic"},"fraction":0.6}},"fraction":0.34}}"#;
const CHAT_TRAILING_JSON: &str = r#"{"split":{"axis":"row","first":{"split":{"axis":"column","first":{"pane":"model3D"},"second":{"pane":"schematic"},"fraction":0.6}},"second":{"pane":"chat"},"fraction":0.66}}"#;
const CHAT_BOTTOM_JSON: &str = r#"{"split":{"axis":"column","first":{"split":{"axis":"row","first":{"pane":"model3D"},"second":{"pane":"schematic"},"fraction":0.6}},"second":{"pane":"chat"},"fraction":0.62}}"#;
const THREE_COLUMNS_JSON: &str = r#"{"split":{"axis":"row","first":{"pane":"chat"},"second":{"split":{"axis":"row","first":{"pane":"model3D"},"second":{"pane":"schematic"},"fraction":0.55}},"fraction":0.28}}"#;

#[test]
fn preset_matches_upstream_json() {
    let cases = [
        (LayoutNode::chat_leading(), CHAT_LEADING_JSON),
        (LayoutNode::chat_trailing(), CHAT_TRAILING_JSON),
        (LayoutNode::chat_bottom(), CHAT_BOTTOM_JSON),
        (LayoutNode::three_columns(), THREE_COLUMNS_JSON),
    ];
    for (preset, upstream_json) in cases {
        assert_eq!(serde_json::to_string(&preset).unwrap(), upstream_json);
        let back: LayoutNode = serde_json::from_str(upstream_json).unwrap();
        assert_eq!(back, preset, "upstream bytes decode to the preset");
    }
}

#[test]
fn serde_round_trip() {
    let trees = [
        LayoutNode::chat_leading(),
        LayoutNode::chat_trailing(),
        LayoutNode::chat_bottom(),
        LayoutNode::three_columns(),
        LayoutNode::Pane(PaneKind::Schematic),
        // A three-deep tree past anything the presets cover.
        LayoutNode::Split {
            axis: Axis::Column,
            first: Box::new(LayoutNode::Split {
                axis: Axis::Row,
                first: Box::new(LayoutNode::Pane(PaneKind::Chat)),
                second: Box::new(LayoutNode::Pane(PaneKind::Model3D)),
                fraction: 0.25,
            }),
            second: Box::new(LayoutNode::Split {
                axis: Axis::Row,
                first: Box::new(LayoutNode::Pane(PaneKind::Model3D)),
                second: Box::new(LayoutNode::Pane(PaneKind::Schematic)),
                fraction: 0.75,
            }),
            fraction: 0.1,
        },
    ];
    for tree in trees {
        let json = serde_json::to_string(&tree).unwrap();
        let back: LayoutNode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tree, "round trip via {json}");
    }
}

#[test]
fn min_thickness_matches_upstream() {
    // PaneSplitViewController.minimumLength(of:axis:): 300 only for a chat
    // pane directly on a row split.
    let chat = LayoutNode::Pane(PaneKind::Chat);
    assert_eq!(layout::min_thickness(&chat, Axis::Row), 300.0);
    assert_eq!(layout::min_thickness(&chat, Axis::Column), 140.0);
    assert_eq!(
        layout::min_thickness(&LayoutNode::Pane(PaneKind::Model3D), Axis::Row),
        240.0
    );
    assert_eq!(
        layout::min_thickness(&LayoutNode::Pane(PaneKind::Schematic), Axis::Column),
        140.0
    );
    let split_holding_chat = LayoutNode::Split {
        axis: Axis::Column,
        first: Box::new(LayoutNode::Pane(PaneKind::Chat)),
        second: Box::new(LayoutNode::Pane(PaneKind::Model3D)),
        fraction: 0.6,
    };
    assert_eq!(
        layout::min_thickness(&split_holding_chat, Axis::Row),
        240.0,
        "only a bare chat pane gets 300"
    );
}
