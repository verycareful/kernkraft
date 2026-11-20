// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Adding, editing and deleting annotations: ISO 32000-1:2008, 12.5.2
//! (Tables 164 and 165), 12.5.5 (`AP`), 12.5.6.4 (Table 172), 12.5.6.10
//! (Table 179), 12.5.6.13 (Table 184), 12.5.2 (`IRT`, `Popup`) and 14.7.4.3
//! (`OBJR`: a structure element's reference to an annotation).
//!
//! An annotation is listed in its page's `Annots` (a direct array, or one
//! held in an object of its own) and has an appearance stream, a form
//! XObject, under `AP`'s `N`. Adding writes the annotation, its appearance
//! and the page's new `Annots`; editing writes the annotation again, with
//! a new appearance when the edit changes what it shows; deleting takes
//! the annotation out of `Annots` and out of the structure tree. The old
//! objects stay in the file as they were: an incremental save appends to
//! it, and a compacted copy drops what nothing reaches.

use std::collections::HashSet;

use crate::colour::Rgb;
use crate::diagnostic::Diagnostics;
use crate::doc::{encode_text_string, Date, Document};
use crate::pages::references::Gone;
use crate::pages::rewrite::{reference, Rewriter};
use crate::pages::structure;
use crate::syntax::{Dictionary, Name, Object, ObjectId, PdfString, Value};
use crate::{PdfError, Rect};

use super::appearance::{self, Appearance, Look, LINE_WIDTH, NOTE_SIZE};
use super::geometry::{bounds, grown, Geometry, Point, Quad};
use super::read::{
    parse, Parsed, ANNOTS_KEY, INK_LIST_KEY, QUAD_POINTS_KEY, RECT_KEY, SUBTYPE_KEY,
};
use super::{
    Added, AnnotationEdit, AnnotationKind, NewAnnotation, NewShape, FLAG_NO_ROTATE, FLAG_NO_ZOOM,
    FLAG_PRINT,
};

const TYPE_KEY: &[u8] = b"Type";
const ANNOT_TYPE: &[u8] = b"Annot";
const PAGE_KEY: &[u8] = b"P";
const FLAGS_KEY: &[u8] = b"F";
const COLOUR_KEY: &[u8] = b"C";
const OPACITY_KEY: &[u8] = b"CA";
const AUTHOR_KEY: &[u8] = b"T";
const MODIFIED_KEY: &[u8] = b"M";
const CONTENTS_KEY: &[u8] = b"Contents";
const NAME_KEY: &[u8] = b"Name";
const NOTE_ICON: &[u8] = b"Note";
const BORDER_STYLE_KEY: &[u8] = b"BS";
const WIDTH_KEY: &[u8] = b"W";
const APPEARANCE_KEY: &[u8] = b"AP";
const NORMAL_KEY: &[u8] = b"N";
const REPLY_TO_KEY: &[u8] = b"IRT";
const POPUP_KEY: &[u8] = b"Popup";
const WIDGET: &[u8] = b"Widget";

/// An annotation about to be written, apart from the entries every one has.
struct Built {
    subtype: &'static [u8],
    rect: [f64; 4],
    flags: i64,
    /// The entries of this kind (`QuadPoints`, `InkList`, `BS`, `Name`).
    entries: Vec<(&'static [u8], Object)>,
    appearance: Appearance,
}

fn direct(value: Value) -> Object {
    Object::new(value)
}

fn name_object(bytes: &[u8]) -> Object {
    direct(Value::Name(Name::new(bytes)))
}

fn text_object(text: &str) -> Object {
    direct(Value::String(PdfString::new(encode_text_string(text))))
}

fn number_object(value: f64) -> Result<Object, PdfError> {
    crate::build::number(value)
}

fn numbers_object(values: impl IntoIterator<Item = f64>) -> Result<Object, PdfError> {
    let items = values
        .into_iter()
        .map(number_object)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(direct(Value::Array(items)))
}

fn insert(dictionary: &mut Dictionary, key: &[u8], value: Object) {
    dictionary.insert(Name::new(key), value);
}

fn invalid(message: String) -> PdfError {
    PdfError::Invalid(message)
}

fn check_unit(value: f64, what: &str) -> Result<(), PdfError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        return Ok(());
    }
    Err(invalid(format!(
        "the {what} {value} is not between 0 and 1"
    )))
}

