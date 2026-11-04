//! Attachment surfaces beyond the picker — the port of upstream
//! Chat/Attachments.swift:4-33 (:4-7 `isImage` lives engine-side in
//! `engine::agent::attachments`): what a clipboard read or a file drop
//! delivers (file URLs first, PNG data second, :9-19), the temp file a
//! pasted image lands in (:21-32), and the read-side rule that resolves a
//! stored attachment string to the path a thumbnail opens. The MIME
//! plumbing implements iced's [`AllowedMimeTypes`] so ONE type serves both
//! content surfaces the pinned rev exposes: `clipboard::read_data` (paste,
//! real on Wayland — the pin's window_clipboard answers arbitrary mimes
//! over data-control; X11 and macOS backends answer nothing) and
//! `widget::dnd_destination` (drag-drop, Wayland-native).

use std::borrow::Cow;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use cosmic::iced::clipboard::mime::AllowedMimeTypes;
use time::OffsetDateTime;
use time::macros::format_description;

/// What a clipboard read or a drop delivered — upstream `urls(from:)`'s
/// two branches (Attachments.swift:9-19): file URLs first, then image data.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipboardContent {
    /// `text/uri-list` entries that name local files (a file manager's
    /// copy or drag); non-`file:` schemes never survive the parse, like
    /// upstream's `urlReadingFileURLsOnly`.
    Files(Vec<PathBuf>),
    /// `image/png` bytes (a screenshot tool's copy, a browser's drag) —
    /// they become a file attachment via [`saved_image`].
    Image(Vec<u8>),
}

const URI_LIST: &str = "text/uri-list";
const PNG: &str = "image/png";

/// The paste/drop read failed to interpret (no file URLs, no PNG, or a
/// platform whose clipboard backend answers nothing — X11 and macOS return
/// no data through the pin's `read_data`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uninterpreted;

/// The mimes both content surfaces ask for, in upstream's preference
/// order: file URLs before image data (Attachments.swift:10-18).
impl AllowedMimeTypes for ClipboardContent {
    fn allowed() -> Cow<'static, [String]> {
        Cow::Owned(vec![URI_LIST.to_string(), PNG.to_string()])
    }
}

impl TryFrom<(Vec<u8>, String)> for ClipboardContent {
    type Error = Uninterpreted;

    fn try_from((data, mime): (Vec<u8>, String)) -> Result<Self, Self::Error> {
        match mime.as_str() {
            // an empty read interprets as nothing — upstream's
            // `guard !urls.isEmpty else { return event }` (ChatInputView
            // .swift:68-69) sends the press back to the text paste
            URI_LIST => {
                let files = parse_uri_list(&data);
                (!files.is_empty())
                    .then_some(Self::Files(files))
                    .ok_or(Uninterpreted)
            }
            PNG if !data.is_empty() => Ok(Self::Image(data)),
            _ => Err(Uninterpreted),
        }
    }
}

/// The `file:` entries of a `text/uri-list` payload as paths. Lines are
/// `\r\n`-separated per the spec (lenient about bare `\n`); `#` starts a
/// comment; percent-escapes decode; every other scheme is dropped —
/// upstream's pasteboard read asks for FILE URLs only
/// (Attachments.swift:10-12).
pub fn parse_uri_list(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("file://"))
        .filter(|rest| !rest.is_empty())
        .map(percent_decode)
        .map(PathBuf::from)
        .collect()
}

