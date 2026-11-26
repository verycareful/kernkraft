// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `edit_text`: new text takes the place of a block's text (ISO 32000-1,
//! 9.3, 9.4.2 and 9.4.4).
//!
//! Courier glyphs are 600 units wide, 6 points at 10 points, and the font
//! has no descriptor, so a line is 800 + 200 = 1000 units apart (the
//! fallback of `text/collect.rs`), 10 points at 10 points. The font of
//! `embed_truetype` has `A` 700, `B` 450 and the space 250 units wide.

use super::*;
use kernkraft_font::FontList;
use kernkraft_pdf::colour::Rgb;
use kernkraft_pdf::content::StreamId;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::Name;
use kernkraft_pdf::syntax::{ObjectId, Value};
use kernkraft_pdf::{
    edit_text, DiagnosticKind, EditBlocker, Editability, FontSubstitution, PdfError, TextEdit,
    TextStyle,
};

fn page_stream() -> StreamId {
    StreamId::Page(ObjectId {
        number: PAGE,
        generation: 0,
    })
}

/// The page's decoded content with every run of white space as one space
/// and none inside brackets.
pub(super) fn content(doc: &Document) -> String {
    let data = doc.content_stream(page_stream(), 1 << 20).unwrap().data;
    String::from_utf8(data)
        .unwrap()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("[ ", "[")
        .replace(" ]", "]")
}

/// The face list of the one test font, "Test Sans", kept in `dir`.
pub(super) fn font_list(dir: &tempfile::TempDir) -> FontList {
    let path = dir.path().join("test-sans.ttf");
    std::fs::write(&path, embed_truetype(true)).unwrap();
    FontList::index(&[path], &NoProgress).unwrap()
}

fn page_of(doc: &Document) -> TextPage {
    doc.text_page(1, &NoProgress).unwrap()
}

#[test]
fn new_text_is_set_in_the_blocks_font_where_the_block_began() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj 0 -100 Td (World) Tj ET");
    let doc = fixture.open();
    let before = page_of(&doc);
    assert_eq!(before.blocks.len(), 2);
    let edited = edit_text(
        &doc,
        1,
        &before.blocks[0],
        &TextEdit::new("Howdy"),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(edited.lines, 1);
    // The first glyph's text matrix is a move to (100, 700); the new text
    // is in a q ... Q of its own after the ET with the block's parameters.
    assert_eq!(
        content(&edited.document),
        "BT /F1 10 Tf 100 700 Td [-3000] TJ 0 -100 Td (World) Tj ET \
         q 0 g BT /F1 10 Tf 0 Tc 0 Tw 100 Tz 0 Tr 0 Ts 1 0 0 1 100 700 Tm (Howdy) Tj ET Q"
    );
    let after = page_of(&edited.document);
    assert_eq!(after.blocks.len(), 2);
    let texts: Vec<String> = after.blocks.iter().map(|b| b.lines[0].text()).collect();
    assert!(texts.contains(&"Howdy".to_owned()));
    assert!(texts.contains(&"World".to_owned()));
    // The block that was not edited has not moved.
    let world = |page: &TextPage| {
        page.blocks
            .iter()
            .find(|b| b.lines[0].text() == "World")
            .unwrap()
            .rect
    };
    assert_eq!(world(&before), world(&after));
    // Howdy is where Hello was.
    let howdy = after.blocks.iter().find(|b| b.lines[0].text() == "Howdy");
    assert_eq!(howdy.unwrap().rect, before.blocks[0].rect);
}

#[test]
fn a_word_that_passes_the_width_of_the_block_starts_the_next_line() {
    // The block is 5 * 6 = 30 points wide. "ab cd" is 30 and fits, "ab cde"
    // is 36 and breaks after "ab". The next line is 10 points lower.
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let fits = edit_text(&doc, 1, &block, &TextEdit::new("ab cd"), &NoProgress).unwrap();
    assert_eq!(fits.lines, 1);
    let broken = edit_text(&doc, 1, &block, &TextEdit::new("ab cde"), &NoProgress).unwrap();
    assert_eq!(broken.lines, 2);
    assert!(content(&broken.document)
        .ends_with("1 0 0 1 100 700 Tm (ab) Tj 1 0 0 1 100 690 Tm (cde) Tj ET Q"));
}