fn check_colour(colour: &Rgb) -> Result<(), PdfError> {
    check_unit(colour.r, "red")?;
    check_unit(colour.g, "green")?;
    check_unit(colour.b, "blue")
}

fn check_point((x, y): Point) -> Result<(), PdfError> {
    if x.is_finite() && y.is_finite() {
        return Ok(());
    }
    Err(invalid(format!("the point ({x}, {y}) is not finite")))
}

fn check_quads(quads: &[Rect]) -> Result<(), PdfError> {
    if quads.is_empty() {
        return Err(invalid("a text markup has no rectangle".to_string()));
    }
    for rect in quads {
        check_point((rect.left, rect.top))?;
        check_point((rect.right, rect.bottom))?;
    }
    Ok(())
}

fn check_width(width: f64) -> Result<(), PdfError> {
    if width.is_finite() && width > 0.0 {
        return Ok(());
    }
    Err(invalid(format!("the line width {width} is not above 0")))
}

fn check_new(new: &NewAnnotation) -> Result<(), PdfError> {
    check_colour(&new.colour)?;
    check_unit(new.opacity, "opacity")?;
    match &new.shape {
        NewShape::Highlight { quads }
        | NewShape::Underline { quads }
        | NewShape::StrikeOut { quads } => check_quads(quads),
        NewShape::Ink { strokes, width } => {
            check_width(*width)?;
            if strokes.is_empty() || strokes.iter().any(Vec::is_empty) {
                return Err(invalid("an ink stroke has no point".to_string()));
            }
            strokes
                .iter()
                .flatten()
                .try_for_each(|&point| check_point(point))
        }
        NewShape::Note { at } => check_point(*at),
    }
}

fn check_edit(edit: &AnnotationEdit) -> Result<(), PdfError> {
    if let Some(colour) = &edit.colour {
        check_colour(colour)?;
    }
    if let Some(opacity) = edit.opacity {
        check_unit(opacity, "opacity")?;
    }
    if let Some(width) = edit.line_width {
        check_width(width)?;
    }
    if let Some(by) = edit.translate {
        check_point(by)?;
    }
    Ok(())
}

fn flatten(quads: &[Quad]) -> impl Iterator<Item = f64> + '_ {
    quads.iter().flatten().flat_map(|&(x, y)| [x, y])
}

fn rect_of(quads: &[Quad], strokes: &[Vec<Point>]) -> Option<[f64; 4]> {
    bounds(
        quads
            .iter()
            .flatten()
            .chain(strokes.iter().flatten())
            .copied(),
    )
}