/// Decode `%XX` escapes; a stray `%` or short escape passes through
/// verbatim rather than failing the whole read.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let digit = |b: u8| {
        let v = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => 0xff,
        };
        (v != 0xff).then_some(v)
    };
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (digit(bytes[at + 1]), digit(bytes[at + 2])) {
                out.push(hi << 4 | lo);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The pasted-image temp file upstream's `savedImage` writes
/// (Attachments.swift:21-32): `<tmp>/CAD Studio Attachments/Pasted image
/// yyyy-MM-dd at HH.mm.ss.png`. Upstream formats LOCAL time; the port pins
/// UTC like the engine's log names (a process-local offset is unavailable
/// once threads exist — see `LOG_TIMESTAMP` in engine's agent/session.rs); the name only
/// needs uniqueness and upstream's shape.
pub fn saved_image(png: &[u8]) -> Option<PathBuf> {
    // Upstream's NSImage(data:) refuses bytes that are not an image at paste;
    // the clipboard offered `image/png`, so anything else is not staged.
    if !matches!(image::guess_format(png), Ok(image::ImageFormat::Png)) {
        return None;
    }
    let folder = std::env::temp_dir().join("CAD Studio Attachments");
    std::fs::create_dir_all(&folder).ok()?;
    const STAMP: &[time::format_description::FormatItem] =
        format_description!("[year]-[month]-[day] at [hour].[minute].[second]");
    let stamp = OffsetDateTime::now_utc().format(&STAMP).ok()?;
    let path = folder.join(format!("Pasted image {stamp}.png"));
    std::fs::File::create(&path).ok()?.write_all(png).ok()?;
    Some(path)
}

/// Resolve a stored attachment string to the path a thumbnail opens
/// (MessageView.swift:14-16 resolves each stored string against
/// projectRoot): a RELATIVE name — a current row's `references/<stamp>-<n>-
/// <name>` — joins the project root; an ABSOLUTE path stands as itself,
/// which is exactly what a legacy row carries (the original request
/// paths). The draft strip passes its staged originals through the same
/// rule and they resolve as themselves.
pub fn resolve_attachment(path: &str, root: &Path) -> PathBuf {
    let stored = Path::new(path);
    if stored.is_absolute() {
        stored.to_path_buf()
    } else {
        root.join(stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        let mut png = Vec::new();
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    #[test]
    fn pasted_bytes_that_do_not_decode_are_refused() {
        assert_eq!(saved_image(b"not an image"), None);
        let png = tiny_png();
        let saved = saved_image(&png).expect("valid PNG bytes save");
        assert_eq!(std::fs::read(&saved).unwrap(), png);
        let _ = std::fs::remove_file(saved);
    }

    /// uri-list parsing: file URLs decode (percent-escapes included),
    /// comments and other schemes drop, CRLF and LF both split
    /// (Attachments.swift:10-12's file-URLs-only read).
    #[test]
    fn uri_list_keeps_decoded_file_urls_only() {
        let raw = b"file:///home/user/sketch.png\r\n\
                    # a comment line\r\n\
                    file:///tmp/My%20Folder/bracket%2Estep\r\n\
                    https://example.com/not-a-file\r\n";
        assert_eq!(
            parse_uri_list(raw),
            vec![
                PathBuf::from("/home/user/sketch.png"),
                PathBuf::from("/tmp/My Folder/bracket.step"),
            ]
        );
        // empty and non-utf8 payloads read as no files
        assert!(parse_uri_list(b"").is_empty());
        assert!(
            parse_uri_list(b"\xff\xfe").is_empty(),
            "lossy, still no urls"
        );
    }

    /// A stray `%` never fails the decode — it passes through verbatim.
    #[test]
    fn percent_decode_is_lenient() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("a%20b"), "a b");
    }

    /// The MIME surface: the allowed list is upstream's preference order,
    /// and the (bytes, mime) pair lands in the right branch.
    #[test]
    fn clipboard_content_interprets_both_mimes() {
        assert_eq!(
            ClipboardContent::allowed().as_ref(),
            ["text/uri-list".to_string(), "image/png".to_string()]
        );
        assert_eq!(
            ClipboardContent::try_from((b"file:///a.png".to_vec(), "text/uri-list".into())),
            Ok(ClipboardContent::Files(vec![PathBuf::from("/a.png")]))
        );
        let png = vec![1u8, 2, 3];
        assert_eq!(
            ClipboardContent::try_from((png.clone(), "image/png".into())),
            Ok(ClipboardContent::Image(png))
        );
        // the source picked a mime nobody asked for — a read that
        // interprets as nothing
        assert_eq!(
            ClipboardContent::try_from((Vec::new(), "text/plain".into())),
            Err(Uninterpreted)
        );
        // a uri-list with no file URLs, and empty image bytes, interpret
        // as nothing too — the paste falls through to text
        assert_eq!(
            ClipboardContent::try_from((b"# comment\r\n".to_vec(), URI_LIST.into())),
            Err(Uninterpreted)
        );
        assert_eq!(
            ClipboardContent::try_from((Vec::new(), PNG.into())),
            Err(Uninterpreted)
        );
    }

    /// saved_image writes real bytes under upstream's naming shape
    /// (Attachments.swift:26-31) and the name sorts by time.
    #[test]
    fn saved_image_writes_upstreams_temp_shape() {
        let path = saved_image(&tiny_png()).expect("a writable tmp");
        let text = path.to_string_lossy().into_owned();
        assert!(
            path.to_string_lossy().contains("CAD Studio Attachments"),
            "{text}"
        );
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("Pasted image "), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        // the date-time stamp shape: yyyy-MM-dd ' at ' HH.mm.ss
        let stamp = name
            .strip_prefix("Pasted image ")
            .and_then(|s| s.strip_suffix(".png"))
            .unwrap();
        assert!(
            stamp.len() == "yyyy-MM-dd at HH.mm.ss".len()
                && stamp.contains(" at ")
                && stamp.rfind('.').is_some(),
            "{stamp}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), tiny_png());
    }

    /// The read-side law: relative `references/…` names resolve against the
    /// project root (MessageView.swift:14-16); absolute paths — a
    /// legacy row's originals, and the draft strip's staged files — stand
    /// as themselves.
    #[test]
    fn stored_paths_resolve_by_absoluteness() {
        let root = Path::new("/proj");
        assert_eq!(
            resolve_attachment("references/1790000000-1-sketch.png", root),
            PathBuf::from("/proj/references/1790000000-1-sketch.png")
        );
        assert_eq!(
            resolve_attachment("/tmp/CAD Studio Attachments/Pasted image x.png", root),
            PathBuf::from("/tmp/CAD Studio Attachments/Pasted image x.png")
        );
    }
}
