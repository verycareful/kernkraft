// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Searching: `text/search.rs`, and `Document::search`.

use super::*;
use kernkraft_pdf::text::{SearchHit, SearchOptions};
use kernkraft_pdf::PdfError;

const TWO_LINES: &str = "BT /F1 10 Tf 100 700 Td (Hello World) Tj 0 -12 Td (Another line) Tj ET";

fn found(page: &TextPage, query: &str, options: &SearchOptions) -> Vec<(usize, usize)> {
    page.search(query, options)
        .into_iter()
        .map(|m| (m.start, m.end))
        .collect()
}

#[test]
fn a_search_ignores_case_unless_asked() {
    let page = page(TWO_LINES);
    let any = SearchOptions::default();
    assert_eq!(found(&page, "hello", &any), [(0, 5)]);
    let exact = SearchOptions {
        case_sensitive: true,
        ..any
    };
    assert!(found(&page, "hello", &exact).is_empty());
    assert_eq!(found(&page, "Hello", &exact), [(0, 5)]);
}

#[test]
fn a_match_has_the_rectangle_of_its_characters() {
    let page = page(TWO_LINES);
    let matches = page.search("World", &SearchOptions::default());
    assert_eq!(matches.len(), 1);
    // World is characters 6 to 10 of the first line: x 136 to 166, y 84
    // to 94.
    assert_eq!(matches[0].rects.len(), 1);
    assert_rect(matches[0].rects[0], 136.0, 84.0, 166.0, 94.0, "World");
}

#[test]
fn a_match_may_cross_a_line_break_and_has_a_rectangle_for_each_line() {
    // The break is white space: "World Another" matches across it.
    let page = page(TWO_LINES);
    let matches = page.search("world another", &SearchOptions::default());
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].rects.len(), 2);
    assert_rect(matches[0].rects[0], 136.0, 84.0, 166.0, 94.0, "first line");
    assert_rect(
        matches[0].rects[1],
        100.0,
        96.0,
        142.0,
        106.0,
        "second line",
    );
}

/// `a well-` ending a line and `known word` starting the next, in 10 point
/// Courier: 6 points a character, so `well-` runs from x 112 to 142 and
/// `known` from x 100 to 130.
const BROKEN: &str = "BT /F1 10 Tf 100 700 Td (a well-) Tj 0 -12 Td (known word) Tj ET";

#[test]
fn a_match_may_span_a_line_end_hyphen_and_the_break() {
    let page = page(BROKEN);
    let options = SearchOptions::default();
    // The text is as written, hyphen and break in it.
    assert_eq!(page.text(), "a well-\nknown word");
    // The word without the hyphen, with it and as the text reads.
    for query in ["wellknown", "well-known", "well- known"] {
        let matches = page.search(query, &options);
        assert_eq!(matches.len(), 1, "{query}");
        assert_eq!(matches[0].rects.len(), 2, "{query}");
    }
    // The first line's part of the match runs from `w` (x 112) to the end of
    // the hyphen (x 142).
    let whole = &page.search("wellknown", &options)[0];
    assert_rect(whole.rects[0], 112.0, 84.0, 142.0, 94.0, "first line");
    assert_rect(whole.rects[1], 100.0, 96.0, 130.0, 106.0, "second line");
}

#[test]
fn the_parts_of_a_broken_word_are_each_found_once() {
    let page = page(BROKEN);
    let options = SearchOptions::default();
    assert_eq!(page.search("known", &options).len(), 1);
    assert_eq!(page.search("well", &options).len(), 1);
    let whole = SearchOptions {
        whole_word: true,
        ..options
    };
    assert_eq!(page.search("wellknown", &whole).len(), 1);
}

#[test]
fn white_space_in_the_query_and_the_text_is_one_space() {
    let page = page(TWO_LINES);
    let any = SearchOptions::default();
    assert_eq!(found(&page, "  hello \t  world  ", &any).len(), 1);
}

