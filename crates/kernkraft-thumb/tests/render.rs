// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use image::{ImageFormat, Rgba, RgbaImage};
use kernkraft_thumb::{can_thumbnail, generate, lookup, Lookup, ThumbError, ThumbSize};

fn dims(path: &Path) -> (u32, u32) {
    image::image_dimensions(path).unwrap()
}

fn pixels(path: &Path) -> RgbaImage {
    image::open(path).unwrap().to_rgba8()
}

fn cache_in(dir: &Path) -> PathBuf {
    let cache = dir.join("cache");
    fs::create_dir_all(&cache).unwrap();
    cache
}

fn solid(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba([200, 30, 90, 255]))
}

#[test]
fn can_thumbnail_lists_the_five_types() {
    for mime in [
        "image/png",
        "image/jpeg",
        "image/webp",
        "image/gif",
        "image/svg+xml",
    ] {
        assert!(can_thumbnail(mime), "{mime}");
    }
    for mime in ["image/tiff", "text/plain", "application/pdf", ""] {
        assert!(!can_thumbnail(mime), "{mime}");
    }
}

#[test]
fn never_upscales() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("small.png");
    solid(40, 20).save(&src).unwrap();
    let made = generate(&cache, &src, "image/png", ThumbSize::Normal).unwrap();
    assert_eq!(dims(&made), (40, 20));
}

#[test]
fn fits_aspect() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("wide.png");
    solid(1000, 500).save(&src).unwrap();
    let made = generate(&cache, &src, "image/png", ThumbSize::Normal).unwrap();
    assert_eq!(dims(&made), (128, 64));
    let made = generate(&cache, &src, "image/png", ThumbSize::Large).unwrap();
    assert_eq!(dims(&made), (256, 128));
}

#[test]
fn every_raster_format_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let rgb = image::DynamicImage::ImageRgba8(solid(300, 100));
    for (ext, format, mime) in [
        ("jpg", ImageFormat::Jpeg, "image/jpeg"),
        ("webp", ImageFormat::WebP, "image/webp"),
        ("gif", ImageFormat::Gif, "image/gif"),
    ] {
        let src = dir.path().join(format!("pic.{ext}"));
        // JPEG has no alpha channel.
        let img = if ext == "jpg" {
            image::DynamicImage::ImageRgb8(rgb.to_rgb8())
        } else {
            rgb.clone()
        };
        img.save_with_format(&src, format).unwrap();
        let made = generate(&cache, &src, mime, ThumbSize::Normal).unwrap();
        let (w, h) = dims(&made);
        assert_eq!(w, 128, "{ext}");
        // 100 * 128 / 300 = 42.67: the height rounds to either neighbour.
        assert!((42..=43).contains(&h), "{ext}: {h}");
    }
}

#[test]
fn gif_uses_the_first_frame() {
    use image::codecs::gif::GifEncoder;
    use image::Frame;
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("anim.gif");
    let red = RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
    let blue = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 255, 255]));
    let mut encoder = GifEncoder::new(File::create(&src).unwrap());
    encoder.encode_frame(Frame::new(red)).unwrap();
    encoder.encode_frame(Frame::new(blue)).unwrap();
    drop(encoder);
    let made = generate(&cache, &src, "image/gif", ThumbSize::Normal).unwrap();
    let px = pixels(&made);
    assert!(px.get_pixel(5, 5).0[0] > 200 && px.get_pixel(5, 5).0[2] < 50);
}

#[test]
fn decompression_bomb_is_a_decode_error() {
    // A header that asks for a 60000 x 60000 image (14.4 GB as RGBA8) and
    // carries no data: the limits refuse it before anything is allocated.
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("bomb.png");
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 60000, 60000);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::One);
        let writer = encoder.write_header().unwrap();
        std::mem::forget(writer);
    }
    fs::write(&src, &bytes).unwrap();
    let err = generate(&cache, &src, "image/png", ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Decode { .. }), "{err:?}");
    assert_eq!(
        lookup(&cache, &src, ThumbSize::Normal).unwrap(),
        Lookup::Failed
    );
}