/// What `shape` is, in user space.
fn build(geometry: &Geometry, shape: &NewShape, look: &Look) -> Result<Built, PdfError> {
    let user_quads = |quads: &[Rect]| -> Vec<Quad> {
        quads.iter().map(|rect| geometry.user_quad(rect)).collect()
    };
    let no_bounds = || invalid("the annotation has no area".to_string());
    match shape {
        NewShape::Highlight { quads }
        | NewShape::Underline { quads }
        | NewShape::StrikeOut { quads } => {
            let quads = user_quads(quads);
            let is_highlight = matches!(shape, NewShape::Highlight { .. });
            let mut rect = rect_of(&quads, &[]).ok_or_else(no_bounds)?;
            if !is_highlight {
                // A line reaches half its width past its path and a
                // renderer paints a soft edge there; a box one whole width
                // larger than the path holds both, where one that ends at
                // the path clips the edge.
                rect = grown(rect, LINE_WIDTH);
            }
            let (subtype, appearance): (&'static [u8], _) = match shape {
                NewShape::Highlight { .. } => {
                    (b"Highlight", appearance::highlight(&quads, rect, look)?)
                }
                NewShape::Underline { .. } => (
                    b"Underline",
                    appearance::line_markup(&quads, false, rect, look)?,
                ),
                _ => (
                    b"StrikeOut",
                    appearance::line_markup(&quads, true, rect, look)?,
                ),
            };
            Ok(Built {
                subtype,
                rect,
                flags: FLAG_PRINT,
                entries: vec![(QUAD_POINTS_KEY, numbers_object(flatten(&quads))?)],
                appearance,
            })
        }
        NewShape::Ink { strokes, width } => {
            let strokes: Vec<Vec<Point>> = strokes
                .iter()
                .map(|stroke| stroke.iter().map(|&p| geometry.user_point(p)).collect())
                .collect();
            // As for a line: a whole width of margin holds the stroke and
            // its soft edge.
            let rect = grown(rect_of(&[], &strokes).ok_or_else(no_bounds)?, *width);
            Ok(Built {
                subtype: b"Ink",
                rect,
                flags: FLAG_PRINT,
                entries: vec![
                    (INK_LIST_KEY, ink_list(&strokes)?),
                    (BORDER_STYLE_KEY, border_style(*width)?),
                ],
                appearance: appearance::ink(&strokes, *width, rect, look)?,
            })
        }
        NewShape::Note { at } => {
            let (x, y) = geometry.user_point(*at);
            let rect = [x, y - NOTE_SIZE, x + NOTE_SIZE, y];
            Ok(Built {
                subtype: b"Text",
                rect,
                flags: FLAG_PRINT | FLAG_NO_ZOOM | FLAG_NO_ROTATE,
                entries: vec![(NAME_KEY, name_object(NOTE_ICON))],
                appearance: appearance::note(rect, look)?,
            })
        }
    }
}

fn ink_list(strokes: &[Vec<Point>]) -> Result<Object, PdfError> {
    let items = strokes
        .iter()
        .map(|stroke| numbers_object(stroke.iter().flat_map(|&(x, y)| [x, y])))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(direct(Value::Array(items)))
}

/// `BS` (Table 166) with a width.
fn border_style(width: f64) -> Result<Object, PdfError> {
    let mut style = Dictionary::new();
    insert(&mut style, WIDTH_KEY, number_object(width)?);
    Ok(direct(Value::Dictionary(style)))
}

/// Writes `appearance` as a new stream and gives its object.
fn add_appearance(
    rewriter: &mut Rewriter<'_>,
    appearance: Appearance,
) -> Result<ObjectId, PdfError> {
    let id = ObjectId {
        number: rewriter.reserve_many(1)?,
        generation: 0,
    };
    rewriter.set_stream(id, appearance.dictionary, appearance.data)?;
    Ok(id)
}

/// `AP` with `N` the stream `normal`, keeping what else `existing` has.
fn appearance_dictionary(existing: Option<Dictionary>, normal: ObjectId) -> Object {
    let mut dictionary = existing.unwrap_or_default();
    insert(
        &mut dictionary,
        NORMAL_KEY,
        direct(Value::Reference(normal)),
    );
    direct(Value::Dictionary(dictionary))
}

fn set_colour(dictionary: &mut Dictionary, colour: &Rgb) -> Result<(), PdfError> {
    insert(
        dictionary,
        COLOUR_KEY,
        numbers_object([colour.r, colour.g, colour.b])?,
    );
    Ok(())
}

/// `CA`: left out at 1, its default (Table 164).
fn set_opacity(dictionary: &mut Dictionary, opacity: f64) -> Result<(), PdfError> {
    if opacity >= 1.0 {
        dictionary.remove(OPACITY_KEY);
    } else {
        insert(dictionary, OPACITY_KEY, number_object(opacity)?);
    }
    Ok(())
}

