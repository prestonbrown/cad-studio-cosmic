//! parameters.rs tests: ordering, the rewrite regex against a
//! corpus of adversarial lines, integer formatting, and the atomic
//! set_parameter round-trip.

use std::collections::BTreeMap;

use engine::project::parameters::{
    ModelParameter, format_number, ordered_parameters, rewrite_value, set_parameter,
};

#[test]
fn parameters_order_follows_first_occurrence_in_model_py() {
    let script = "z_unused = 1\na = 2\nmiddle()\nb = 3\n";
    let manifest: BTreeMap<String, f64> = [
        ("b".to_string(), 3.0),
        ("a".to_string(), 2.0),
        ("zz".to_string(), 9.0),
    ]
    .into();
    assert_eq!(
        ordered_parameters(&manifest, script),
        [
            ModelParameter {
                name: "a".into(),
                value: 2.0
            },
            ModelParameter {
                name: "b".into(),
                value: 3.0
            },
            ModelParameter {
                name: "zz".into(),
                value: 9.0
            }
        ]
    );
    // both absent: alphabetical
    let manifest2: BTreeMap<String, f64> = [("q".to_string(), 1.0), ("c".to_string(), 2.0)].into();
    assert_eq!(
        ordered_parameters(&manifest2, "nothing here")
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["c", "q"]
    );
    // empty script edge: same as absent
    assert_eq!(ordered_parameters(&manifest2, "").len(), 2);
}

#[test]
fn rewrite_hits_typed_indented_and_spaced_assignments() {
    let cases = [
        ("wall = 2.5", "wall", 3.0, "wall = 3"),
        ("height: float = 10", "height", 12.5, "height: float = 12.5"),
        ("depth : float=4.0", "depth", 5.0, "depth : float=5"),
        ("count=7", "count", 2.0, "count=2"),
        ("  inner = 4", "inner", 6.0, "  inner = 6"),
        ("gap = 1e-3", "gap", 0.5, "gap = 0.5"),
        ("span = 1.2E+2", "span", 130.0, "span = 130"),
        ("off = -2.5", "off", 4.0, "off = 4"),
        ("tiny = .3", "tiny", 2.0, "tiny = 2"),
        ("r = 2.", "r", 3.0, "r = 3"),
    ];
    for (line, name, value, expected) in cases {
        let rewrite = rewrite_value(line, name, value).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(rewrite.script, expected, "line: {line}");
    }
}

#[test]
fn adversarial_lines_never_match_or_mangle() {
    let script = "total == 3\n# width = 5\nwidth_mm = 3\nx = y = 3\nwall: str = \"5\"\n";
    // comparison operator: the name has a plain assignment nowhere
    assert!(matches!(
        rewrite_value(script, "total", 1.0),
        Err(engine::project::parameters::ParameterError::NotFound { .. })
    ));
    // commented assignment
    assert!(matches!(
        rewrite_value(script, "width", 1.0),
        Err(engine::project::parameters::ParameterError::NotFound { .. })
    ));
    // prefix collision: `width` must not match `width_mm` — the exact name
    // still rewrites, surgically, in the full script
    assert_eq!(
        rewrite_value(script, "width_mm", 1.0).unwrap().script,
        "total == 3\n# width = 5\nwidth_mm = 1\nx = y = 3\nwall: str = \"5\"\n"
    );
    // non-numeric rhs
    assert!(matches!(
        rewrite_value(script, "x", 1.0),
        Err(engine::project::parameters::ParameterError::NotFound { .. })
    ));
    // string literal rhs
    assert!(matches!(
        rewrite_value(script, "wall", 1.0),
        Err(engine::project::parameters::ParameterError::NotFound { .. })
    ));
    // name with regex metacharacters must be escaped, still matchable
    let dotted = "part.width = 2\n";
    assert_eq!(
        rewrite_value(dotted, "part.width", 4.0).unwrap().script,
        "part.width = 4\n"
    );
    // a blank line before the target (multiline \s spans it) still rewrites surgically
    assert_eq!(
        rewrite_value("\n\nsize = 3", "size", 4.0).unwrap().script,
        "\n\nsize = 4"
    );
    // first match wins when several assignments share a name
    assert_eq!(
        rewrite_value("k = 1\nk = 2\n", "k", 9.0).unwrap().script,
        "k = 9\nk = 2\n"
    );
    // untouched trailing content is preserved byte-for-byte
    let with_tail = "wall = 2.5  # comment\nnext = 1\n";
    assert_eq!(
        rewrite_value(with_tail, "wall", 3.0).unwrap().script,
        "wall = 3  # comment\nnext = 1\n"
    );
}

#[test]
fn integer_formatting_follows_the_original_literal() {
    assert_eq!(format_number(30.0, "20"), "30");
    assert_eq!(format_number(30.5, "20"), "30.5"); // not rounded
    assert_eq!(format_number(30.0, "20.0"), "30"); // original had a dot: shortest form
    assert_eq!(format_number(-4.0, "-3"), "-4");
    assert_eq!(format_number(0.001234, "9"), "0.001234");
}

#[test]
fn set_parameter_round_trips_atomically() {
    let root = std::env::temp_dir().join(format!("cad-param-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("model.py"), "wall = 2.5\n").unwrap();
    let rewrite = set_parameter(&root, "wall", 4.0).unwrap();
    assert_eq!(rewrite.literal, "4");
    assert_eq!(
        std::fs::read_to_string(root.join("model.py")).unwrap(),
        "wall = 4\n"
    );
    assert!(matches!(
        set_parameter(&root, "nope", 1.0).unwrap_err(),
        engine::project::parameters::ParameterError::NotFound { .. }
    ));
    assert!(!root.join("model.py.tmp").exists()); // temp file renamed away
}

#[test]
fn scan_extracts_every_plain_assignment() {
    let script = "wall = 2\nheight: float = 10\ndepth :float= 0.5\ncount=7\n\
                  gap = 1e-3\noff = -2.5\ntiny = .3\nr = 2.\nwall = 9  # trailing comment\n";
    assert_eq!(
        engine::project::parameters::scan(script),
        BTreeMap::from([
            ("count".to_string(), 7.0),
            ("depth".to_string(), 0.5),
            ("gap".to_string(), 1e-3),
            ("height".to_string(), 10.0),
            ("off".to_string(), -2.5),
            ("r".to_string(), 2.0),
            ("tiny".to_string(), 0.3),
            // last write per name wins, Python rebinding semantics
            ("wall".to_string(), 9.0),
        ])
    );
}

#[test]
fn scan_skips_adversarial_lines() {
    // comparisons, comments, augmented assigns, non-numeric rhs,
    // mid-line assigns after ';', and `x = y = 3` never contribute
    let script =
        "total == 3\n# width = 5\nx += 1\nwidth_mm = 3\ny = z\nx = y = 3\nfoo(); wall = 2\n";
    assert_eq!(
        engine::project::parameters::scan(script),
        BTreeMap::from([("width_mm".to_string(), 3.0)])
    );
    assert!(engine::project::parameters::scan("").is_empty());
}