#[test]
fn truncated_and_empty_files_fail_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let full = dir.path().join("full.png");
    solid(64, 64).save(&full).unwrap();
    let bytes = fs::read(&full).unwrap();
    let cut = dir.path().join("cut.png");
    fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
    let err = generate(&cache, &cut, "image/png", ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Decode { .. }), "{err:?}");
    let empty = dir.path().join("empty.jpg");
    fs::write(&empty, b"").unwrap();
    let err = generate(&cache, &empty, "image/jpeg", ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Decode { .. }), "{err:?}");
}

const SVG_HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="200" height="100" viewBox="0 0 200 100">"#;
const SVG_BODY: &str = r##"<rect width="200" height="100" fill="#336699"/><circle cx="150" cy="50" r="30" fill="red"/>"##;

fn svg(images: &str) -> String {
    format!("{SVG_HEAD}{SVG_BODY}{images}</svg>")
}

fn render_svg(cache: &Path, dir: &Path, name: &str, content: &str) -> RgbaImage {
    let src = dir.join(name);
    fs::write(&src, content).unwrap();
    let made = generate(cache, &src, "image/svg+xml", ThumbSize::Normal).unwrap();
    pixels(&made)
}

#[test]
fn svg_renders_and_fits() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("a.svg");
    fs::write(&src, svg("")).unwrap();
    let made = generate(&cache, &src, "image/svg+xml", ThumbSize::XxLarge).unwrap();
    // A 200 x 100 SVG is smaller than 1024 and is not enlarged.
    assert_eq!(dims(&made), (200, 100));
    let px = pixels(&made);
    // The rectangle colour #336699 at a corner, the red circle at its centre.
    assert_eq!(px.get_pixel(2, 2).0, [0x33, 0x66, 0x99, 255]);
    assert_eq!(px.get_pixel(150, 50).0, [255, 0, 0, 255]);

    let big = svg("").replace(
        r#"width="200" height="100""#,
        r#"width="2000" height="1000""#,
    );
    fs::write(&src, big).unwrap();
    let made = generate(&cache, &src, "image/svg+xml", ThumbSize::Normal).unwrap();
    assert_eq!(dims(&made), (128, 64));
}

#[test]
fn svg_malformed_is_a_decode_error() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("bad.svg");
    fs::write(&src, "<svg><rect").unwrap();
    let err = generate(&cache, &src, "image/svg+xml", ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Decode { .. }), "{err:?}");
}

#[test]
fn svg_external_href_not_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let svg_dir = dir.path().join("svg");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&svg_dir).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let cache = cache_in(dir.path());

    let secret = outside.join("secret.png");
    RgbaImage::from_pixel(50, 50, Rgba([0, 255, 0, 255]))
        .save(&secret)
        .unwrap();

    let with_refs = svg(&format!(
        r#"<image x="0" y="0" width="100" height="100" xlink:href="{}"/><image x="0" y="0" width="100" height="100" xlink:href="http://example.com/x.png"/>"#,
        secret.display()
    ));
    let rendered = render_svg(&cache, &svg_dir, "with.svg", &with_refs);
    let plain = render_svg(&cache, &svg_dir, "without.svg", &svg(""));
    assert_eq!(rendered.dimensions(), plain.dimensions());
    assert!(rendered.as_raw() == plain.as_raw(), "pixels differ");
    // The image sat over the top-left corner; had it loaded, green would be there.
    assert_eq!(rendered.get_pixel(5, 5).0, [0x33, 0x66, 0x99, 255]);
}

#[test]
fn svg_relative_escapes_and_links_refused() {
    let dir = tempfile::tempdir().unwrap();
    let svg_dir = dir.path().join("svg");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&svg_dir).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let cache = cache_in(dir.path());
    RgbaImage::from_pixel(50, 50, Rgba([0, 255, 0, 255]))
        .save(outside.join("secret.png"))
        .unwrap();
    symlink(outside.join("secret.png"), svg_dir.join("link.png")).unwrap();

    let image = |href: &str| {
        svg(&format!(
            r#"<image x="0" y="0" width="100" height="100" xlink:href="{href}"/>"#
        ))
    };
    let plain = render_svg(&cache, &svg_dir, "plain.svg", &svg(""));
    for href in [
        "../outside/secret.png",
        "link.png",
        "file:///etc/hostname",
        "file://localhost/etc/hostname",
        "ftp://example.com/x.png",
    ] {
        let got = render_svg(&cache, &svg_dir, "escape.svg", &image(href));
        assert!(got.as_raw() == plain.as_raw(), "{href} was loaded");
    }
}