/// Sets or, for an empty text, removes the text string `key`.
fn set_text(dictionary: &mut Dictionary, key: &[u8], text: &str) {
    if text.is_empty() {
        dictionary.remove(key);
    } else {
        insert(dictionary, key, text_object(text));
    }
}

fn set_modified(dictionary: &mut Dictionary, modified: &Date) {
    insert(
        dictionary,
        MODIFIED_KEY,
        direct(Value::String(PdfString::new(modified.format().as_bytes()))),
    );
}

/// `document` with the annotation `new` on page `page`, counted from 1:
/// the annotation, its appearance stream, and the page's `Annots` with it
/// last, so that it is drawn over what is there.
///
/// Geometry is as fractions of the displayed page. A text markup's
/// rectangles are those of a text selection
/// ([`Selection::rects`](crate::text::Selection)). A colour or opacity out
/// of 0 to 1, a point that is not a number, and a shape with nothing to
/// draw are [`PdfError::Invalid`].
pub fn add_annotation(
    document: &Document,
    page: u32,
    new: &NewAnnotation,
) -> Result<Added, PdfError> {
    check_new(new)?;
    let page = document.page(page)?;
    let mut diagnostics = Diagnostics::new();
    diagnostics.set_object(Some(page.id));
    let geometry = Geometry::of(&page, &mut diagnostics)?;
    document.record_diagnostics(diagnostics);

    let look = Look {
        colour: new.colour,
        opacity: new.opacity,
    };
    let built = build(&geometry, &new.shape, &look)?;
    let mut rewriter = Rewriter::new(document);
    let normal = add_appearance(&mut rewriter, built.appearance)?;

    let mut dictionary = Dictionary::new();
    insert(&mut dictionary, TYPE_KEY, name_object(ANNOT_TYPE));
    insert(&mut dictionary, SUBTYPE_KEY, name_object(built.subtype));
    insert(&mut dictionary, RECT_KEY, numbers_object(built.rect)?);
    insert(&mut dictionary, PAGE_KEY, direct(Value::Reference(page.id)));
    insert(
        &mut dictionary,
        FLAGS_KEY,
        direct(Value::Integer(built.flags)),
    );
    set_colour(&mut dictionary, &new.colour)?;
    set_opacity(&mut dictionary, new.opacity)?;
    if let Some(author) = &new.author {
        set_text(&mut dictionary, AUTHOR_KEY, author);
    }
    set_modified(&mut dictionary, &new.modified);
    if let Some(contents) = &new.contents {
        set_text(&mut dictionary, CONTENTS_KEY, contents);
    }
    for (key, value) in built.entries {
        insert(&mut dictionary, key, value);
    }
    insert(
        &mut dictionary,
        APPEARANCE_KEY,
        appearance_dictionary(None, normal),
    );
    let id = rewriter.add(direct(Value::Dictionary(dictionary)))?;
    edit_annots(&mut rewriter, page.id, |items| {
        items.push(direct(Value::Reference(id)));
    })?;
    Ok(Added {
        document: document.apply(rewriter.finish())?,
        id,
    })
}