#[test]
fn a_line_feed_ends_a_line() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(&doc, 1, &block, &TextEdit::new("a\nb"), &NoProgress).unwrap();
    assert_eq!(edited.lines, 2);
    assert!(content(&edited.document)
        .ends_with("1 0 0 1 100 700 Tm (a) Tj 1 0 0 1 100 690 Tm (b) Tj ET Q"));
}

#[test]
fn every_hard_break_ends_a_line_without_changing_the_font_chosen() {
    // Courier in WinAnsiEncoding has no code for a carriage return or for
    // U+2028, and none is drawn: they end the line (UAX #14, LB4 and LB5),
    // so the block's font still shows the text and no font list is needed.
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    for text in ["ab\r\ncd", "ab\rcd", "ab\u{2028}cd", "ab\u{000B}cd"] {
        let edited = edit_text(&doc, 1, &block, &TextEdit::new(text), &NoProgress).unwrap();
        assert_eq!(edited.lines, 2, "{text:?}");
        assert!(
            content(&edited.document)
                .ends_with("1 0 0 1 100 700 Tm (ab) Tj 1 0 0 1 100 690 Tm (cd) Tj ET Q"),
            "{text:?}"
        );
    }
}

#[test]
fn a_soft_hyphen_is_not_shown_and_does_not_change_the_font_chosen() {
    // The soft hyphen is dropped from the text, so "abcd" is set in the
    // block's font: 4 glyphs, 24 points, one line of a 30 point block.
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(&doc, 1, &block, &TextEdit::new("ab\u{00AD}cd"), &NoProgress).unwrap();
    assert_eq!(edited.lines, 1);
    assert!(content(&edited.document).ends_with("1 0 0 1 100 700 Tm (abcd) Tj ET Q"));
}

#[test]
fn lines_are_as_far_apart_as_the_blocks_first_two() {
    // 14 points between the baselines of Hello and World.
    let fixture = one_page("BT /F1 10 Tf 14 TL 100 700 Td (Hello) Tj T* (World) Tj ET");
    let doc = fixture.open();
    let before = page_of(&doc);
    assert_eq!(before.blocks.len(), 1);
    assert_eq!(before.blocks[0].lines.len(), 2);
    let edited = edit_text(
        &doc,
        1,
        &before.blocks[0],
        &TextEdit::new("ab cd ef"),
        &NoProgress,
    )
    .unwrap();
    // "ab cd" is 30 points, the width of the block, and "ef" follows.
    assert_eq!(edited.lines, 2);
    assert!(content(&edited.document)
        .ends_with("1 0 0 1 100 700 Tm (ab cd) Tj 1 0 0 1 100 686 Tm (ef) Tj ET Q"));
}

#[test]
fn a_colour_in_the_style_sets_the_fill() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let style = TextStyle::default().with_colour(Rgb {
        r: 1.0,
        g: 0.0,
        b: 0.0,
    });
    let edited = edit_text(
        &doc,
        1,
        &block,
        &TextEdit::new("Hi").with_style(style),
        &NoProgress,
    )
    .unwrap();
    assert!(content(&edited.document).contains(" q 1 0 0 rg BT "));
}

#[test]
fn empty_text_deletes_the_block() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(&doc, 1, &block, &TextEdit::new(""), &NoProgress).unwrap();
    assert_eq!(edited.lines, 0);
    assert_eq!(
        content(&edited.document),
        "BT /F1 10 Tf 100 700 Td [-3000] TJ ET"
    );
}

