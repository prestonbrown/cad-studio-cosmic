//! Inline markdown → neutral IR — the port of upstream `MarkdownText`
//! (MessageView.swift:80-111). Two stages, exactly upstream's pipeline:
//!
//! 1. [`segments`] splits on `` ``` `` — odd parts are code segments (the
//!    info-string line is dropped, surrounding newlines trimmed, empties
//!    dropped) that render as code cards;
//! 2. Everything else parses INLINE-ONLY, the way
//!    `AttributedString(markdown:, .inlineOnlyPreservingWhitespace)` does:
//!    no headings, lists, tables, block quotes or breaks are interpreted —
//!    their markers pass through as literal text, and all whitespace is
//!    preserved verbatim.
//!
//! There is no block-level API in pulldown-cmark, so inline-only behavior is
//! reproduced over the block parser's offset iterator: every byte the parser
//! consumes without turning it into a styled span (heading `##`, list dashes,
//! block-quote `>`, emphasis markers, link destinations) is re-emitted from
//! the source as a [`MdStyle::Plain`] span. What survives as styled is
//! exactly what Apple's parser styles.
//!
//! Verified against the real `AttributedString` (Swift 6.1 probe,
//! 2026-10-12): smart punctuation is OFF (apostrophes pass through — ASCII
//! `'` stays ASCII, `’` stays U+2019), strikethrough IS parsed upstream but
//! the IR has no variant for it, so it folds to plain styled text (marker
//! dropped — a documented deviation; add the variant if it ever matters).
//! Reference-style link definitions render as plain text including the URL
//! (upstream styles the URL as a link — same visible characters). A link
//! label's emphasis (`[**b**](u)` is a bold-labeled link upstream) also
//! collapses: the link style wins because its URL is semantic.
//!
//! Pure: no iced/cosmic types, no I/O — the chat view maps [`Segment`]s
//! and [`MdSpan`]s onto iced widgets.

/// How a span of message text is styled. Upstream is
/// `Text.inlinePresentationIntent` (MessageView.swift:108-110): strongly
/// emphasized, emphasized, code, link — plus unstyled runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdStyle {
    Plain,
    Strong,
    Emphasis,
    Code,
    /// Upstream attaches `link` to the run; the label is the span text.
    Link {
        url: String,
    },
}

/// One styled fragment of a message. Adjacent fragments of equal style are
/// merged by [`to_ir`] so a paragraph reads as one plain span unless styling
/// actually changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdSpan {
    pub text: String,
    pub style: MdStyle,
}

/// One block of an assistant message, upstream `segments`
/// (MessageView.swift:98-106): prose parsed inline-only, or a fenced code
/// block that renders as a code card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text(Vec<MdSpan>),
    Code(String),
}

