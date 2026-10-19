// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsStr;
use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use kernkraft_thumb::{
    file_uri, generate, lookup, thumb_path, Lookup, ThumbError, ThumbSize, FAIL_DIR,
};

const PNG: &str = "image/png";

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn write_png(path: &Path, w: u32, h: u32) {
    image::RgbaImage::from_pixel(w, h, image::Rgba([10, 200, 30, 255]))
        .save_with_format(path, image::ImageFormat::Png)
        .unwrap();
}

fn set_mtime(path: &Path, to: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(to)
        .unwrap();
}

fn text_chunks(path: &Path) -> Vec<(String, String)> {
    let decoder = png::Decoder::new(std::io::BufReader::new(File::open(path).unwrap()));
    let reader = decoder.read_info().unwrap();
    reader
        .info()
        .uncompressed_latin1_text
        .iter()
        .map(|c| (c.keyword.clone(), c.text.clone()))
        .collect()
}

/// A source image and an empty cache home, both inside one temp dir.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let src = dir.path().join("pic.png");
    write_png(&src, 20, 10);
    (dir, cache, src)
}

#[test]
fn uri_and_hash_match_standard_example() {
    // Thumbnail Managing Standard 0.9.0, "Thumbnail Saving": the worked
    // example gives this URI and this MD5.
    let uri = file_uri(Path::new("/home/jens/photos/me.png"));
    assert_eq!(uri, "file:///home/jens/photos/me.png");
    let cache = Path::new("/c");
    assert_eq!(
        thumb_path(cache, &uri, ThumbSize::Normal),
        Path::new("/c/thumbnails/normal/c6ee772d9e49320e97ec29a7eb5b1697.png")
    );
    assert_eq!(
        thumb_path(cache, &uri, ThumbSize::XxLarge),
        Path::new("/c/thumbnails/xx-large/c6ee772d9e49320e97ec29a7eb5b1697.png")
    );
}

#[test]
fn size_classes_follow_the_standard() {
    let all = [
        (ThumbSize::Normal, 128, "normal"),
        (ThumbSize::Large, 256, "large"),
        (ThumbSize::XLarge, 512, "x-large"),
        (ThumbSize::XxLarge, 1024, "xx-large"),
    ];
    for (size, pixels, name) in all {
        assert_eq!(size.pixels(), pixels);
        assert_eq!(size.dir_name(), name);
    }
}

#[test]
fn uri_encodes_space_and_non_utf8() {
    // GLib's g_filename_to_uri escapes space, `%`, `#`, `?`, brackets and
    // quotes, and every byte outside ASCII.
    assert_eq!(
        file_uri(Path::new("/a b/c#d%e?f[g]h.png")),
        "file:///a%20b/c%23d%25e%3Ff%5Bg%5Dh.png"
    );
    assert_eq!(
        file_uri(Path::new("/\\\"<>^`{|}")),
        "file:///%5C%22%3C%3E%5E%60%7B%7C%7D"
    );
    // Unreserved characters stay as written.
    assert_eq!(file_uri(Path::new("/A-z_0.~/x")), "file:///A-z_0.~/x");
    // U+00E9 is the UTF-8 bytes C3 A9; 0xFF 0xFE are not UTF-8 at all.
    assert_eq!(file_uri(Path::new("/\u{e9}")), "file:///%C3%A9");
    let raw = Path::new(OsStr::from_bytes(b"/x/\xff\xfe.png"));
    assert_eq!(file_uri(raw), "file:///x/%FF%FE.png");
    // Control bytes.
    let ctl = Path::new(OsStr::from_bytes(b"/x\x01\x7f\n"));
    assert_eq!(file_uri(ctl), "file:///x%01%7F%0A");
}

#[test]
fn uri_keeps_glib_sub_delims() {
    // g_filename_to_uri leaves `!$&'()*+,;=:@` as written, so a file named
    // with them shares its cache entry with GTK and GNOME applications.
    assert_eq!(
        file_uri(Path::new("/a+b (1)&c=d;e,f@g:h!i*j'k")),
        "file:///a+b%20(1)&c=d;e,f@g:h!i*j'k"
    );
    assert_eq!(file_uri(Path::new("/$x")), "file:///$x");
}