/// Changes the array of a page's `Annots`: the page's own, or the one
/// held in an object of its own. A page with none gets one when `change`
/// leaves it something.
fn edit_annots(
    rewriter: &mut Rewriter<'_>,
    page: ObjectId,
    change: impl FnOnce(&mut Vec<Object>),
) -> Result<(), PdfError> {
    let Some(dictionary) = rewriter.read_dictionary(page)? else {
        return Err(PdfError::MissingObject { object: page });
    };
    let invalid_entry = PdfError::InvalidEntry {
        object: Some(page),
        key: "Annots",
    };
    match dictionary.get(ANNOTS_KEY).map(Object::value) {
        None | Some(Value::Null) => {
            let mut items = Vec::new();
            change(&mut items);
            if items.is_empty() {
                return Ok(());
            }
            rewriter.edit(page, |_, dictionary| {
                insert(dictionary, ANNOTS_KEY, direct(Value::Array(items)));
                Ok(())
            })?;
            Ok(())
        }
        Some(Value::Array(_)) => {
            rewriter.edit(page, |_, dictionary| {
                if let Some(Value::Array(items)) =
                    dictionary.get_mut(ANNOTS_KEY).map(Object::value_mut)
                {
                    change(items);
                }
                Ok(())
            })?;
            Ok(())
        }
        Some(Value::Reference(array)) => {
            let array = *array;
            let Some(mut object) = rewriter.read(array)? else {
                return Err(invalid_entry);
            };
            let Value::Array(items) = object.value_mut() else {
                return Err(invalid_entry);
            };
            change(items);
            rewriter.set(array, object)
        }
        _ => Err(invalid_entry),
    }
}

/// The dictionary of annotation `id`, which must be listed by `page`.
fn annotation_dictionary(
    document: &Document,
    page: &crate::doc::Page,
    number: u32,
    id: ObjectId,
) -> Result<Dictionary, PdfError> {
    if !document.lists_annotation(&page.dictionary, id)? {
        return Err(PdfError::AnnotationNotOnPage {
            object: id,
            page: number,
        });
    }
    let object = document.required(id)?;
    match object.object().map(Object::value) {
        Some(Value::Dictionary(dictionary)) => Ok(dictionary.clone()),
        _ => Err(PdfError::WrongKind { object: id }),
    }
}

fn subtype_name(dictionary: &Dictionary) -> Option<&[u8]> {
    match dictionary.get(SUBTYPE_KEY).map(Object::value) {
        Some(Value::Name(name)) => Some(name.as_bytes()),
        _ => None,
    }
}

/// `document` with annotation `id` of page `page`, counted from 1,
/// changed as `edit` says, and `modified` as its `M`.
///
/// Contents and author can be changed on an annotation of any kind. A
/// change to what the annotation shows (colour, opacity, line width,
/// position) draws its appearance again, which replaces the one the file
/// had: that is [`PdfError::AnnotationNotEditable`] for a kind the library
/// does not write, and so is a line width on any but an ink annotation.
/// An annotation the page does not list is
/// [`PdfError::AnnotationNotOnPage`]. Every entry the edit does not name
/// stays as it was.
pub fn edit_annotation(
    document: &Document,
    page: u32,
    id: ObjectId,
    edit: &AnnotationEdit,
    modified: &Date,
) -> Result<Document, PdfError> {
    check_edit(edit)?;
    let page_number = page;
    let page = document.page(page_number)?;
    let mut dictionary = annotation_dictionary(document, &page, page_number, id)?;
    let mut diagnostics = Diagnostics::new();
    diagnostics.set_object(Some(id));
    let offset = document.offset_of(Some(id));
    let parsed =
        parse(document, &dictionary, &mut diagnostics, offset)?.ok_or(PdfError::InvalidEntry {
            object: Some(id),
            key: "Rect",
        })?;
    document.record_diagnostics(diagnostics);

    let redraws = edit.colour.is_some()
        || edit.opacity.is_some()
        || edit.line_width.is_some()
        || edit.translate.is_some();
    let subtype = || kind_name(&parsed.kind);
    if redraws && !parsed.kind.is_drawn_by_library() {
        return Err(PdfError::AnnotationNotEditable { subtype: subtype() });
    }
    if edit.line_width.is_some() && parsed.kind != AnnotationKind::Ink {
        return Err(PdfError::AnnotationNotEditable { subtype: subtype() });
    }

    let mut rewriter = Rewriter::new(document);
    if let Some(contents) = &edit.contents {
        set_text(&mut dictionary, CONTENTS_KEY, contents);
    }
    if let Some(author) = &edit.author {
        set_text(&mut dictionary, AUTHOR_KEY, author);
    }
    set_modified(&mut dictionary, modified);
    if redraws {
        redraw(
            document,
            &mut rewriter,
            &page,
            id,
            &mut dictionary,
            parsed,
            edit,
        )?;
    }
    rewriter.edit(id, |_, target| {
        *target = dictionary;
        Ok(())
    })?;
    document.apply(rewriter.finish())
}