#[test]
fn a_font_from_the_list_is_embedded_when_the_block_s_cannot_show_the_text() {
    // Courier's WinAnsi encoding has no Cyrillic: the text needs a face.
    // The test font maps A, B, the space and l, so only the Latin text it
    // has can be set; a character it lacks is an error, not a hole.
    let dir = tempfile::tempdir().unwrap();
    let list = font_list(&dir);
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let style = TextStyle::default().with_family("Test Sans");
    let edit = TextEdit::new("AB A").with_style(style).with_fonts(&list);
    let edited = edit_text(&doc, 1, &block, &edit, &NoProgress).unwrap();
    // A is glyph 1, B glyph 2, the space glyph 3: two bytes a character.
    let shown = content(&edited.document);
    assert!(shown.contains("/KernFont1 10 Tf"), "{shown}");
    assert!(
        shown.contains("(\0\u{1}\0\u{2}\0\u{3}\0\u{1}) Tj"),
        "{shown:?}"
    );
    // The page has its own resources now: the Pages node is as it was.
    let pages = edited
        .document
        .required(ObjectId {
            number: 2,
            generation: 0,
        })
        .unwrap();
    let original = doc
        .required(ObjectId {
            number: 2,
            generation: 0,
        })
        .unwrap();
    assert_eq!(pages.dictionary(), original.dictionary());
    // The font is read back, and the new text is searchable.
    let after = page_of(&edited.document);
    assert_eq!(after.text().trim(), "AB A");
    // 7 + 4.5 + 2.5 + 7 = 21 points wide, one line.
    assert_eq!(edited.lines, 1);
    let page = edited
        .document
        .required(ObjectId {
            number: PAGE,
            generation: 0,
        })
        .unwrap();
    let resources = page.dictionary().unwrap().get(b"Resources").unwrap();
    let Value::Dictionary(resources) = resources.value() else {
        panic!("the page has no resources of its own");
    };
    let Some(Value::Dictionary(fonts)) = resources.get(b"Font").map(|o| o.value()) else {
        panic!("no fonts");
    };
    assert!(fonts.get(b"F1").is_some() && fonts.get(b"KernFont1").is_some());
}