#[test]
fn svg_sibling_image_and_data_url_load() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let green = RgbaImage::from_pixel(10, 10, Rgba([0, 255, 0, 255]));
    green.save(dir.path().join("sibling.png")).unwrap();

    let image = |href: &str| {
        svg(&format!(
            r#"<image x="0" y="0" width="100" height="100" xlink:href="{href}"/>"#
        ))
    };
    let sibling = render_svg(&cache, dir.path(), "s.svg", &image("sibling.png"));
    assert_eq!(sibling.get_pixel(5, 5).0, [0, 255, 0, 255]);

    let mut png_bytes = Vec::new();
    green
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png)
        .unwrap();
    let href = format!("data:image/png;base64,{}", base64(&png_bytes));
    let embedded = render_svg(&cache, dir.path(), "d.svg", &image(&href));
    assert_eq!(embedded.get_pixel(5, 5).0, [0, 255, 0, 255]);
}

/// RFC 4648 section 4 with padding.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn assert_decode_quickly(cache: &Path, src: &Path, mime: &str) {
    let started = std::time::Instant::now();
    let err = generate(cache, src, mime, ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Decode { .. }), "{err:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn gzip_svg_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let small = dir.path().join("small.svgz");
    fs::write(&small, gzip(svg("").as_bytes())).unwrap();
    assert_decode_quickly(&cache, &small, "image/svg+xml");

    // 64 MiB of text inflates from a few tens of kilobytes.
    let mut bomb = svg("").into_bytes();
    bomb.truncate(bomb.len() - "</svg>".len());
    bomb.extend(std::iter::repeat_n(b' ', 64 * 1024 * 1024));
    bomb.extend_from_slice(b"</svg>");
    let big = dir.path().join("bomb.svgz");
    fs::write(&big, gzip(&bomb)).unwrap();
    assert_decode_quickly(&cache, &big, "image/svg+xml");

    // The same through a nested data URL is not inflated either.
    let nested = svg(&format!(
        r#"<image width="10" height="10" xlink:href="data:image/svg+xml;base64,{}"/>"#,
        base64(&gzip(&bomb))
    ));
    let src = dir.path().join("nested.svg");
    fs::write(&src, nested).unwrap();
    let made = generate(&cache, &src, "image/svg+xml", ThumbSize::Normal).unwrap();
    let plain = render_svg(&cache, dir.path(), "plain.svg", &svg(""));
    assert!(pixels(&made).as_raw() == plain.as_raw());
}

/// Nesting depth the SVG path accepts (see `MAX_SVG_DEPTH`).
const SVG_DEPTH_CAP: usize = 32;

fn nested_groups(depth: usize) -> String {
    let mut s = SVG_HEAD.to_string();
    for _ in 0..depth {
        s.push_str(r#"<g opacity="0.9">"#);
    }
    s.push_str(r#"<rect width="50" height="50" fill="red"/>"#);
    for _ in 0..depth {
        s.push_str("</g>");
    }
    s.push_str("</svg>");
    s
}

#[test]
fn deep_nesting_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    // Under usvg's own limit of 1024, deep enough to overflow a thread stack.
    for depth in [SVG_DEPTH_CAP - 1, 1000] {
        let src = dir.path().join("deep.svg");
        fs::write(&src, nested_groups(depth)).unwrap();
        let handle = {
            let cache = cache.clone();
            std::thread::Builder::new()
                .stack_size(2 * 1024 * 1024)
                .spawn(move || generate(&cache, &src, "image/svg+xml", ThumbSize::Normal))
                .unwrap()
        };
        let err = handle.join().unwrap().unwrap_err();
        assert!(matches!(err, ThumbError::Decode { .. }), "{depth}: {err:?}");
    }
}

#[test]
fn nesting_at_cap_renders_on_2mib_thread() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("deep.svg");
    // The svg element is depth 1 and the rect inside the groups the last level.
    fs::write(&src, nested_groups(SVG_DEPTH_CAP - 2)).unwrap();
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || generate(&cache, &src, "image/svg+xml", ThumbSize::Normal))
        .unwrap();
    handle.join().unwrap().unwrap();
}