#[test]
fn fresh_after_generate() {
    let (_dir, cache, src) = fixture();
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Normal).unwrap(),
        Lookup::Missing
    );
    let made = generate(&cache, &src, PNG, ThumbSize::Normal).unwrap();
    assert_eq!(made, thumb_path(&cache, &file_uri(&src), ThumbSize::Normal));
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Normal).unwrap(),
        Lookup::Fresh(made.clone())
    );
    // Another size class is a separate entry.
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Large).unwrap(),
        Lookup::Missing
    );

    let meta = fs::metadata(&src).unwrap();
    let chunks = text_chunks(&made);
    let get = |key: &str| {
        chunks
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("Thumb::URI"), Some(file_uri(&src).as_str()));
    assert_eq!(get("Thumb::MTime"), Some(meta.mtime().to_string().as_str()));
    assert_eq!(get("Thumb::Size"), Some(meta.len().to_string().as_str()));
    assert_eq!(get("Software"), Some("kernkraft"));
}

#[test]
fn stale_after_mtime_change() {
    let (_dir, cache, src) = fixture();
    generate(&cache, &src, PNG, ThumbSize::Normal).unwrap();
    // A different whole second.
    let later = fs::metadata(&src).unwrap().modified().unwrap() + Duration::from_secs(5);
    set_mtime(&src, later);
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Normal).unwrap(),
        Lookup::Missing
    );
}

fn foreign_png(path: &Path, texts: &[(&str, &str)]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut encoder = png::Encoder::new(File::create(path).unwrap(), 2, 2);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    for (k, v) in texts {
        encoder
            .add_text_chunk((*k).to_string(), (*v).to_string())
            .unwrap();
    }
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[255; 16]).unwrap();
    writer.finish().unwrap();
}

#[test]
fn foreign_thumbnail_reused() {
    let (_dir, cache, src) = fixture();
    let uri = file_uri(&src);
    let mtime = fs::metadata(&src).unwrap().mtime().to_string();
    let target = thumb_path(&cache, &uri, ThumbSize::Large);

    // Written the way another file manager would: only the two required keys.
    foreign_png(&target, &[("Thumb::URI", &uri), ("Thumb::MTime", &mtime)]);
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Large).unwrap(),
        Lookup::Fresh(target.clone())
    );

    // The wrong URI or the wrong time is not ours to reuse.
    foreign_png(
        &target,
        &[("Thumb::URI", "file:///other"), ("Thumb::MTime", &mtime)],
    );
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Large).unwrap(),
        Lookup::Missing
    );
    foreign_png(&target, &[("Thumb::URI", &uri), ("Thumb::MTime", "1")]);
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Large).unwrap(),
        Lookup::Missing
    );
    foreign_png(&target, &[]);
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Large).unwrap(),
        Lookup::Missing
    );

    // A cache file that is not a PNG is missing, not an error.
    fs::write(&target, b"not a png").unwrap();
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Large).unwrap(),
        Lookup::Missing
    );
}

#[test]
fn lookup_of_a_missing_source_is_an_error() {
    let (dir, cache, _src) = fixture();
    let err = lookup(&cache, &dir.path().join("gone.png"), ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Io { .. }));
}