fn kind_name(kind: &AnnotationKind) -> String {
    match kind {
        AnnotationKind::Highlight => "Highlight".to_string(),
        AnnotationKind::Underline => "Underline".to_string(),
        AnnotationKind::StrikeOut => "StrikeOut".to_string(),
        AnnotationKind::Ink => "Ink".to_string(),
        AnnotationKind::Note => "Text".to_string(),
        AnnotationKind::Other(name) => name.clone(),
    }
}

/// Applies the edit's changes to what the annotation shows, to
/// `dictionary`, and gives it an appearance drawn from the result.
fn redraw(
    document: &Document,
    rewriter: &mut Rewriter<'_>,
    page: &crate::doc::Page,
    id: ObjectId,
    dictionary: &mut Dictionary,
    mut parsed: Parsed,
    edit: &AnnotationEdit,
) -> Result<(), PdfError> {
    let mut diagnostics = Diagnostics::new();
    let geometry = Geometry::of(page, &mut diagnostics)?;
    document.record_diagnostics(diagnostics);

    if let Some(by) = edit.translate {
        let (dx, dy) = geometry.user_delta(by);
        let shift = |(x, y): Point| (x + dx, y + dy);
        parsed.rect = [
            parsed.rect[0] + dx,
            parsed.rect[1] + dy,
            parsed.rect[2] + dx,
            parsed.rect[3] + dy,
        ];
        for quad in &mut parsed.quads {
            *quad = quad.map(shift);
        }
        for stroke in &mut parsed.strokes {
            *stroke = stroke.iter().copied().map(shift).collect();
        }
        insert(dictionary, RECT_KEY, numbers_object(parsed.rect)?);
        if !parsed.quads.is_empty() {
            insert(
                dictionary,
                QUAD_POINTS_KEY,
                numbers_object(flatten(&parsed.quads))?,
            );
        }
        if !parsed.strokes.is_empty() {
            insert(dictionary, INK_LIST_KEY, ink_list(&parsed.strokes)?);
        }
    }
    if let Some(colour) = &edit.colour {
        set_colour(dictionary, colour)?;
    }
    if let Some(opacity) = edit.opacity {
        set_opacity(dictionary, opacity)?;
    }
    let mut width = document.border_width(dictionary)?;
    if let Some(new_width) = edit.line_width {
        width = new_width;
        let mut style = match document.get(dictionary, BORDER_STYLE_KEY)?.dictionary() {
            Some(style) => style.clone(),
            None => Dictionary::new(),
        };
        insert(&mut style, WIDTH_KEY, number_object(width)?);
        insert(
            dictionary,
            BORDER_STYLE_KEY,
            direct(Value::Dictionary(style)),
        );
    }

    let colour = edit.colour.or(parsed.colour).ok_or_else(|| {
        invalid("the annotation has no colour to draw its appearance with".to_string())
    })?;
    let look = Look {
        colour,
        opacity: edit.opacity.unwrap_or(parsed.opacity),
    };
    let missing = |key: &'static str| PdfError::InvalidEntry {
        object: Some(id),
        key,
    };
    let drawn = match parsed.kind {
        AnnotationKind::Highlight if !parsed.quads.is_empty() => {
            appearance::highlight(&parsed.quads, parsed.rect, &look)?
        }
        AnnotationKind::Underline if !parsed.quads.is_empty() => {
            appearance::line_markup(&parsed.quads, false, parsed.rect, &look)?
        }
        AnnotationKind::StrikeOut if !parsed.quads.is_empty() => {
            appearance::line_markup(&parsed.quads, true, parsed.rect, &look)?
        }
        AnnotationKind::Highlight | AnnotationKind::Underline | AnnotationKind::StrikeOut => {
            return Err(missing("QuadPoints"));
        }
        AnnotationKind::Ink if !parsed.strokes.is_empty() => {
            appearance::ink(&parsed.strokes, width, parsed.rect, &look)?
        }
        AnnotationKind::Ink => return Err(missing("InkList")),
        AnnotationKind::Note => appearance::note(parsed.rect, &look)?,
        AnnotationKind::Other(_) => {
            return Err(PdfError::AnnotationNotEditable {
                subtype: kind_name(&parsed.kind),
            })
        }
    };
    let normal = add_appearance(rewriter, drawn)?;
    let existing = document
        .get(dictionary, APPEARANCE_KEY)?
        .dictionary()
        .cloned();
    insert(
        dictionary,
        APPEARANCE_KEY,
        appearance_dictionary(existing, normal),
    );
    Ok(())
}

