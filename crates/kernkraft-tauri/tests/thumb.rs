// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The thumbnail queue: events, cancelling, and the cache left alone.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{env, path, w, Env};
use kernkraft_tauri::{ThumbEvent, EVENT_THUMB};

/// A small SVG that renders: one filled square.
const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" fill="#3f5b8b"/></svg>"##;

/// An SVG that takes a while to render: many overlapping circles, so a
/// worker is still busy when the test cancels.
fn slow_svg() -> String {
    let mut s =
        String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="2000">"#);
    for i in 0..2000 {
        s.push_str(&format!(
            r#"<circle cx="{}" cy="{}" r="400" fill-opacity="0.3"/>"#,
            i % 2000,
            (i * 7) % 2000
        ));
    }
    s.push_str("</svg>");
    s
}

fn thumb_events(e: &Env, count: usize) -> Vec<ThumbEvent> {
    (0..count)
        .map(|_| serde_json::from_value(e.wait_event(EVENT_THUMB)).unwrap())
        .collect()
}

/// Thumb events that arrive within `quiet`, which a test expects to be
/// none.
fn late_thumb_events(e: &Env, quiet: Duration) -> Vec<ThumbEvent> {
    let mut late = Vec::new();
    while let Ok(ev) = e.events.recv_timeout(quiet) {
        if ev.name == EVENT_THUMB {
            late.push(serde_json::from_value(ev.payload).unwrap());
        }
    }
    late
}

fn cache_pngs(dir: &Path) -> usize {
    fs::read_dir(dir).map_or(0, |d| d.count())
}

#[test]
fn thumb_events_for_images_only() {
    let e = env();
    let dir = e.dir("pics");
    let svg = dir.join("square.svg");
    fs::write(&svg, SVG).unwrap();
    // Named as a PNG, not one: no thumbnail, and no error either.
    let broken = dir.join("broken.png");
    fs::write(&broken, b"not a png at all").unwrap();
    let notes = dir.join("notes.txt");
    fs::write(&notes, "text").unwrap();
    // An image inside the thumbnail cache itself is never thumbnailed.
    let cache = e.xdg.cache.join("thumbnails/normal");
    fs::create_dir_all(&cache).unwrap();
    let inside = cache.join("own.svg");
    fs::write(&inside, SVG).unwrap();

    e.kk.thumb_request(
        "grid".to_owned(),
        vec![w(&notes), w(&svg), w(&broken), w(&inside)],
        "normal".to_owned(),
        None,
    )
    .unwrap();

    let got: HashMap<PathBuf, ThumbEvent> = thumb_events(&e, 3)
        .into_iter()
        .map(|ev| (path(&ev.path), ev))
        .collect();
    assert_eq!(got.len(), 3);
    for ev in got.values() {
        assert_eq!(ev.key, "grid");
        assert_eq!(ev.error, None, "{ev:?}");
    }
    let file = path(got[&svg].file.as_ref().expect("the SVG has a thumbnail"));
    assert!(file.starts_with(&cache), "{}", file.display());
    assert!(file.is_file());
    assert_eq!(got[&broken].file, None);
    assert_eq!(got[&inside].file, None);
    // The cache holds the SVG's thumbnail and the planted file, nothing
    // made from the planted file.
    assert_eq!(cache_pngs(&cache), 2);
    // The text file gets no event at all.
    assert!(late_thumb_events(&e, Duration::from_millis(300)).is_empty());

    // A second request is served from the cache: the same file.
    e.kk.thumb_request("grid".to_owned(), vec![w(&svg)], "normal".to_owned(), None)
        .unwrap();
    let again = thumb_events(&e, 1).remove(0);
    assert_eq!(again.file.map(|f| path(&f)), Some(file));
}

#[test]
fn thumb_cancel_stops_queue() {
    let e = env();
    let dir = e.dir("many");
    let body = slow_svg();
    const COUNT: usize = 300;
    let paths: Vec<PathBuf> = (0..COUNT)
        .map(|i| {
            let p = dir.join(format!("{i:03}.svg"));
            fs::write(&p, &body).unwrap();
            p
        })
        .collect();

    e.kk.thumb_request(
        "folder".to_owned(),
        paths.iter().map(|p| w(p)).collect(),
        "normal".to_owned(),
        None,
    )
    .unwrap();
    e.kk.thumb_cancel("folder".to_owned()).unwrap();
    // Every event sent before the cancel returned is already queued here.
    let mut before = 0;
    while let Ok(ev) = e.events.try_recv() {
        if ev.name == EVENT_THUMB {
            before += 1;
        }
    }

    // Another key still works, and is served after nothing of the first.
    let other = e.dir("other").join("one.svg");
    fs::write(&other, SVG).unwrap();
    e.kk.thumb_request(
        "other".to_owned(),
        vec![w(&other)],
        "normal".to_owned(),
        None,
    )
    .unwrap();
    let mut after_cancel = Vec::new();
    loop {
        let ev: ThumbEvent = serde_json::from_value(e.wait_event(EVENT_THUMB)).unwrap();
        if ev.key == "other" {
            assert!(ev.file.is_some());
            break;
        }
        after_cancel.push(ev);
    }
    // Items in flight at the cancel finish without an event.
    after_cancel.extend(late_thumb_events(&e, Duration::from_secs(2)));
    assert!(after_cancel.is_empty(), "{after_cancel:?}");

    // The queued work was dropped, not only silenced: far fewer thumbnails
    // than requested exist (the other key's one among them).
    let made = cache_pngs(&e.xdg.cache.join("thumbnails/normal"));
    assert!(
        made < COUNT,
        "{made} of {COUNT} made; {before} events before"
    );
}

#[test]
fn thumb_request_refuses_unknown_size() {
    let e = env();
    let err =
        e.kk.thumb_request(
            "k".to_owned(),
            vec![w(&e.base.join("a.svg"))],
            "huge".to_owned(),
            None,
        )
        .unwrap_err();
    assert_eq!(err.kind, "InvalidArgument");
}
