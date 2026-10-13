use engine::agent::stream_event::{ContentBlock, StreamEvent, parse_line};
use std::path::PathBuf;

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/fake-agent")
        .join(p)
}

#[test]
fn corpus_02_decodes_line_by_line() {
    let lines = std::fs::read_to_string(fixture("02-minimal-session.jsonl")).unwrap();
    let mut n = 0;
    for line in lines.lines().filter(|l| !l.trim().is_empty()) {
        let ev = parse_line(line).unwrap_or_else(|| panic!("line failed: {line}"));
        n += 1;
        let _ = ev; // every line decodes to SOME variant
    }
    assert_eq!(n, 19);
    // terminal line is a result with is_error false
    let last = lines
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap();
    match parse_line(last).unwrap() {
        StreamEvent::Result(r) => {
            assert!(!r.is_error);
            assert_eq!(r.result, "pong");
        }
        other => panic!("last line is a result, got {other:?}"),
    }
}

#[test]
fn corpus_03_tooluse_shapes() {
    let raw = std::fs::read_to_string(fixture("03-tool-use.jsonl")).unwrap();
    let lines: Vec<_> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut tool_results = 0;
    let mut assistant_tool_uses = 0;
    for line in &lines {
        match parse_line(line).unwrap() {
            StreamEvent::Assistant(a) => {
                if matches!(a.block, ContentBlock::ToolUse { .. }) {
                    assistant_tool_uses += 1;
                }
            }
            StreamEvent::UserToolResult(_) => tool_results += 1,
            _ => {}
        }
    }
    assert_eq!(assistant_tool_uses, 2, "Write + Read");
    assert_eq!(tool_results, 2);
    // the Write tool_result interleaved MID-STREAM decodes with String content, is_error None
}

#[test]
fn corpus_06_error_result_is_detected_by_is_error() {
    let lines = std::fs::read_to_string(fixture("06-error-minimal.jsonl")).unwrap();
    let last = lines
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap();
    match parse_line(last).unwrap() {
        StreamEvent::Result(r) => {
            assert_eq!(
                r.subtype, "success",
                "probe: subtype stays success on errors"
            );
            assert!(r.is_error);
            assert_eq!(r.terminal_reason.as_deref(), Some("api_error"));
            assert_eq!(r.api_error_status, Some(404));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_types_and_subtypes_never_error() {
    for line in std::fs::read_to_string(fixture("scenarios/unknown-events.jsonl"))
        .unwrap()
        .lines()
    {
        if line.trim().is_empty() {
            continue;
        }
        let ev = parse_line(line).unwrap_or_else(|| panic!("rejected: {line}"));
        let _ = ev;
    }
    assert!(
        matches!(parse_line(r#"{"type":"future_event","x":1}"#).unwrap(), StreamEvent::Unknown(t) if t == "future_event")
    );
    assert!(matches!(
        parse_line(r#"{"type":"system","subtype":"brand_new","session_id":"s"}"#).unwrap(),
        StreamEvent::System(s) if matches!(s.kind, engine::agent::stream_event::SystemKind::Other(ref k) if k == "brand_new")
    ));
}

#[test]
fn tool_result_is_error_and_array_forms() {
    let lines = std::fs::read_to_string(fixture("scenarios/tool-failure.jsonl")).unwrap();
    assert!(lines.contains(r#""is_error":true"#));
    let arr = std::fs::read_to_string(fixture("scenarios/array-tool-result.jsonl")).unwrap();
    assert!(arr.contains(r#""content":[{"#));
    // both decode without error via full-file parse in later session tests; here assert the
    // single tool_result lines decode to the right variants
    for file in [
        "scenarios/tool-failure.jsonl",
        "scenarios/array-tool-result.jsonl",
    ] {
        for line in std::fs::read_to_string(fixture(file)).unwrap().lines() {
            if line.contains("tool_result") {
                assert!(matches!(
                    parse_line(line).unwrap(),
                    StreamEvent::UserToolResult(_)
                ));
            }
        }
    }
}

#[test]
fn synthetic_error_assistant_fields_tolerated() {
    // corpus 06's assistant has error/request_id/is_api_error_message/timestamp at top level
    let lines = std::fs::read_to_string(fixture("06-error-minimal.jsonl")).unwrap();
    let has_synth = lines.lines().any(|l| l.contains("is_api_error_message"));
    assert!(
        has_synth,
        "corpus must contain the synthetic error assistant"
    );
    for line in lines.lines().filter(|l| l.contains("is_api_error_message")) {
        assert!(matches!(
            parse_line(line).unwrap(),
            StreamEvent::Assistant(_)
        ));
    }
}
