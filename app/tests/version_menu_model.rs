//! version menu model tests (M5 T7): the pure entry projection the toolbar
//! menu renders — prompt summaries, newest-first order, the current
//! checkmark, and the subtitle's relative + abbreviated date formatting.

use app::version_menu::{abbreviated, menu_entries, relative};
use engine::project::versions::ProjectVersion;
use time::OffsetDateTime;

#[test]
fn prompt_summary_takes_first_line_capped_at_sixty() {
    assert_eq!(
        app::version_menu::prompt_summary("line one\nline two"),
        "line one"
    );
    let long = "x".repeat(61);
    assert_eq!(
        app::version_menu::prompt_summary(&long),
        format!("{}…", "x".repeat(60))
    );
    assert_eq!(app::version_menu::prompt_summary(""), "");
}

#[test]
fn entries_are_newest_first_with_current_marked() {
    let vs = (1..=3)
        .map(|n| ProjectVersion {
            number: n,
            created_at: OffsetDateTime::now_utc(),
            prompt: format!("p{n}"),
            session_id: None,
        })
        .collect::<Vec<_>>();
    let entries = menu_entries(&vs, Some(2));
    assert_eq!(
        entries.iter().map(|e| e.number).collect::<Vec<_>>(),
        [3, 2, 1]
    );
    assert!(entries[1].is_current && !entries[0].is_current);
}

/// The label line folds the summary (VersionMenu.swift:17/19).
#[test]
fn entries_carry_the_summarized_prompt() {
    let vs = vec![ProjectVersion {
        number: 7,
        created_at: OffsetDateTime::now_utc(),
        prompt: "a 20 mm cube\nwith more detail".into(),
        session_id: None,
    }];
    let entries = menu_entries(&vs, None);
    assert_eq!(entries[0].prompt_summary, "a 20 mm cube");
    assert!(!entries[0].is_current, "no current means none marked");
}

/// The subtitle's relative half (SwiftUI `.relative(presentation: .named)`):
/// "just now", "N minutes ago", "yesterday", "N days ago".
#[test]
fn relative_names_the_distance() {
    let now = OffsetDateTime::now_utc();
    assert_eq!(relative(now, now), "just now");
    assert_eq!(relative(now - time::Duration::seconds(30), now), "just now");
    assert_eq!(
        relative(now - time::Duration::minutes(1), now),
        "1 minute ago"
    );
    assert_eq!(
        relative(now - time::Duration::minutes(5), now),
        "5 minutes ago"
    );
    assert_eq!(relative(now - time::Duration::hours(1), now), "1 hour ago");
    assert_eq!(relative(now - time::Duration::hours(3), now), "3 hours ago");
    assert_eq!(relative(now - time::Duration::days(1), now), "yesterday");
    assert_eq!(relative(now - time::Duration::days(4), now), "4 days ago");
}

/// The subtitle's date half (`.formatted(date: .abbreviated, time:
/// .shortened)`): "Oct 13, 2026 at 3:24 PM", 12-hour clock, UTC (no
/// local-offset lookup in the port).
#[test]
fn abbreviated_dates_use_month_names_and_twelve_hour_clock() {
    let at = OffsetDateTime::parse(
        "2026-10-13T15:24:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert_eq!(abbreviated(at), "Oct 13, 2026 at 3:24 PM");
    let midnight = OffsetDateTime::parse(
        "2026-01-05T00:07:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert_eq!(abbreviated(midnight), "Jan 5, 2026 at 12:07 AM");
}