#[test]
fn a_character_no_font_has_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let list = font_list(&dir);
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let style = TextStyle::default().with_family("Test Sans");
    let edit = TextEdit::new("AZ").with_style(style).with_fonts(&list);
    match edit_text(&doc, 1, &block, &edit, &NoProgress) {
        Err(PdfError::NoGlyphForCharacter { character }) => assert_eq!(character, 'Z'),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn a_font_from_the_list_is_required_when_the_blocks_cannot_show_the_text() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    // U+0416 is in no Latin encoding.
    match edit_text(&doc, 1, &block, &TextEdit::new("\u{416}"), &NoProgress) {
        Err(PdfError::FontListRequired) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn a_page_that_ends_a_text_object_late_is_refused() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    assert_eq!(
        doc.text_block_editability(1, &block, &NoProgress).unwrap(),
        Editability::Blocked(EditBlocker::UnterminatedTextObject)
    );
    match edit_text(&doc, 1, &block, &TextEdit::new("x"), &NoProgress) {
        Err(PdfError::TextNotEditable { reason }) => {
            assert_eq!(reason, EditBlocker::UnterminatedTextObject)
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
    // Taking the text out needs no ET.
    assert!(edit_text(&doc, 1, &block, &TextEdit::new(""), &NoProgress).is_ok());
}

#[test]
fn a_transparency_change_before_the_et_is_refused() {
    let fixture = build(
        "BT /F1 10 Tf 100 700 Td (Hello) Tj /GS1 gs ET",
        "",
        "/Resources << /Font << /F1 60 0 R >> /ExtGState << /GS1 << /ca 0.5 >> >> >>",
        None,
        &[],
    );
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    assert_eq!(
        doc.text_block_editability(1, &block, &NoProgress).unwrap(),
        Editability::Blocked(EditBlocker::StateChangedBeforeEnd)
    );
}

#[test]
fn new_text_stays_in_the_marked_content_of_the_block() {
    let fixture = one_page("/P << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Hello) Tj ET EMC");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(&doc, 1, &block, &TextEdit::new("Hi"), &NoProgress).unwrap();
    assert!(content(&edited.document).ends_with("Tm (Hi) Tj ET Q EMC"));
}

#[test]
fn a_content_stream_two_pages_share_is_not_changed_for_the_other() {
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (
            2,
            b"<< /Type /Pages /Kids [10 0 R 12 0 R] /Count 2 /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 60 0 R >> >> >>"
                .to_vec(),
        ),
        (3, b"<< /Producer (kernkraft tests) >>".to_vec()),
        (
            10,
            b"<< /Type /Page /Parent 2 0 R /Contents 11 0 R >>".to_vec(),
        ),
        (
            12,
            b"<< /Type /Page /Parent 2 0 R /Contents 11 0 R >>".to_vec(),
        ),
        (11, stream("", b"BT /F1 10 Tf 100 700 Td (Hello) Tj ET")),
        (60, COURIER.to_vec()),
    ];
    let fixture = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(&doc, 1, &block, &TextEdit::new("Hi"), &NoProgress).unwrap();
    assert_eq!(page_of(&edited.document).text().trim(), "Hi");
    let other = edited.document.text_page(2, &NoProgress).unwrap();
    assert_eq!(other.text().trim(), "Hello");
}

/// A page that shows `AB` in a simple TrueType font whose embedded program
/// is `embed_truetype(false)`: it has marks for `A` and `B` and none for
/// `l`.
fn embedded_simple_font_page() -> Fixture {
    build(
        "BT /F2 10 Tf 100 700 Td (AB) Tj ET",
        "/F2 70 0 R",
        "",
        None,
        &[
            (
                70,
                b"<< /Type /Font /Subtype /TrueType /BaseFont /TestSans-Regular \
                  /FirstChar 65 /LastChar 66 /Widths [700 450] \
                  /Encoding /WinAnsiEncoding /FontDescriptor 71 0 R >>"
                    .as_slice(),
            ),
            (
                71,
                b"<< /Type /FontDescriptor /FontName /TestSans-Regular /Flags 32 \
                  /FontBBox [0 0 700 700] /ItalicAngle 0 /Ascent 800 /Descent -200 \
                  /CapHeight 700 /StemV 80 /FontFile2 72 0 R >>"
                    .as_slice(),
            ),
            (72, &stream("/Length1 0", &embed_truetype(false))),
        ],
    )
}

#[test]
fn an_embedded_font_is_used_again_only_for_glyphs_its_program_has() {
    let fixture = embedded_simple_font_page();
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    assert_eq!(block.lines[0].text(), "AB");
    // B and A have outlines in the program: the font on the page is used.
    let edited = edit_text(&doc, 1, &block, &TextEdit::new("BA"), &NoProgress).unwrap();
    assert!(
        content(&edited.document).contains("/F2 10 Tf"),
        "{}",
        content(&edited.document)
    );
    assert!(content(&edited.document).ends_with("Tm (BA) Tj ET Q"));
    // The program has no outline for `l`, though the encoding gives it a
    // code: the page's font would draw nothing, so a face must come from
    // a list.
    match edit_text(&doc, 1, &block, &TextEdit::new("Al"), &NoProgress) {
        Err(PdfError::FontListRequired) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
}

fn geometry(block: &kernkraft_pdf::text::TextBlock) -> Vec<kernkraft_pdf::Rect> {
    let mut rects = vec![block.rect];
    rects.extend(block.lines.iter().map(|line| line.rect));
    rects
}

#[test]
fn a_block_of_several_text_objects_is_set_after_the_last() {
    // One BT ... ET a line, as an exporter writes them: two text objects,
    // one block of two lines 14 points apart.
    let fixture =
        one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET BT /F1 10 Tf 100 686 Td (World) Tj ET");
    let doc = fixture.open();
    let before = page_of(&doc);
    assert_eq!(before.blocks.len(), 1);
    let edited = edit_text(
        &doc,
        1,
        &before.blocks[0],
        &TextEdit::new("ab cd ef"),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(
        content(&edited.document),
        "BT /F1 10 Tf 100 700 Td [-3000] TJ ET BT /F1 10 Tf 100 686 Td [-3000] TJ ET \
         q 0 g BT /F1 10 Tf 0 Tc 0 Tw 100 Tz 0 Tr 0 Ts 1 0 0 1 100 700 Tm (ab cd) Tj \
         1 0 0 1 100 686 Tm (ef) Tj ET Q"
    );
}

#[test]
fn a_cm_between_the_text_objects_does_not_move_the_new_text() {
    // The second line is drawn under a translation of -14, so the new text,
    // which goes after its ET and under that translation, must undo it.
    let fixture = one_page(
        "BT /F1 10 Tf 100 700 Td (Hello) Tj ET \
         q 1 0 0 1 0 -14 cm BT /F1 10 Tf 100 700 Td (World) Tj ET Q",
    );
    let doc = fixture.open();
    let before = page_of(&doc);
    assert_eq!(before.blocks.len(), 1);
    let edited = edit_text(
        &doc,
        1,
        &before.blocks[0],
        &TextEdit::new("Howdy\nWorld"),
        &NoProgress,
    )
    .unwrap();
    assert!(content(&edited.document).contains("q 1 0 0 1 0 14 cm 0 g BT "));
    let after = page_of(&edited.document);
    assert_eq!(after.blocks.len(), 1);
    assert_eq!(geometry(&after.blocks[0]), geometry(&before.blocks[0]));
}

#[test]
fn a_clip_between_the_text_objects_blocks_the_edit() {
    let fixture = one_page(
        "BT /F1 10 Tf 100 700 Td (Hello) Tj ET 0 0 300 600 re W n \
         BT /F1 10 Tf 100 686 Td (World) Tj ET",
    );
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    assert_eq!(
        doc.text_block_editability(1, &block, &NoProgress).unwrap(),
        Editability::Blocked(EditBlocker::StateChangedBeforeEnd)
    );
}

#[test]
fn tagged_text_in_a_form_blocks_the_edit() {
    let fixture = build(
        "/Fm1 Do",
        "",
        "/Resources << /XObject << /Fm1 70 0 R >> /Font << /F1 60 0 R >> >>",
        None,
        &[(
            70,
            &stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /Font << /F1 60 0 R >> >>",
                b"/P << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Hello) Tj ET EMC",
            ),
        )],
    );
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    assert_eq!(
        doc.text_block_editability(1, &block, &NoProgress).unwrap(),
        Editability::Blocked(EditBlocker::TaggedFormContent)
    );
}

#[test]
fn a_code_the_font_states_no_width_for_is_not_used() {
    // Widths cover codes 65 and 66 only; the font is not one of the
    // standard 14, so any other code would take MissingWidth (0) and the
    // glyph would overlap its neighbour.
    let fixture = build(
        "BT /F2 10 Tf 100 700 Td (AB) Tj ET",
        "/F2 70 0 R",
        "",
        None,
        &[(
            70,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /ArialMT /FirstChar 65 /LastChar 66 \
              /Widths [700 450] /Encoding /WinAnsiEncoding >>"
                .as_slice(),
        )],
    );
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    assert!(edit_text(&doc, 1, &block, &TextEdit::new("BA"), &NoProgress).is_ok());
    match edit_text(&doc, 1, &block, &TextEdit::new("ABC"), &NoProgress) {
        Err(PdfError::FontListRequired) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// A font like the test font under another name.
fn other_sans() -> Vec<u8> {
    use fontkit::sfnt::{self, Glyph, SfntSpec};
    let rectangle = |width: i16, height: i16| {
        vec![vec![
            (0, 0, true),
            (width, 0, true),
            (width, height, true),
            (0, height, true),
        ]]
    };
    let glyph = |name: &'static str, advance: u16, contours| Glyph {
        name,
        advance,
        contours,
    };
    let mut spec = SfntSpec::new(vec![
        glyph(".notdef", 500, vec![]),
        glyph("A", 700, rectangle(500, 700)),
        glyph("B", 450, rectangle(200, 200)),
        glyph("space", 250, vec![]),
    ]);
    spec.unicode = vec![(0x41, 1), (0x42, 2), (0x20, 3)];
    spec.family = "Other Sans";
    spec.full = "Other Sans Regular";
    spec.postscript = "OtherSans-Regular";
    sfnt::build(&spec)
}

/// A font of the space and the Cyrillic letter U+0416, which Courier's
/// WinAnsi encoding cannot show.
fn cyrillic_sans() -> Vec<u8> {
    use fontkit::sfnt::{self, Glyph, SfntSpec};
    let glyph = |name: &'static str, advance: u16, contours| Glyph {
        name,
        advance,
        contours,
    };
    let square = vec![vec![
        (0, 0, true),
        (500, 0, true),
        (500, 500, true),
        (0, 500, true),
    ]];
    let mut spec = SfntSpec::new(vec![
        glyph(".notdef", 500, vec![]),
        glyph("uni0416", 600, square),
        glyph("space", 250, vec![]),
    ]);
    spec.unicode = vec![(0x416, 1), (0x20, 2)];
    spec.family = "Cyrillic Sans";
    spec.full = "Cyrillic Sans Regular";
    spec.postscript = "CyrillicSans-Regular";
    sfnt::build(&spec)
}

pub(super) fn cyrillic_list(dir: &tempfile::TempDir) -> FontList {
    let path = dir.path().join("cyrillic-sans.ttf");
    std::fs::write(&path, cyrillic_sans()).unwrap();
    FontList::index(&[path], &NoProgress).unwrap()
}

/// The substitutions of `doc`'s log.
fn logged_substitutions(doc: &Document) -> usize {
    doc.diagnostics()
        .iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::FontSubstituted { .. }))
        .count()
}

#[test]
fn an_edit_that_needs_a_face_of_the_list_reports_the_font_it_replaced() {
    // Tf names the font by the page's resource name, `F1`; that is the name
    // the substitution carries (ISO 32000-1, 9.3.1).
    let dir = tempfile::tempdir().unwrap();
    let list = cyrillic_list(&dir);
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edit = TextEdit::new("\u{416}").with_fonts(&list);
    let edited = edit_text(&doc, 1, &block, &edit, &NoProgress).unwrap();
    assert_eq!(
        edited.substitutions,
        vec![FontSubstitution {
            name: Name::new(&b"F1"[..]),
            family: "Cyrillic Sans".to_owned(),
        }]
    );
}

#[test]
fn an_edit_in_the_blocks_own_font_reports_no_substitution() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(&doc, 1, &block, &TextEdit::new("Howdy"), &NoProgress).unwrap();
    assert!(edited.substitutions.is_empty());
    let deleted = edit_text(&doc, 1, &block, &TextEdit::new(""), &NoProgress).unwrap();
    assert!(deleted.substitutions.is_empty());
}