#[test]
fn huge_gif_canvas_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("big.gif");
    // Header, 65535 x 65535 logical screen, no colour table, trailer.
    let mut bytes = b"GIF89a".to_vec();
    bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x3B]);
    fs::write(&src, bytes).unwrap();
    assert_decode_quickly(&cache, &src, "image/gif");
}

#[test]
fn huge_jpeg_header_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("big.jpg");
    // SOI, SOF0 for 65535 x 65535 with one component, EOI.
    let bytes = [
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x01, 0x11, 0x00,
        0xFF, 0xD9,
    ];
    fs::write(&src, bytes).unwrap();
    assert_decode_quickly(&cache, &src, "image/jpeg");
}

#[test]
fn huge_and_zero_svg_size() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let sized = |w: &str, h: &str| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><rect width="5" height="5"/></svg>"#
        )
    };
    let src = dir.path().join("size.svg");
    fs::write(&src, sized("1e30", "1e30")).unwrap();
    match generate(&cache, &src, "image/svg+xml", ThumbSize::Normal) {
        Ok(made) => {
            let (w, h) = dims(&made);
            assert!(w <= 128 && h <= 128 && w >= 1 && h >= 1);
        }
        Err(e) => assert!(matches!(e, ThumbError::Decode { .. }), "{e:?}"),
    }
    fs::write(&src, sized("0", "10")).unwrap();
    let err = generate(&cache, &src, "image/svg+xml", ThumbSize::Normal).unwrap_err();
    assert!(matches!(err, ThumbError::Decode { .. }), "{err:?}");
}

#[test]
fn svg_fifo_in_directory_does_not_hang() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let fifo = dir.path().join("pipe.png");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let src = dir.path().join("fifo.svg");
    fs::write(
        &src,
        svg(r#"<image width="10" height="10" xlink:href="pipe.png"/>"#),
    )
    .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(generate(&cache, &src, "image/svg+xml", ThumbSize::Normal).is_ok());
    });
    let done = rx.recv_timeout(std::time::Duration::from_secs(5));
    assert_eq!(done, Ok(true), "a FIFO named in an href must not block");
}

#[test]
fn svg_depth_scan_follows_markup_it_must_skip() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    // Comments, CDATA, a DOCTYPE with an internal subset and a `>` inside an
    // attribute value are not nesting.
    let doc = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [<!ENTITY a "x">]><!-- <g><g><g> --><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><desc><![CDATA[<g><g><g>]]></desc>{}<rect width="5" height="5" data-x="a>b"/></svg>"#,
        "<g/>".repeat(100)
    );
    let img = render_svg(&cache, dir.path(), "ok.svg", &doc);
    assert_eq!(img.dimensions(), (10, 10));

    let bad = doc.replace(r#"<!ENTITY a "x">"#, r#"<!ENTITY a "<g><g>">"#);
    let src = dir.path().join("entity.svg");
    fs::write(&src, bad).unwrap();
    assert_decode_quickly(&cache, &src, "image/svg+xml");
}

fn assert_decode_on_small_stack(cache: &Path, src: &Path) {
    let (cache, src) = (cache.to_path_buf(), src.to_path_buf());
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || generate(&cache, &src, "image/svg+xml", ThumbSize::Normal))
        .unwrap();
    let err = handle.join().unwrap().unwrap_err();
    assert!(matches!(err, ThumbError::Decode { .. }), "{err:?}");
}

fn render_on_small_stack(cache: &Path, src: &Path) {
    let (cache, src) = (cache.to_path_buf(), src.to_path_buf());
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || generate(&cache, &src, "image/svg+xml", ThumbSize::Normal))
        .unwrap()
        .join()
        .unwrap()
        .unwrap();
}

