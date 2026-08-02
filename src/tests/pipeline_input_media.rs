//! `-f` media-detection tests (§5.5): a path whose extension is in the closed
//! media table attaches as a `Content::Image`/`Content::Document` base64 part
//! instead of text; everything unmapped is the text path unchanged. The signal
//! is the path the caller typed — never magic-byte sniffing — so the fixtures
//! deliberately hold bytes that contradict their names.

use std::io::{self, Read};
use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use tempfile::TempDir;

use crate::{read_files, Content, DocumentSource, ImageSource};

/// A reader that always fails — proves named-file paths never touch stdin.
struct FailReader;
impl Read for FailReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("boom"))
    }
}

/// Write `bytes` under `name` in `dir` and return the path — extension-named
/// fixtures, unlike `NamedTempFile`'s random suffix (which is the unmapped case).
fn named(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// The one part read from `path`, stdin untouched.
fn part(path: PathBuf) -> Content {
    let mut parts = read_files(&[path], &mut FailReader).unwrap();
    assert_eq!(parts.len(), 1);
    parts.remove(0)
}

/// Every image row of the table (§5.5), the alias `.jpg`/`.jpeg` pair included,
/// maps to `Image{Base64}` with the table's media type and the file's bytes
/// standard-base64'd — non-UTF-8 bytes, which the text path would refuse (66).
#[test]
fn mapped_image_extensions_attach_as_base64_image_parts() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = [0x89, 0x50, 0x4e, 0x47, 0xff, 0x00];
    for (name, media_type) in [
        ("a.png", "image/png"),
        ("a.jpg", "image/jpeg"),
        ("a.jpeg", "image/jpeg"),
        ("a.gif", "image/gif"),
        ("a.webp", "image/webp"),
    ] {
        assert_eq!(
            part(named(&dir, name, &bytes)),
            Content::Image {
                source: ImageSource::Base64 {
                    media_type: media_type.into(),
                    data: STANDARD.encode(bytes),
                }
            },
            "{name}"
        );
    }
}

/// `.pdf` is the document row: the family derives from the media type
/// (`application/pdf` is not `image/*`), so it lands as `Document{Base64}`.
#[test]
fn pdf_extension_attaches_as_a_base64_document_part() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        part(named(&dir, "report.pdf", b"%PDF-1.7\xff")),
        Content::Document {
            source: DocumentSource::Base64 {
                media_type: "application/pdf".into(),
                data: STANDARD.encode(b"%PDF-1.7\xff"),
            }
        }
    );
}

/// The extension match is case-insensitive: `.PNG` is the same table row.
#[test]
fn extension_match_is_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        part(named(&dir, "shout.PNG", &[0xff])),
        Content::Image { .. }
    ));
}

/// An unmapped extension and a bare name are the text path unchanged — and the
/// caller's name is honored, never sniffed: binary bytes under `.bin`/no-suffix
/// fail UTF-8 exactly as before (→ 66), naming the offending path.
#[test]
fn unmapped_extensions_stay_on_the_text_path() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        part(named(&dir, "notes.rs", b"utf-8 text")),
        Content::Text("utf-8 text".into())
    );
    assert_eq!(
        part(named(&dir, "no_extension", b"also text")),
        Content::Text("also text".into())
    );
    let bin = named(&dir, "blob.bin", &[0xff, 0xfe]);
    let (path, _e) = read_files(std::slice::from_ref(&bin), &mut FailReader).unwrap_err();
    assert_eq!(path, bin);
}

/// A missing media-extension file is the same exit-66 class as a missing text
/// file: `fs::read` folds missing/unreadable into one `io::Error` with the path.
#[test]
fn missing_media_file_returns_the_offending_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.png");
    let (path, _e) = read_files(std::slice::from_ref(&missing), &mut FailReader).unwrap_err();
    assert_eq!(path, missing);
}