#[test]
fn a_family_the_style_names_is_a_choice_and_not_a_substitution() {
    let dir = tempfile::tempdir().unwrap();
    let list = font_list(&dir);
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let style = TextStyle::default().with_family("Test Sans");
    let edit = TextEdit::new("AB").with_style(style).with_fonts(&list);
    let edited = edit_text(&doc, 1, &block, &edit, &NoProgress).unwrap();
    assert!(edited.substitutions.is_empty());
}

#[test]
fn the_same_substitution_is_reported_by_each_edit_though_the_log_keeps_it_once() {
    let dir = tempfile::tempdir().unwrap();
    let list = cyrillic_list(&dir);
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edit = TextEdit::new("\u{416}").with_fonts(&list);
    // An undo and a new edit of the block start from the same document.
    let first = edit_text(&doc, 1, &block, &edit, &NoProgress).unwrap();
    let second = edit_text(&doc, 1, &block, &edit, &NoProgress).unwrap();
    assert_eq!(first.substitutions.len(), 1);
    assert_eq!(second.substitutions, first.substitutions);
    assert_eq!(logged_substitutions(&doc), 1);
}

#[test]
fn a_block_edited_twice_stays_in_the_face_it_was_given() {
    let dir = tempfile::tempdir().unwrap();
    // Other Sans sorts first, so a choice by anything but the name of the
    // font on the page would find it before Test Sans.
    let other = dir.path().join("a-other.ttf");
    let test = dir.path().join("b-test.ttf");
    std::fs::write(&other, other_sans()).unwrap();
    std::fs::write(&test, embed_truetype(true)).unwrap();
    let list = FontList::index(&[other, test], &NoProgress).unwrap();
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let first = edit_text(
        &doc,
        1,
        &block,
        &TextEdit::new("AB")
            .with_style(TextStyle::default().with_family("Test Sans"))
            .with_fonts(&list),
        &NoProgress,
    )
    .unwrap();
    let block = page_of(&first.document).blocks.remove(0);
    let second = edit_text(
        &first.document,
        1,
        &block,
        &TextEdit::new("BA").with_fonts(&list),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(page_of(&second.document).text().trim(), "BA");
    let page = second
        .document
        .required(ObjectId {
            number: PAGE,
            generation: 0,
        })
        .unwrap();
    let Some(Value::Dictionary(resources)) = page
        .dictionary()
        .unwrap()
        .get(b"Resources")
        .map(|o| o.value())
    else {
        panic!("no resources");
    };
    let Some(Value::Dictionary(fonts)) = resources.get(b"Font").map(|o| o.value()) else {
        panic!("no fonts");
    };
    let Some(Value::Reference(font)) = fonts.get(b"KernFont2").map(|o| o.value()) else {
        panic!("the second edit added no font");
    };
    let font = second.document.required(*font).unwrap();
    let Some(Value::Name(base)) = font
        .dictionary()
        .unwrap()
        .get(b"BaseFont")
        .map(|o| o.value())
    else {
        panic!("no BaseFont");
    };
    assert!(
        String::from_utf8_lossy(base.as_bytes()).ends_with("+TestSans-Regular"),
        "{base:?}"
    );
}

/// "Test Sans" as a variable font with a weight axis from 400 to 900 and the
/// named instance Bold at 700, with glyphs for `A` and `B`.
fn variable_test_sans() -> Vec<u8> {
    use fontkit::sfnt::{self, Glyph, Instance, SfntSpec, Variable};
    const BOLD_NAME: u16 = 256;
    let square = |size: i16| {
        vec![vec![
            (0, 0, true),
            (size, 0, true),
            (size, size, true),
            (0, size, true),
        ]]
    };
    let glyph = |name: &'static str, contours| Glyph {
        name,
        advance: 600,
        contours,
    };
    let mut spec = SfntSpec::new(vec![
        glyph(".notdef", vec![]),
        glyph("A", square(500)),
        glyph("B", square(200)),
    ]);
    spec.unicode = vec![(0x41, 1), (0x42, 2)];
    spec.extra_names = vec![(BOLD_NAME, "Bold")];
    spec.variable = Some(Variable {
        tag: *b"wght",
        min: 400,
        default: 400,
        max: 900,
        instances: vec![Instance {
            name_id: BOLD_NAME,
            value: 700,
            postscript_name_id: None,
        }],
        moves: vec![None, None, None],
    });
    sfnt::build(&spec)
}