#[test]
fn a_whole_word_is_not_part_of_a_longer_one() {
    let page = page("BT /F1 10 Tf 100 700 Td (cat concat cat.) Tj ET");
    let any = SearchOptions::default();
    assert_eq!(found(&page, "cat", &any), [(0, 3), (7, 10), (11, 14)]);
    let whole = SearchOptions {
        whole_word: true,
        ..any
    };
    // Not the one inside "concat".
    assert_eq!(found(&page, "cat", &whole), [(0, 3), (11, 14)]);
}

#[test]
fn matches_do_not_overlap() {
    let page = page("BT /F1 10 Tf 100 700 Td (aaaa) Tj ET");
    assert_eq!(
        found(&page, "aa", &SearchOptions::default()),
        [(0, 2), (2, 4)]
    );
}

#[test]
fn an_empty_query_matches_nothing() {
    let page = page(TWO_LINES);
    let any = SearchOptions::default();
    assert!(found(&page, "", &any).is_empty());
    assert!(found(&page, "  \n ", &any).is_empty());
}

#[test]
fn a_ligature_matches_its_letters() {
    // Code 1 of F2 is `fi`, U+FB01: one character that NFKC reads as two.
    let fixture = document(
        &[PageSpec::of("BT /F2 10 Tf 100 700 Td (\\001nd) Tj ET")],
        "",
        "",
        &[],
    );
    let page = text_of(&fixture);
    assert_eq!(page.text(), "\u{FB01}nd");
    assert_eq!(found(&page, "find", &SearchOptions::default()), [(0, 3)]);
    assert_eq!(
        found(&page, "\u{FB01}nd", &SearchOptions::default()),
        [(0, 3)]
    );
}

#[test]
fn a_synthetic_space_is_a_space_in_a_search() {
    let page = page("BT /F1 10 Tf 100 700 Td (Hello) Tj 34 0 Td (World) Tj ET");
    assert_eq!(
        found(&page, "hello world", &SearchOptions::default()),
        [(0, 11)]
    );
}

fn pages() -> Fixture {
    document(
        &[
            PageSpec::of("BT /F1 10 Tf 100 700 Td (one two) Tj ET"),
            PageSpec::of("BT /F1 10 Tf 100 700 Td (two three) Tj ET"),
            PageSpec::of("BT /F1 10 Tf 100 700 Td (TWO) Tj ET"),
        ],
        "",
        "",
        &[],
    )
}

#[test]
fn a_document_search_gives_the_page_of_each_match() {
    let hits = open(&pages())
        .search("two", &SearchOptions::default(), &NoProgress)
        .unwrap();
    let places: Vec<(u32, usize, usize)> = hits
        .iter()
        .map(|SearchHit { page, found }| (*page, found.start, found.end))
        .collect();
    assert_eq!(places, [(1, 4, 7), (2, 0, 3), (3, 0, 3)]);
}

#[test]
fn a_document_search_stops_when_cancelled() {
    struct Cancelled;
    impl kernkraft_fs::Progress for Cancelled {
        fn report(&self, _: u64, _: u64, _: &str) {}
        fn is_cancelled(&self) -> bool {
            true
        }
    }
    let error = open(&pages())
        .search("two", &SearchOptions::default(), &Cancelled)
        .unwrap_err();
    assert!(matches!(error, PdfError::Cancelled));
}

#[test]
fn a_document_search_reports_its_progress() {
    use std::sync::Mutex;
    struct Reports(Mutex<Vec<(u64, u64)>>);
    impl kernkraft_fs::Progress for Reports {
        fn report(&self, done: u64, total: u64, _: &str) {
            self.0.lock().unwrap().push((done, total));
        }
    }
    let reports = Reports(Mutex::new(Vec::new()));
    open(&pages())
        .search("two", &SearchOptions::default(), &reports)
        .unwrap();
    assert_eq!(*reports.0.lock().unwrap(), [(0, 3), (1, 3), (2, 3), (3, 3)]);
}
