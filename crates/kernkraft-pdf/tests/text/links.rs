// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Link annotations and where destinations lead: `src/links.rs` and
//! `named_destinations` of `src/destination.rs`. ISO 32000-1, 12.5.6.5 and
//! 12.3.2.

use super::*;
use kernkraft_pdf::{
    ActionKind, Destination, DestinationPage, DiagnosticKind, Located, NamedDestination, View,
};

/// A two-page document (objects 10 and 12) whose first page has the
/// annotations `annots`, with `catalog` entries added.
fn linked(annots: &str, catalog: &str, extra: &[(u32, &[u8])]) -> Fixture {
    document(
        &[
            PageSpec {
                content: "",
                entries: &format!("/Annots [{annots}]"),
            },
            PageSpec::of(""),
        ],
        catalog,
        "",
        extra,
    )
}

fn link_dict(rect: &str, rest: &str) -> String {
    format!("<< /Type /Annot /Subtype /Link /Rect {rect} {rest} >>")
}

#[test]
fn a_link_with_a_destination_gives_its_region_and_target() {
    // The rectangle [100 700 200 720] is 20 high from y 72 down to 92.
    let fixture = linked(
        &link_dict("[100 700 200 720]", "/Dest [12 0 R /XYZ 10 20 1.5]"),
        "",
        &[],
    );
    let links = open(&fixture).links(1).unwrap();
    assert_eq!(links.len(), 1);
    let link = &links[0];
    assert_eq!(link.rects.len(), 1);
    assert_rect(link.rects[0], 100.0, 72.0, 200.0, 92.0, "the link");
    assert_eq!(
        link.destination,
        Some(Destination {
            page: DestinationPage::Page(2),
            view: View::Xyz {
                left: Some(10.0),
                top: Some(20.0),
                zoom: Some(1.5)
            },
        })
    );
    assert_eq!(link.target(), link.destination.as_ref());
    assert!(link.contains(150.0 / PAGE_WIDTH, 80.0 / PAGE_HEIGHT));
    assert!(!link.contains(250.0 / PAGE_WIDTH, 80.0 / PAGE_HEIGHT));
    assert_eq!(link.border_width, 1.0);
    assert!(link.annotation.is_none());
}

#[test]
fn the_corners_of_a_rect_may_come_in_either_order() {
    let fixture = linked(&link_dict("[200 720 100 700]", ""), "", &[]);
    let links = open(&fixture).links(1).unwrap();
    assert_rect(links[0].rects[0], 100.0, 72.0, 200.0, 92.0, "the link");
}

#[test]
fn a_uri_action_gives_the_address() {
    let fixture = linked(
        &link_dict(
            "[0 0 10 10]",
            "/A << /S /URI /URI (https://example.org/a) >>",
        ),
        "",
        &[],
    );
    let links = open(&fixture).links(1).unwrap();
    assert_eq!(links[0].uri(), Some("https://example.org/a"));
    assert_eq!(links[0].actions[0].kind, ActionKind::Uri);
    assert!(links[0].target().is_none());
}

#[test]
fn a_go_to_action_gives_its_destination_as_the_target() {
    let fixture = linked(
        &link_dict("[0 0 10 10]", "/A << /S /GoTo /D [12 0 R /Fit] >>"),
        "",
        &[],
    );
    let links = open(&fixture).links(1).unwrap();
    assert!(links[0].destination.is_none());
    assert_eq!(
        links[0].target(),
        Some(&Destination {
            page: DestinationPage::Page(2),
            view: View::Fit
        })
    );
}

#[test]
fn a_named_destination_is_looked_up() {
    let fixture = linked(
        &link_dict("[0 0 10 10]", "/Dest (chapter1)"),
        "/Names << /Dests 80 0 R >>",
        &[(80, b"<< /Names [(chapter1) [12 0 R /FitH 300]] >>")],
    );
    let links = open(&fixture).links(1).unwrap();
    assert_eq!(
        links[0].destination,
        Some(Destination {
            page: DestinationPage::Page(2),
            view: View::FitH { top: Some(300.0) }
        })
    );
}

#[test]
fn quad_points_give_one_region_for_each_quadrilateral() {
    // Two lines of a link: [100 700 180 712] and [50 688 120 700].
    let fixture = linked(
        &link_dict(
            "[50 688 180 712]",
            "/QuadPoints [100 712 180 712 100 700 180 700  50 700 120 700 50 688 120 688]",
        ),
        "",
        &[],
    );
    let links = open(&fixture).links(1).unwrap();
    assert_eq!(links[0].rects.len(), 2);
    assert_rect(links[0].rects[0], 100.0, 80.0, 180.0, 92.0, "first line");
    assert_rect(links[0].rects[1], 50.0, 92.0, 120.0, 104.0, "second line");
}