/// `document` without annotation `id` of page `page`, counted from 1:
/// out of the page's `Annots` and out of the structure tree. What replies
/// to it (`IRT`) and its pop-up go with it. An annotation the page does not
/// list is [`PdfError::AnnotationNotOnPage`], and a widget, which belongs
/// to a form field, is [`PdfError::AnnotationNotEditable`].
pub fn delete_annotation(
    document: &Document,
    page: u32,
    id: ObjectId,
) -> Result<Document, PdfError> {
    let page_number = page;
    let page = document.page(page_number)?;
    let target = annotation_dictionary(document, &page, page_number, id)?;
    if subtype_name(&target) == Some(WIDGET) {
        return Err(PdfError::AnnotationNotEditable {
            subtype: "Widget".to_string(),
        });
    }
    let doomed = with_replies_and_popups(document, &page.dictionary, id)?;

    let mut rewriter = Rewriter::new(document);
    edit_annots(&mut rewriter, page.id, |items| {
        items.retain(|item| !reference(item).is_some_and(|id| doomed.contains(&id)));
    })?;
    let gone = Gone {
        pages: HashSet::new(),
        annotations: doomed,
    };
    let catalog = document.catalog()?.id;
    structure::prune(&mut rewriter, catalog, &gone)?;
    document.apply(rewriter.finish())
}

/// `id`, the annotations of the page that reply to one of those found
/// (`IRT`, Table 170) however deep, and the pop-up of each (`Popup`).
fn with_replies_and_popups(
    document: &Document,
    page: &Dictionary,
    id: ObjectId,
) -> Result<HashSet<ObjectId>, PdfError> {
    let listed: Vec<ObjectId> = document
        .get(page, ANNOTS_KEY)?
        .array()
        .unwrap_or_default()
        .iter()
        .filter_map(reference)
        .collect();
    let mut found = HashSet::from([id]);
    loop {
        let before = found.len();
        for &candidate in &listed {
            let Some(object) = document.object(candidate)? else {
                continue;
            };
            let Some(dictionary) = object.dictionary() else {
                continue;
            };
            let replies = dictionary
                .get(REPLY_TO_KEY)
                .and_then(reference)
                .is_some_and(|parent| found.contains(&parent));
            if replies {
                found.insert(candidate);
            }
        }
        for &member in &found.clone() {
            let Some(object) = document.object(member)? else {
                continue;
            };
            if let Some(popup) = object
                .dictionary()
                .and_then(|dictionary| dictionary.get(POPUP_KEY))
                .and_then(reference)
            {
                found.insert(popup);
            }
        }
        if found.len() == before {
            return Ok(found);
        }
    }
}
