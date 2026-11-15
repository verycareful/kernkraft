// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::sync::atomic::{AtomicU64, Ordering};

use kernkraft_fs::{NoProgress, Progress};
use kernkraft_pdf::{
    split_by_bookmarks, split_per_page, split_ranges, PageRange, PdfError, START_PART_TITLE,
};

use super::*;

fn range(start: u32, end: u32) -> PageRange {
    PageRange { start, end }
}

#[test]
fn one_document_per_page() {
    let document = open(&pdf_n(3));
    let parts = split_per_page(&document, &NoProgress).unwrap();
    let marks: Vec<Vec<String>> = parts.iter().map(markers).collect();
    assert_eq!(marks, [["P1"], ["P2"], ["P3"]]);
    for part in &parts {
        assert_eq!(markers(&reopen(part)).len(), 1);
    }
}

struct Counting {
    done: AtomicU64,
    cancel_after: u64,
}

impl Progress for Counting {
    fn report(&self, done: u64, _total: u64, _stage: &str) {
        self.done.store(done, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.done.load(Ordering::SeqCst) >= self.cancel_after
    }
}

#[test]
fn splitting_per_page_can_be_cancelled() {
    let document = open(&pdf_n(5));
    let progress = Counting {
        done: AtomicU64::new(0),
        cancel_after: 2,
    };
    assert!(matches!(
        split_per_page(&document, &progress),
        Err(PdfError::Cancelled)
    ));
    assert_eq!(progress.done.load(Ordering::SeqCst), 2);
}

#[test]
fn one_document_per_range() {
    let document = open(&pdf_n(5));
    let parts = split_ranges(&document, &[range(1, 2), range(4, 5), range(3, 3)]).unwrap();
    let marks: Vec<Vec<String>> = parts.iter().map(markers).collect();
    assert_eq!(marks, [vec!["P1", "P2"], vec!["P4", "P5"], vec!["P3"]]);
}

#[test]
fn bad_ranges_are_refused() {
    let document = open(&pdf_n(3));
    assert!(matches!(
        split_ranges(&document, &[]),
        Err(PdfError::Invalid(_))
    ));
    assert!(matches!(
        split_ranges(&document, &[range(3, 2)]),
        Err(PdfError::BadRange(_))
    ));
    assert!(matches!(
        split_ranges(&document, &[range(1, 4)]),
        Err(PdfError::PageOutOfRange { page: 4, count: 3 })
    ));
}

#[test]
fn parts_keep_the_outline_that_leads_into_them() {
    let document = open(&features("").bytes);
    let parts = split_per_page(&document, &NoProgress).unwrap();
    assert_eq!(outline(&parts[0]), [(0, "One".to_string(), Some(1))]);
    assert_eq!(
        outline(&parts[1]),
        [
            (0, "One.A".to_string(), Some(1)),
            (0, "Two".to_string(), Some(1))
        ]
    );
    assert_eq!(outline(&parts[2]), [(0, "Three".to_string(), Some(1))]);
    assert_eq!(field_names(&parts[0]), Vec::<String>::new());
    assert_eq!(field_names(&parts[1]), ["_f2"]);
    assert_eq!(field_names(&parts[2]), ["_f3"]);
    assert_eq!(labels(&parts[2]).unwrap(), ["A-2"]);
}

#[test]
fn splits_at_the_top_level_bookmarks() {
    let document = open(&features("").bytes);
    let parts = split_by_bookmarks(&document).unwrap();
    let summary: Vec<(String, Vec<String>)> = parts
        .iter()
        .map(|(title, part)| (title.clone(), markers(part)))
        .collect();
    assert_eq!(
        summary,
        [
            ("One".to_string(), vec!["P1".to_string()]),
            ("Two".to_string(), vec!["P2".to_string()]),
            ("Three".to_string(), vec!["P3".to_string()]),
        ]
    );
}

fn with_bookmarks(pages: usize, items: &[(&str, u32)]) -> Document {
    let mut extra: Vec<(u32, String)> = Vec::new();
    let last = 30 + items.len() as u32;
    extra.push((
        30,
        format!(
            "<< /Type /Outlines /First 31 0 R /Last {last} 0 R /Count {} >>",
            items.len()
        ),
    ));
    for (i, (title, page)) in items.iter().enumerate() {
        let number = 31 + i as u32;
        let prev = if i > 0 {
            format!("/Prev {} 0 R", number - 1)
        } else {
            String::new()
        };
        let next = if i + 1 < items.len() {
            format!("/Next {} 0 R", number + 1)
        } else {
            String::new()
        };
        extra.push((
            number,
            format!(
                "<< /Title ({title}) /Parent 30 0 R {prev} {next} /Dest [{} 0 R /Fit] >>",
                page_object(page - 1)
            ),
        ));
    }
    let borrowed: Vec<(u32, &str)> = extra.iter().map(|(n, s)| (*n, s.as_str())).collect();
    let fixture = flat_with(
        &vec![(100.0, 100.0); pages],
        "/Outlines 30 0 R",
        "",
        "",
        &borrowed,
    );
    open(&fixture.bytes)
}

#[test]
fn pages_before_the_first_bookmark_form_the_start_part() {
    let document = with_bookmarks(4, &[("Later", 3), ("Middle", 2)]);
    let parts = split_by_bookmarks(&document).unwrap();
    let summary: Vec<(String, Vec<String>)> = parts
        .iter()
        .map(|(title, part)| (title.clone(), markers(part)))
        .collect();
    assert_eq!(
        summary,
        [
            (START_PART_TITLE.to_string(), vec!["P1".to_string()]),
            ("Middle".to_string(), vec!["P2".to_string()]),
            (
                "Later".to_string(),
                vec!["P3".to_string(), "P4".to_string()]
            ),
        ]
    );
}

#[test]
fn a_second_bookmark_on_one_page_starts_no_part() {
    let document = with_bookmarks(3, &[("First", 1), ("Same", 1), ("Next", 3)]);
    let titles: Vec<String> = split_by_bookmarks(&document)
        .unwrap()
        .into_iter()
        .map(|(title, _)| title)
        .collect();
    assert_eq!(titles, ["First", "Next"]);
}

#[test]
fn no_bookmark_that_leads_to_a_page_is_refused() {
    assert!(matches!(
        split_by_bookmarks(&open(&pdf_n(2))),
        Err(PdfError::Invalid(_))
    ));
}