#[test]
fn fails_recorded_and_reported() {
    let (dir, cache, _src) = fixture();
    let bad = dir.path().join("bad.png");
    fs::write(&bad, b"\x89PNG\r\n\x1a\nthis is not a png").unwrap();

    let err = generate(&cache, &bad, PNG, ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Decode { .. }), "{err:?}");
    assert_eq!(
        lookup(&cache, &bad, ThumbSize::Normal).unwrap(),
        Lookup::Failed
    );
    // The failure applies to every size class.
    assert_eq!(
        lookup(&cache, &bad, ThumbSize::XLarge).unwrap(),
        Lookup::Failed
    );

    let entry = cache.join("thumbnails/fail").join(FAIL_DIR).join(
        thumb_path(&cache, &file_uri(&bad), ThumbSize::Normal)
            .file_name()
            .unwrap(),
    );
    assert!(entry.is_file());
    assert!(FAIL_DIR.starts_with("kern-"));

    // A new version of the file gets another try.
    set_mtime(
        &bad,
        fs::metadata(&bad).unwrap().modified().unwrap() + Duration::from_secs(5),
    );
    assert_eq!(
        lookup(&cache, &bad, ThumbSize::Normal).unwrap(),
        Lookup::Missing
    );
}

#[test]
fn unsupported_mime_is_refused_without_an_entry() {
    let (_dir, cache, src) = fixture();
    let err = generate(&cache, &src, "text/plain", ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Unsupported { .. }));
    assert!(!cache.join("thumbnails").exists());
}

#[test]
fn perms_0700_0600() {
    let (dir, cache, src) = fixture();
    // umask cannot widen a mode, but a permissive process umask must still
    // leave 0700 and 0600.
    let made = generate(&cache, &src, PNG, ThumbSize::Normal).unwrap();
    assert_eq!(mode(&cache.join("thumbnails")), 0o700);
    assert_eq!(mode(&cache.join("thumbnails/normal")), 0o700);
    assert_eq!(mode(&made), 0o600);

    // A thumbnail that already exists with a wider mode ends up 0600.
    fs::set_permissions(&made, fs::Permissions::from_mode(0o644)).unwrap();
    generate(&cache, &src, PNG, ThumbSize::Normal).unwrap();
    assert_eq!(mode(&made), 0o600);

    let bad = dir.path().join("bad.png");
    fs::write(&bad, b"nope").unwrap();
    generate(&cache, &bad, PNG, ThumbSize::Normal).unwrap_err();
    let fail_dir = cache.join("thumbnails/fail").join(FAIL_DIR);
    assert_eq!(mode(&cache.join("thumbnails/fail")), 0o700);
    assert_eq!(mode(&fail_dir), 0o700);
    for entry in fs::read_dir(&fail_dir).unwrap() {
        assert_eq!(mode(&entry.unwrap().path()), 0o600);
    }
}

#[test]
fn foreign_fail_dir_ignored() {
    // Only this application's failure directory counts; another
    // application giving up on a file does not stop us trying.
    let (_dir, cache, src) = fixture();
    let uri = file_uri(&src);
    let mtime = fs::metadata(&src).unwrap().mtime().to_string();
    let name = thumb_path(&cache, &uri, ThumbSize::Normal);
    let entry = cache
        .join("thumbnails/fail/other-app-1")
        .join(name.file_name().unwrap());
    foreign_png(&entry, &[("Thumb::URI", &uri), ("Thumb::MTime", &mtime)]);
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Normal).unwrap(),
        Lookup::Missing
    );
}

#[test]
fn truncated_cache_png_is_missing() {
    let (_dir, cache, src) = fixture();
    let uri = file_uri(&src);
    let mtime = fs::metadata(&src).unwrap().mtime().to_string();
    let target = thumb_path(&cache, &uri, ThumbSize::Normal);
    foreign_png(&target, &[("Thumb::URI", &uri), ("Thumb::MTime", &mtime)]);
    let full = fs::read(&target).unwrap();
    // Cuts inside the signature, the IHDR chunk (ends at byte 33) and the
    // text chunks, all before the image data where the attributes end.
    let idat = full.windows(4).position(|w| w == b"IDAT").unwrap();
    for len in [0, 4, 8, 20, 33, 40, 60, idat - 8] {
        fs::write(&target, &full[..len]).unwrap();
        assert_eq!(
            lookup(&cache, &src, ThumbSize::Normal).unwrap(),
            Lookup::Missing,
            "cut at {len}"
        );
    }
}