/// Split `md` on `` ``` `` the way upstream does: odd components are code
/// with the info-string line dropped (`drop(while: != "\n").dropFirst()`);
/// every component is trimmed of surrounding newlines, and empty ones
/// disappear.
pub fn segments(md: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    for (index, part) in md.split("```").enumerate() {
        let is_code = index % 2 == 1;
        let content = if is_code {
            match part.find('\n') {
                Some(at) => &part[at + 1..],
                None => "",
            }
        } else {
            part
        };
        let trimmed = content.trim_matches(['\n', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        out.push(if is_code {
            Segment::Code(trimmed.to_string())
        } else {
            Segment::Text(to_ir(trimmed))
        });
    }
    out
}

/// Parse one prose segment into inline-styled spans (see the module docs
/// for the inline-only contract). Fences are not interpreted here —
/// [`segments`] splits them out first.
pub fn to_ir(md: &str) -> Vec<MdSpan> {
    let mut spans = Vec::new();
    parse_inline(md, &mut spans);
    spans
}

/// An open inline element: the style its children carry, where the element
/// starts (the opening marker — gap emission clamps before it) and where it
/// ends (past the closing marker / link destination).
struct Open {
    style: MdStyle,
    start: usize,
    end: usize,
}

/// One even (non-code) segment, parsed inline-only over the block parser's
/// offset iterator. `source` is the segment; all ranges index into it.
fn parse_inline(source: &str, spans: &mut Vec<MdSpan>) {
    // Strikethrough parses so its markers drop (upstream styles the text; the
    // IR folds to plain). Everything else CommonMark-block stays unenabled —
    // GFM tables and footnotes must render literally, and smart punctuation
    // must stay off.
    let mut options = pulldown_cmark::Options::empty();
    options.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    let parser = pulldown_cmark::Parser::new_ext(source, options).into_offset_iter();

    let mut cursor = 0usize;
    let mut stack: Vec<Open> = Vec::new();

    for (event, range) in parser {
        match event {
            pulldown_cmark::Event::Start(tag) => match tag {
                pulldown_cmark::Tag::Strong => stack.push(Open {
                    style: MdStyle::Strong,
                    start: range.start,
                    end: range.end,
                }),
                pulldown_cmark::Tag::Emphasis => stack.push(Open {
                    style: MdStyle::Emphasis,
                    start: range.start,
                    end: range.end,
                }),
                pulldown_cmark::Tag::Strikethrough => stack.push(Open {
                    style: MdStyle::Plain,
                    start: range.start,
                    end: range.end,
                }),
                pulldown_cmark::Tag::Link {
                    link_type,
                    dest_url,
                    ..
                } => match link_type {
                    // Reference-style links never resolve inline-only: Apple's
                    // parser sees no definitions, so `[label][1]` (and the
                    // `[shortcut]` form) stays literal text (probe
                    // 2026-10-12). The definition itself emits as plain gap
                    // text. The pushed entry is a balanced sentinel for the
                    // matching End; its children are skipped by the
                    // already-consumed rule below.
                    pulldown_cmark::LinkType::Reference
                    | pulldown_cmark::LinkType::Collapsed
                    | pulldown_cmark::LinkType::Shortcut => {
                        push_merged(
                            spans,
                            source[range.start..range.end].to_string(),
                            MdStyle::Plain,
                        );
                        cursor = range.end;
                        stack.push(Open {
                            style: MdStyle::Plain,
                            start: range.start,
                            end: range.end,
                        });
                    }
                    _ => stack.push(Open {
                        style: MdStyle::Link {
                            url: dest_url.into_string(),
                        },
                        start: range.start,
                        end: range.end,
                    }),
                },
                // Blocks (paragraphs, headings, lists, code blocks…) structure
                // the parse but style nothing; their markers re-emerge as
                // plain gap text below.
                _ => {}
            },
            pulldown_cmark::Event::End(tag_end) => {
                match tag_end {
                    pulldown_cmark::TagEnd::Strong
                    | pulldown_cmark::TagEnd::Emphasis
                    | pulldown_cmark::TagEnd::Strikethrough
                    | pulldown_cmark::TagEnd::Link => {
                        if let Some(open) = stack.pop() {
                            // Skip the closing marker (and, for links, the
                            // whole `](destination)` tail).
                            cursor = cursor.max(open.end);
                        }
                    }
                    _ => {}
                }
            }
            pulldown_cmark::Event::Text(text) => {
                if range.start >= cursor {
                    emit_gap(
                        spans,
                        source,
                        cursor,
                        range.start,
                        marker_floor(&stack, cursor),
                    );
                    push_merged(spans, text.into_string(), current_style(&stack));
                    cursor = range.end;
                }
            }
            pulldown_cmark::Event::Code(code) => {
                if range.start >= cursor {
                    emit_gap(
                        spans,
                        source,
                        cursor,
                        range.start,
                        marker_floor(&stack, cursor),
                    );
                    push_merged(spans, code.into_string(), MdStyle::Code);
                    cursor = range.end;
                }
            }
            // Soft/hard breaks, rules, raw HTML: leave the cursor alone so
            // the next gap re-emits the source bytes verbatim
            // (inlineOnlyPreservingWhitespace keeps them all).
            _ => {}
        }
    }
    emit_gap(spans, source, cursor, source.len(), None);
}

/// Where a gap emission must stop: the earliest open element whose start is
/// still ahead of the cursor — the next opening marker region. Markers of
/// stacked openers (`**[link](u)**`) all lie between that floor and the next
/// text, so they suppress together; an element the cursor has already passed
/// (its first child emitted, or an End skip moved past it) contributes no
/// floor, which keeps REAL text between sibling elements
/// (`**[a](u) [b](v)**`) out of the clamp.
fn marker_floor(stack: &[Open], cursor: usize) -> Option<usize> {
    stack
        .iter()
        .map(|o| o.start)
        .filter(|&start| start >= cursor)
        .min()
}

/// The style a text run carries. A link's URL is semantic, so a link
/// anywhere in the open chain wins over emphasis nesting (links cannot
/// nest); otherwise the innermost open style applies. The emphasis a link
/// label carries upstream (`[**b**](u)` is a bold-labeled link) is the
/// documented single-style collapse.
fn current_style(stack: &[Open]) -> MdStyle {
    for open in stack.iter().rev() {
        if matches!(open.style, MdStyle::Link { .. }) {
            return open.style.clone();
        }
    }
    stack
        .last()
        .map(|o| o.style.clone())
        .unwrap_or(MdStyle::Plain)
}

/// Emit `source[cursor..end]` as a plain span, clamped at `floor` — the
/// next unpassed opening marker, so markers (`**`, the `[` of a link) are
/// skipped rather than leaked as literal text.
fn emit_gap(
    spans: &mut Vec<MdSpan>,
    source: &str,
    cursor: usize,
    end: usize,
    floor: Option<usize>,
) {
    let end = floor.map_or(end, |floor| end.min(floor));
    if cursor < end {
        let gap = &source[cursor..end];
        // A gap that is a lone backslash is an escape marker: pulldown
        // starts the escaped character's Text range AFTER the backslash, and
        // Apple decodes the escape (a backslash-asterisk renders as "*",
        // probe 2026-10-12).
        if gap != "\\" {
            push_merged(spans, gap.to_string(), MdStyle::Plain);
        }
    }
}

fn push_merged(spans: &mut Vec<MdSpan>, text: String, style: MdStyle) {
    match spans.last_mut() {
        Some(last) if last.style == style => last.text.push_str(&text),
        _ => spans.push(MdSpan { text, style }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use MdStyle::{Code, Emphasis, Link, Plain, Strong};

    fn spans(md: &str) -> Vec<(MdStyle, String)> {
        to_ir(md).into_iter().map(|s| (s.style, s.text)).collect()
    }

    fn ir(md: &str) -> Vec<MdSpan> {
        to_ir(md)
    }

    /// The four inline styles, with markers consumed and adjacent plain runs
    /// merged (Swift probe: "plain " / inline=2 "bold" / " and " / inline=1
    /// "em" / " and " / inline=4 "code").
    #[test]
    fn strong_emphasis_code_link_spans() {
        assert_eq!(
            spans("plain **bold** and *em* and `code`"),
            vec![
                (Plain, "plain ".into()),
                (Strong, "bold".into()),
                (Plain, " and ".into()),
                (Emphasis, "em".into()),
                (Plain, " and ".into()),
                (Code, "code".into()),
            ]
        );
    }

    /// A link keeps only its label as text and carries the destination; the
    /// `](url)` tail is consumed, not rendered (Swift probe).
    #[test]
    fn link_span_carries_url_not_destination() {
        assert_eq!(
            spans("see [docs](https://example.com/x) ok"),
            vec![
                (Plain, "see ".into()),
                (
                    Link {
                        url: "https://example.com/x".into()
                    },
                    "docs".into()
                ),
                (Plain, " ok".into()),
            ]
        );
        // autolinks label with the URL itself
        assert_eq!(
            spans("<https://auto.example>"),
            vec![(
                Link {
                    url: "https://auto.example".into()
                },
                "https://auto.example".into()
            )]
        );
    }

    /// `##` (and every other block marker) renders literally — inline-only
    /// parsing (Swift probe: "## Heading line" as one unstyled run).
    #[test]
    fn heading_marker_stays_literal() {
        assert_eq!(
            spans("## Heading line"),
            vec![(Plain, "## Heading line".into())]
        );
        assert_eq!(spans("#H no-space"), vec![(Plain, "#H no-space".into())]);
        // mid-text hash is just text
        assert_eq!(
            spans("after text ## H line"),
            vec![(Plain, "after text ## H line".into())]
        );
    }

    /// Lists, ordered lists and block quotes are literal text with markers
    /// and newlines intact (Swift probe: "- one\n- two" verbatim).
    #[test]
    fn list_and_quote_markers_stay_literal() {
        assert_eq!(spans("- one\n- two"), vec![(Plain, "- one\n- two".into())]);
        assert_eq!(spans("* a\n+ b"), vec![(Plain, "* a\n+ b".into())]);
        assert_eq!(
            spans("1. first\n2. second"),
            vec![(Plain, "1. first\n2. second".into())]
        );
        assert_eq!(
            spans("> quoted line"),
            vec![(Plain, "> quoted line".into())]
        );
        // GFM tables are off: pipes stay literal
        assert_eq!(
            spans("a | b\n--- | ---"),
            vec![(Plain, "a | b\n--- | ---".into())]
        );
    }

    /// Smart punctuation is OFF upstream: ASCII apostrophes stay ASCII and
    /// U+2019 passes through unchanged (Swift probe: "isn't" verbatim). The
    /// reducer's streaming prefix-match depends on text never being
    /// rewritten under it.
    #[test]
    fn apostrophes_pass_through_unsmartened() {
        assert_eq!(spans("isn't"), vec![(Plain, "isn't".into())]);
        assert_eq!(spans("l’oggetto"), vec![(Plain, "l’oggetto".into())]);
        assert_eq!(
            spans("\"quoted\" — dash"),
            vec![(Plain, "\"quoted\" — dash".into())]
        );
    }

    fn seg(md: &str) -> Vec<Segment> {
        segments(md)
    }

    fn text(t: &str) -> Segment {
        Segment::Text(vec![MdSpan {
            text: t.into(),
            style: Plain,
        }])
    }

    /// A fence becomes its own code segment between the prose segments: the
    /// info-string line is dropped and every segment is trimmed of the
    /// newlines around it (upstream segments, MessageView.swift:98-106).
    #[test]
    fn fence_splits_into_code_segment() {
        assert_eq!(
            seg("before\n```rust\nfn main() {}\n```\nafter"),
            vec![
                text("before"),
                Segment::Code("fn main() {}".into()),
                text("after"),
            ]
        );
        // fence without an info string
        assert_eq!(
            seg("```\nplain fence\n```"),
            vec![Segment::Code("plain fence".into())]
        );
        // multi-line code keeps its inner newlines and indentation
        assert_eq!(
            seg("```py\nif x:\n    y()\n```"),
            vec![Segment::Code("if x:\n    y()".into())]
        );
        // inline fences with no info-string newline drop entirely (the
        // drop-while rule): the prose either side stays two segments
        assert_eq!(seg("a```x=1```b```y=2```"), vec![text("a"), text("b")]);
        // markdown inside prose segments is still parsed inline
        assert_eq!(
            seg("**done**\n```\nx\n```"),
            vec![
                Segment::Text(vec![MdSpan {
                    text: "done".into(),
                    style: Strong,
                }]),
                Segment::Code("x".into()),
            ]
        );
    }

    /// Upstream's drop(while: != "\n").dropFirst(): a code part with no
    /// newline at all is empty after the info-string drop and disappears;
    /// so does a part that is only newlines, and so does empty prose.
    #[test]
    fn empty_fence_parts_drop() {
        assert_eq!(seg("a```oops"), vec![text("a")]);
        assert_eq!(seg("```\n```"), vec![]);
        assert_eq!(seg(""), vec![]);
        assert_eq!(seg("\n\n```\nx\n```\n\n"), vec![Segment::Code("x".into())]);
    }

    /// Paragraph breaks and hard-break whitespace are preserved verbatim
    /// (Swift probe: "a\n\nb" and "2  \n" both pass through unchanged).
    #[test]
    fn whitespace_is_preserved() {
        assert_eq!(spans("a\n\nb"), vec![(Plain, "a\n\nb".into())]);
        assert_eq!(
            spans("text with 2  \nhard break and\nsoft break"),
            vec![(Plain, "text with 2  \nhard break and\nsoft break".into())]
        );
        // indented code is not a block here: the leading spaces survive
        assert_eq!(
            spans("    indented code"),
            vec![(Plain, "    indented code".into())]
        );
    }

    /// Nested inline markup keeps per-run styles; markers never leak.
    #[test]
    fn nested_emphasis_inside_strong() {
        assert_eq!(
            spans("**a *b* c**"),
            vec![
                (Strong, "a ".into()),
                (Emphasis, "b".into()),
                (Strong, " c".into()),
            ]
        );
        assert_eq!(
            spans("**bold with `code` inside**"),
            vec![
                (Strong, "bold with ".into()),
                (Code, "code".into()),
                (Strong, " inside".into()),
            ]
        );
    }

    /// Stacked openers with NO text between them: a link directly inside
    /// strong must not leak the strong's `**` (the gap walk clamps at the
    /// earliest not-yet-passed opener). Swift: one strong link run "link".
    #[test]
    fn strong_around_link_leaks_no_markers() {
        assert_eq!(
            spans("**[link](u)**"),
            vec![(Link { url: "u".into() }, "link".into())]
        );
    }

    /// The mirror stack — strong inside a link — must not leak the `[` and
    /// must keep the link's URL: a link anywhere in the open chain wins the
    /// single-valued style (the URL is semantic; the emphasis loss is the
    /// documented single-style collapse). Swift: link run, bold label.
    #[test]
    fn strong_inside_link_keeps_link() {
        assert_eq!(
            spans("[**b**](u)"),
            vec![(Link { url: "u".into() }, "b".into())]
        );
        // emphasis inside strong inside link, still no leaks
        assert_eq!(
            spans("*[**b**](u)*"),
            vec![(Link { url: "u".into() }, "b".into())]
        );
    }

    /// Sibling nested elements inside one outer: the text BETWEEN them is
    /// real and survives marker suppression (the clamp only reaches the next
    /// unpassed opener, never across a closed sibling).
    #[test]
    fn adjacent_links_keep_separating_text() {
        assert_eq!(
            spans("**[a](u) [b](v)**"),
            vec![
                (Link { url: "u".into() }, "a".into()),
                (Strong, " ".into()),
                (Link { url: "v".into() }, "b".into()),
            ]
        );
        // real text around a nested chain, spaces intact end to end
        assert_eq!(
            spans("a **b [c](u) d** e"),
            vec![
                (Plain, "a ".into()),
                (Strong, "b ".into()),
                (Link { url: "u".into() }, "c".into()),
                (Strong, " d".into()),
                (Plain, " e".into()),
            ]
        );
    }

    /// Strikethrough parses upstream (inline=32) but the IR has no variant:
    /// the markers drop and the text folds to plain (documented deviation).
    #[test]
    fn strikethrough_folds_to_plain() {
        assert_eq!(
            spans("strike ~~x~~ tail"),
            vec![(Plain, "strike x tail".into())]
        );
    }

    /// Entities and backslash escapes decode to their characters (both
    /// parsers do); the escape sequence itself never renders.
    #[test]
    fn entities_and_escapes_decode() {
        assert_eq!(spans("a &amp; b"), vec![(Plain, "a & b".into())]);
        assert_eq!(spans("\\*not em\\*"), vec![(Plain, "*not em*".into())]);
    }

    /// Reference-style links stay literal — Apple's inline-only parser sees
    /// no definitions, so `[label][1]`, `[shortcut]` and the definition line
    /// all render verbatim (probe 2026-10-12). Apple additionally styles the
    /// bare definition URL as a link; this port keeps it plain (documented,
    /// styling-only deviation).
    #[test]
    fn reference_links_stay_literal() {
        assert_eq!(
            ir("[ref-style][1]\n\n[1]: https://ref.example"),
            vec![MdSpan {
                text: "[ref-style][1]\n\n[1]: https://ref.example".into(),
                style: Plain,
            }]
        );
        assert_eq!(
            spans("[shortcut] alone"),
            vec![(Plain, "[shortcut] alone".into())]
        );
        // inline links still resolve
        assert_eq!(
            spans("[a](https://x.example)"),
            vec![(
                Link {
                    url: "https://x.example".into()
                },
                "a".into()
            )]
        );
    }
}