/// The `BaseFont` of the font `name` of the page's resources.
fn base_font_of(doc: &Document, name: &[u8]) -> String {
    let page = doc
        .required(ObjectId {
            number: PAGE,
            generation: 0,
        })
        .unwrap();
    let Some(Value::Dictionary(resources)) = page
        .dictionary()
        .unwrap()
        .get(b"Resources")
        .map(|o| o.value())
    else {
        panic!("no resources");
    };
    let Some(Value::Dictionary(fonts)) = resources.get(b"Font").map(|o| o.value()) else {
        panic!("no fonts");
    };
    let Some(Value::Reference(font)) = fonts.get(name).map(|o| o.value()) else {
        panic!("no font {name:?}");
    };
    let font = doc.required(*font).unwrap();
    let Some(Value::Name(base)) = font
        .dictionary()
        .unwrap()
        .get(b"BaseFont")
        .map(|o| o.value())
    else {
        panic!("no BaseFont");
    };
    String::from_utf8_lossy(base.as_bytes()).into_owned()
}

#[test]
fn a_bold_block_is_set_in_the_bold_instance_of_a_variable_face() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test-sans-variable.ttf");
    std::fs::write(&path, variable_test_sans()).unwrap();
    let list = FontList::index(&[path], &NoProgress).unwrap();
    assert_eq!(list.faces().len(), 2);
    let bold = b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier-Bold \
                 /Encoding /WinAnsiEncoding >>";
    let fixture = build(
        "BT /F2 10 Tf 100 700 Td (Hello) Tj ET",
        "/F2 61 0 R",
        "",
        None,
        &[(61, bold)],
    );
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(
        &doc,
        1,
        &block,
        &TextEdit::new("AB")
            .with_style(TextStyle::default().with_family("Test Sans"))
            .with_fonts(&list),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(page_of(&edited.document).text().trim(), "AB");
    let base = base_font_of(&edited.document, b"KernFont1");
    assert!(base.ends_with("+TestSans-Bold"), "{base}");
}

#[test]
fn a_regular_block_is_set_in_the_default_face_of_a_variable_font() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test-sans-variable.ttf");
    std::fs::write(&path, variable_test_sans()).unwrap();
    let list = FontList::index(&[path], &NoProgress).unwrap();
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = page_of(&doc).blocks.remove(0);
    let edited = edit_text(
        &doc,
        1,
        &block,
        &TextEdit::new("AB")
            .with_style(TextStyle::default().with_family("Test Sans"))
            .with_fonts(&list),
        &NoProgress,
    )
    .unwrap();
    let base = base_font_of(&edited.document, b"KernFont1");
    assert!(base.ends_with("+TestSans-Regular"), "{base}");
}