#[test]
fn quad_points_that_are_not_whole_quadrilaterals_leave_the_rect() {
    let fixture = linked(
        &link_dict("[100 700 200 720]", "/QuadPoints [1 2 3 4 5 6 7]"),
        "",
        &[],
    );
    let links = open(&fixture).links(1).unwrap();
    assert_eq!(links[0].rects.len(), 1);
    assert_rect(links[0].rects[0], 100.0, 72.0, 200.0, 92.0, "the rect");
}

#[test]
fn a_hidden_or_not_viewable_link_is_not_given() {
    // Table 165: bit 2 (2) is Hidden, bit 6 (32) is NoView; Print (4) is
    // a link still.
    let annots = [
        link_dict("[0 0 1 1]", "/F 2"),
        link_dict("[0 0 2 2]", "/F 32"),
        link_dict("[0 0 3 3]", "/F 4"),
    ]
    .join(" ");
    let links = open(&linked(&annots, "", &[])).links(1).unwrap();
    assert_eq!(links.len(), 1);
    assert_rect(
        links[0].rects[0],
        0.0,
        789.0,
        3.0,
        792.0,
        "the printed link",
    );
}

#[test]
fn only_link_annotations_are_given() {
    let annots = format!(
        "<< /Type /Annot /Subtype /Text /Rect [0 0 5 5] >> {}",
        link_dict("[0 0 1 1]", "")
    );
    assert_eq!(open(&linked(&annots, "", &[])).links(1).unwrap().len(), 1);
}

#[test]
fn a_link_with_no_rect_is_left_out_and_reported() {
    let annots = "<< /Type /Annot /Subtype /Link /Rect [0 0 5] >>";
    let document = open(&linked(annots, "", &[]));
    assert!(document.links(1).unwrap().is_empty());
    assert!(document.diagnostics().iter().any(
        |d| matches!(&d.kind, DiagnosticKind::IgnoredEntry { key } if key.as_bytes() == b"Rect")
    ));
}

#[test]
fn the_border_width_comes_from_the_style_or_the_border() {
    let annots = [
        link_dict("[0 0 1 1]", "/BS << /W 3 >> /Border [0 0 9]"),
        link_dict("[0 0 2 2]", "/Border [0 0 2.5]"),
        link_dict("[0 0 3 3]", "/Border [0 0 0]"),
        link_dict("[0 0 4 4]", ""),
    ]
    .join(" ");
    let links = open(&linked(&annots, "", &[])).links(1).unwrap();
    let widths: Vec<f64> = links.iter().map(|l| l.border_width).collect();
    assert_eq!(widths, [3.0, 2.5, 0.0, 1.0]);
}

#[test]
fn an_indirect_annotation_is_named() {
    let fixture = linked("81 0 R", "", &[(81, link_dict("[0 0 1 1]", "").as_bytes())]);
    let links = open(&fixture).links(1).unwrap();
    assert_eq!(links[0].annotation, Some(id(81)));
}

#[test]
fn a_page_with_no_annotations_has_no_links() {
    assert!(open(&linked("", "", &[])).links(2).unwrap().is_empty());
}

#[test]
fn a_page_that_is_turned_turns_its_links() {
    // Rotate 90: the user point (x, y) is at (y, x) on a page 792 wide and
    // 612 high.
    let fixture = document(
        &[PageSpec {
            content: "",
            entries:
                "/Rotate 90 /Annots [<< /Type /Annot /Subtype /Link /Rect [100 700 200 720] >>]",
        }],
        "",
        "",
        &[],
    );
    let links = open(&fixture).links(1).unwrap();
    let rect = links[0].rects[0];
    assert_close(rect.left, 700.0 / 792.0, "left");
    assert_close(rect.right, 720.0 / 792.0, "right");
    assert_close(rect.top, 100.0 / 612.0, "top");
    assert_close(rect.bottom, 200.0 / 612.0, "bottom");
}

fn located(destination: Destination, rotate: u16) -> Option<Located> {
    let fixture = document(
        &[
            PageSpec {
                content: "",
                entries: &format!("/Rotate {rotate}"),
            },
            PageSpec::of(""),
        ],
        "",
        "",
        &[],
    );
    open(&fixture).locate(&destination).unwrap()
}

fn to_page(view: View) -> Destination {
    Destination {
        page: DestinationPage::Page(1),
        view,
    }
}