#[test]
fn data_url_svg_not_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let inner = nested_groups(1000);
    for mime in ["image/svg+xml", "text/plain", "image/png"] {
        let href = format!("data:{mime};base64,{}", base64(inner.as_bytes()));
        let outer = svg(&format!(
            r#"<image x="0" y="0" width="100" height="100" xlink:href="{href}"/>"#
        ));
        let src = dir.path().join("outer.svg");
        fs::write(&src, outer).unwrap();
        render_on_small_stack(&cache, &src);
        let made = lookup(&cache, &src, ThumbSize::Normal).unwrap();
        let Lookup::Fresh(made) = made else {
            panic!("{mime}: not stored")
        };
        let plain = render_svg(&cache, dir.path(), "plain.svg", &svg(""));
        assert!(pixels(&made).as_raw() == plain.as_raw(), "{mime}");
    }
}

/// `count` defs g1..g<count>, each wrapping `<use>` of the previous in
/// `per` nested groups; g0 holds a rect. The root uses the last one.
fn use_chain(count: usize, per: usize) -> String {
    let mut s = SVG_HEAD.to_string();
    s.push_str(r#"<defs><g id="g0"><rect width="50" height="50" fill="red"/></g>"#);
    for k in 1..=count {
        s.push_str(&format!(r#"<g id="g{k}">"#));
        s.push_str(&"<g>".repeat(per));
        s.push_str(&format!(r##"<use xlink:href="#g{}"/>"##, k - 1));
        s.push_str(&"</g>".repeat(per));
        s.push_str("</g>");
    }
    s.push_str(&format!(r##"</defs><use xlink:href="#g{count}"/></svg>"##));
    s
}

#[test]
fn use_chain_depth_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("chain.svg");
    // Document depth 31, effective depth several hundred.
    for (count, per) in [(30, 27), (200, 5)] {
        fs::write(&src, use_chain(count, per)).unwrap();
        assert_decode_on_small_stack(&cache, &src);
    }
}

#[test]
fn use_chain_within_cap_renders() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("chain.svg");
    // g0 is 2 deep; each of g1..g3 adds 4 groups, its own element and the
    // use: 6 levels. Root svg and use add 2: 2 + 6 + 6 + 6 + 2 = 22.
    fs::write(&src, use_chain(3, 4)).unwrap();
    render_on_small_stack(&cache, &src);
    let px = pixels(&match lookup(&cache, &src, ThumbSize::Normal).unwrap() {
        Lookup::Fresh(p) => p,
        other => panic!("{other:?}"),
    });
    assert_eq!(px.get_pixel(5, 5).0, [255, 0, 0, 255]);
}

#[test]
fn use_cycle_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("cycle.svg");
    let doc = format!(
        r##"{SVG_HEAD}<defs><g id="a"><use xlink:href="#b"/></g><g id="b"><use xlink:href="#a"/></g></defs><use xlink:href="#a"/></svg>"##
    );
    fs::write(&src, doc).unwrap();
    assert_decode_quickly(&cache, &src, "image/svg+xml");
}

#[test]
fn use_fanout_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("fan.svg");
    // Ten levels of three uses each: depth about 20, 3^10 instances.
    let mut doc = format!(r#"{SVG_HEAD}<defs><g id="g0"><rect width="5" height="5"/></g>"#);
    for k in 1..=10 {
        doc.push_str(&format!(r##"<g id="g{k}">"##));
        for _ in 0..3 {
            doc.push_str(&format!(r##"<use xlink:href="#g{}"/>"##, k - 1));
        }
        doc.push_str("</g>");
    }
    doc.push_str(r##"</defs><use xlink:href="#g10"/></svg>"##);
    fs::write(&src, doc).unwrap();
    assert_decode_quickly(&cache, &src, "image/svg+xml");
}

#[test]
fn doctype_with_apostrophe_in_pi_is_followed() {
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(dir.path());
    let src = dir.path().join("pi.svg");
    let doc = nested_groups(1000).replacen(
        "<svg",
        "<!DOCTYPE svg [<?pi it's a pi?><!ENTITY a \"x\">]><svg",
        1,
    );
    fs::write(&src, doc).unwrap();
    assert_decode_on_small_stack(&cache, &src);
}