#[test]
fn a_destination_shows_at_a_point_of_its_page() {
    // XYZ 100 600 on an upright page: 100 from the left and 792 - 600 =
    // 192 from the top.
    let at = located(
        to_page(View::Xyz {
            left: Some(100.0),
            top: Some(600.0),
            zoom: Some(2.0),
        }),
        0,
    )
    .unwrap();
    assert_eq!(at.page, 1);
    assert_close(at.x.unwrap(), 100.0 / PAGE_WIDTH, "x");
    assert_close(at.y.unwrap(), 192.0 / PAGE_HEIGHT, "y");
    assert_eq!(at.zoom, Some(2.0));
}

#[test]
fn a_view_fixes_only_the_axes_it_gives() {
    let none = |view| {
        let at = located(to_page(view), 0).unwrap();
        (at.x, at.y)
    };
    assert_eq!(none(View::Fit), (None, None));
    assert_eq!(none(View::FitB), (None, None));
    let (x, y) = none(View::FitH { top: Some(692.0) });
    assert_eq!(x, None);
    assert_close(y.unwrap(), 100.0 / PAGE_HEIGHT, "FitH top");
    let (x, y) = none(View::FitBV { left: Some(61.2) });
    assert_close(x.unwrap(), 61.2 / PAGE_WIDTH, "FitBV left");
    assert_eq!(y, None);
    let (x, y) = none(View::FitR {
        left: 10.0,
        bottom: 0.0,
        right: 20.0,
        top: 792.0,
    });
    assert_close(x.unwrap(), 10.0 / PAGE_WIDTH, "FitR left");
    assert_close(y.unwrap(), 0.0, "FitR top");
    let (x, y) = none(View::Xyz {
        left: None,
        top: None,
        zoom: None,
    });
    assert_eq!((x, y), (None, None));
}

#[test]
fn a_destination_on_a_turned_page_follows_the_turn() {
    // Rotate 90: the user point (x, y) is at (y, x) on the displayed page,
    // 792 wide and 612 high. FitH 692 fixes the displayed x: 692 / 792.
    let at = located(to_page(View::FitH { top: Some(692.0) }), 90).unwrap();
    assert_close(at.x.unwrap(), 692.0 / 792.0, "x");
    assert_eq!(at.y, None);
    // Rotate 180: (x, y) is at (612 - x, y) of an upright size: 612 - 100
    // from the left, 600 from the top.
    let at = located(
        to_page(View::Xyz {
            left: Some(100.0),
            top: Some(600.0),
            zoom: None,
        }),
        180,
    )
    .unwrap();
    assert_close(at.x.unwrap(), 512.0 / 612.0, "x");
    assert_close(at.y.unwrap(), 600.0 / 792.0, "y");
}

#[test]
fn a_destination_in_another_file_is_a_page_index() {
    let by_index = |index| {
        located(
            Destination {
                page: DestinationPage::Index(index),
                view: View::Fit,
            },
            0,
        )
    };
    assert_eq!(by_index(0).unwrap().page, 1);
    assert_eq!(by_index(1).unwrap().page, 2);
    assert_eq!(by_index(2), None);
    assert_eq!(by_index(u32::MAX), None);
}

#[test]
fn a_destination_whose_page_is_missing_leads_nowhere() {
    let missing = Destination {
        page: DestinationPage::Missing(id(99)),
        view: View::Fit,
    };
    assert_eq!(located(missing, 0), None);
    let zero = Destination {
        page: DestinationPage::Page(0),
        view: View::Fit,
    };
    assert_eq!(located(zero, 0), None);
}

#[test]
fn the_destinations_a_document_names_are_listed_from_both_places() {
    let fixture = linked(
        "",
        "/Dests 80 0 R /Names << /Dests 81 0 R >>",
        &[
            (
                80,
                b"<< /intro [10 0 R /Fit] /other << /D [12 0 R /FitB] >> /broken 5 >>",
            ),
            (
                81,
                b"<< /Names [(chapter1) [12 0 R /FitV 7] (bad) [99 0 R]] >>",
            ),
        ],
    );
    let document = open(&fixture);
    let mut named = document.named_destinations().unwrap();
    named.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&[u8]> = named.iter().map(|n| n.name.as_slice()).collect();
    assert_eq!(names, [b"chapter1".as_slice(), b"intro", b"other"]);
    assert_eq!(
        named[1],
        NamedDestination {
            name: b"intro".to_vec(),
            destination: Destination {
                page: DestinationPage::Page(1),
                view: View::Fit
            }
        }
    );
    // `broken` and `bad` are no destinations.
    let invalid = document
        .diagnostics()
        .into_iter()
        .filter(|d| d.kind == DiagnosticKind::InvalidDestination)
        .count();
    assert_eq!(invalid, 2);
}

#[test]
fn a_document_with_no_named_destinations_lists_none() {
    assert!(open(&linked("", "", &[]))
        .named_destinations()
        .unwrap()
        .is_empty());
}
