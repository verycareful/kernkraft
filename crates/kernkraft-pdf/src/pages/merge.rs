// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Merging documents: ISO 32000-1:2008, 7.7 (document structure).
//!
//! The merged document is built on a blank one. Each input's pages are
//! copied into it with the inheritable attributes (Table 30) they took
//! from the nodes above them, and with everything they use, under new
//! object numbers: an input's numbers are moved up by a fixed amount, so
//! the order of its objects is kept. A reference to an object an input
//! does not have stays a reference to a number nothing holds, which is
//! null (7.3.10).
//!
//! What is document-wide is merged, each by the rules of its structure:
//!
//! - the outline: the inputs' items follow one another (12.3.3);
//! - named destinations (12.3.2.3, 7.7.4) and the other name trees of the
//!   name dictionary: the entries of all inputs in one tree. A name an
//!   earlier input has gets `+1`, `+2`, ... after it, and every
//!   destination in the later input that used the name is changed to the
//!   new one;
//! - the form (12.7.2, 12.7.3): the fields follow one another. A
//!   top-level field named as an earlier one is renamed the same way,
//!   since fields of one name share one value;
//! - page labels (12.4.2): each input's ranges move to its place, and an
//!   input without labels is numbered from 1 as it was in its own file;
//! - the structure tree (14.7): the inputs' top-level elements follow one
//!   another, and the `ParentTree` keys of a later input (`StructParents`
//!   of its pages, `StructParent` of its annotations and XObjects) move up
//!   past the earlier ones;
//! - optional content (8.11.4): the groups of all inputs, each shown or
//!   hidden as it was.
//!
//! The first input also gives the document's `Lang`, `ViewerPreferences`,
//! `PageMode`, `PageLayout`, `OpenAction`, `URI` and `Extensions`. The
//! rest of the inputs' catalogs, and their `Info` and `Metadata`, describe
//! the document they were in, and are not carried; each such entry is a
//! [`DiagnosticKind::CatalogEntryNotCarried`].

use std::collections::{BTreeMap, HashMap, HashSet};

use kernkraft_fs::Progress;

use super::rewrite::{direct, integer, name, name_object, reference, reference_object, Rewriter};
use super::trees::{self, NAMES_KEY, NUMS_KEY};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{
    decode_text_string, encode_text_string, Content, Document, NameTree, NumberTree, OpenOptions,
    Resolved,
};
use crate::syntax::{Dictionary, Name, Object, ObjectId, PdfString, Value};
use crate::PdfError;

const CATALOG_ID: ObjectId = ObjectId {
    number: 1,
    generation: 0,
};
/// A reference to this leads to no object.
const NULL_ID: ObjectId = ObjectId {
    number: 0,
    generation: 0,
};
const PAGES_ID: ObjectId = ObjectId {
    number: 2,
    generation: 0,
};
/// The version of a merged file when every input is older (the
/// structure tree and page labels need 1.3, the cross-reference table is
/// what the writer gives a file of this version).
const MIN_VERSION: (u8, u8) = (1, 4);

/// Catalog entries the merge builds from all the inputs.
const MERGED_KEYS: [&[u8]; 10] = [
    b"Type",
    b"Pages",
    b"Outlines",
    b"Names",
    b"Dests",
    b"AcroForm",
    b"StructTreeRoot",
    b"MarkInfo",
    b"PageLabels",
    b"OCProperties",
];
/// Catalog entries the first input gives.
const FIRST_KEYS: [&[u8]; 7] = [
    b"Lang",
    b"ViewerPreferences",
    b"PageMode",
    b"PageLayout",
    b"OpenAction",
    b"URI",
    b"Extensions",
];
/// Entries of the catalog that say which version of the format it is
/// (7.7.2): the merged file's own header says it.
const VERSION_KEY: &[u8] = b"Version";

/// Concatenates `documents` in order into one document.
///
/// `options` are those of the document made (the caches it keeps).
/// Fewer than two inputs is [`PdfError::Invalid`]. `progress` gets inputs
/// done out of the input count; a cancel is [`PdfError::Cancelled`].
///
/// An encrypted input is read with the key it was opened with and the
/// merged document is not encrypted; a diagnostic says so. Saving it
/// (with [`crate::write::compact`]) gives a file whose number of pages and
/// order are the inputs'.
pub fn merge(
    documents: &[&Document],
    options: &OpenOptions,
    progress: &dyn Progress,
) -> Result<Document, PdfError> {
    if documents.len() < 2 {
        return Err(PdfError::Invalid(
            "merging needs at least two documents".to_string(),
        ));
    }
    let version = documents
        .iter()
        .map(|document| {
            let version = document.file().version();
            (version.major, version.minor)
        })
        .chain([MIN_VERSION])
        .max()
        .unwrap_or(MIN_VERSION);
    let blank = Document::from_bytes(blank_file(version), options, "")?;
    let mut rewriter = Rewriter::new(&blank);
    let mut diagnostics = Diagnostics::new();
    let outline_root = rewriter.add(direct(Value::Dictionary(typed(b"Outlines"))))?;
    let structure_root = rewriter.add(direct(Value::Dictionary(typed(b"StructTreeRoot"))))?;
    let mut merged = Merged::default();
    let total = documents.len() as u64;
    for (index, document) in documents.iter().enumerate() {
        if progress.is_cancelled() {
            return Err(PdfError::Cancelled);
        }
        merge_input(
            &mut rewriter,
            document,
            index == 0,
            outline_root,
            structure_root,
            &mut merged,
            &mut diagnostics,
        )?;
        progress.report(index as u64 + 1, total, "Merging");
    }
    finish(&mut rewriter, merged, outline_root, structure_root)?;
    let changes = rewriter.finish();
    blank.record_diagnostics(diagnostics);
    blank.apply(changes)
}

/// What the inputs have given so far.
#[derive(Default)]
struct Merged {
    pages: Vec<ObjectId>,
    outline: Vec<super::outline::Node>,
    /// The destinations by name; each name once.
    dests: BTreeMap<Vec<u8>, Object>,
    /// The other name trees of the name dictionary, by category.
    names: BTreeMap<Vec<u8>, BTreeMap<Vec<u8>, Object>>,
    /// Which category names are taken, for renaming.
    fields: Vec<Object>,
    field_names: HashSet<String>,
    form: Option<FormState>,
    structure: Option<StructureState>,
    /// Per input: its page label ranges, shifted to its place; `None` for
    /// an input with no labels.
    labels: Vec<Vec<(i64, Dictionary)>>,
    any_labels: bool,
    optional_content: Option<OptionalContent>,
    carried: Dictionary,
    /// The pages before the input being merged.
    page_offset: i64,
    /// The `ParentTree` keys before the input being merged.
    key_offset: i64,
}

#[derive(Default)]
struct FormState {
    default_appearance: Option<Object>,
    quadding: Option<Object>,
    need_appearances: bool,
    signature_flags: i64,
    resources: BTreeMap<Vec<u8>, BTreeMap<Vec<u8>, Object>>,
    calculation_order: Vec<Object>,
}

#[derive(Default)]
struct StructureState {
    children: Vec<Object>,
    parent_tree: BTreeMap<i64, Object>,
    role_map: BTreeMap<Vec<u8>, Object>,
    class_map: BTreeMap<Vec<u8>, Object>,
    ids: BTreeMap<Vec<u8>, Object>,
    suspects: bool,
}

#[derive(Default)]
struct OptionalContent {
    groups: Vec<Object>,
    off: Vec<Object>,
    order: Vec<Object>,
    any_order: bool,
    radio_groups: Vec<Object>,
    locked: Vec<Object>,
    automatic: Vec<Object>,
    configurations: Vec<Object>,
    properties: Option<Dictionary>,
}

fn typed(kind: &[u8]) -> Dictionary {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(&b"Type"[..]), name_object(kind));
    dictionary
}

/// A file of a catalog and an empty page tree.
fn blank_file(version: (u8, u8)) -> Vec<u8> {
    let mut out = format!("%PDF-{}.{}\n", version.0, version.1).into_bytes();
    let mut offsets = Vec::new();
    for body in [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [] /Count 0 >>",
    ] {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", offsets.len()).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 3\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// Copies objects of one input into the merged document under new
/// numbers.
struct Copier<'a> {
    document: &'a Document,
    /// The new number of the input's object number 1.
    first: u32,
    /// The input's highest object number.
    numbers: u32,
    /// Objects that are not copied but stand for one of the merged
    /// document: the input's catalog, page tree nodes, outline root and
    /// structure root.
    specials: HashMap<ObjectId, ObjectId>,
    copied: HashSet<ObjectId>,
    pending: Vec<ObjectId>,
    key_offset: i64,
    /// `Dest` names (catalog `Dests` entries) as the strings they are
    /// called in the merged name tree.
    dest_names: HashMap<Vec<u8>, Vec<u8>>,
    /// `Dest` strings that had to be renamed.
    dest_strings: HashMap<Vec<u8>, Vec<u8>>,
}

const STRUCT_KEYS: [&[u8]; 2] = [b"StructParents", b"StructParent"];

impl Copier<'_> {
    fn map(&self, id: ObjectId) -> ObjectId {
        if let Some(special) = self.specials.get(&id) {
            return *special;
        }
        // Object 0 is never in use, and a number past the input's last
        // leads to no object: both are null (7.3.10), and must not land on
        // an object of another input.
        if id.number == 0 || id.number > self.numbers {
            return NULL_ID;
        }
        ObjectId {
            number: self.first + (id.number - 1),
            generation: id.generation,
        }
    }

    /// `object` as it is in the merged document: references moved, key
    /// numbers and destination names changed as the module says. The
    /// objects it refers to are queued to be copied.
    fn object(&mut self, object: &Object) -> Object {
        match object.value() {
            Value::Reference(id) => {
                if !self.specials.contains_key(id) && !self.copied.contains(id) {
                    self.pending.push(*id);
                }
                reference_object(self.map(*id))
            }
            Value::Array(items) => direct(Value::Array(
                items.iter().map(|item| self.object(item)).collect(),
            )),
            Value::Dictionary(source) => direct(Value::Dictionary(self.dictionary(source))),
            _ => object.clone(),
        }
    }

    fn dictionary(&mut self, source: &Dictionary) -> Dictionary {
        let go_to = source.get(b"S").and_then(name) == Some(&b"GoTo"[..]);
        let mut out = Dictionary::new();
        for (key, value) in source.iter() {
            let mut value = self.object(value);
            if STRUCT_KEYS.contains(&key.as_bytes()) {
                if let Value::Integer(number) = value.value() {
                    value = integer(number.saturating_add(self.key_offset));
                }
            }
            if key.as_bytes() == b"Dest" || (go_to && key.as_bytes() == b"D") {
                value = self.renamed(value);
            }
            out.insert(key.clone(), value);
        }
        out
    }

    /// A destination `value` that is a name or a string, as the merged
    /// document calls it.
    fn renamed(&self, value: Object) -> Object {
        match value.value() {
            Value::Name(old) => match self.dest_names.get(old.as_bytes()) {
                Some(new) => direct(Value::String(PdfString::new(new.clone()))),
                None => value,
            },
            Value::String(old) => match self.dest_strings.get(old.as_bytes()) {
                Some(new) => direct(Value::String(PdfString::new(new.clone()))),
                None => value,
            },
            _ => value,
        }
    }

    /// Copies the objects queued, and those they refer to.
    fn drain(&mut self, rewriter: &mut Rewriter<'_>) -> Result<(), PdfError> {
        while let Some(old) = self.pending.pop() {
            if self.specials.contains_key(&old) || !self.copied.insert(old) {
                continue;
            }
            let Some(object) = self.document.object(old)? else {
                continue;
            };
            let new = self.map(old);
            match &object.content {
                Content::Object(value) => {
                    let copy = self.object(value);
                    rewriter.set(new, copy)?;
                }
                Content::Stream(stream) => {
                    let dictionary = self.dictionary(&stream.dictionary);
                    let data = self.document.stream_data(old, stream)?.into_owned();
                    rewriter.set_stream(new, dictionary, data)?;
                }
            }
        }
        Ok(())
    }
}

/// Adds to `taken` the first of `name`, `name+1`, `name+2`, ... that it
/// does not hold, and returns it.
fn unused_name(taken: &mut HashSet<Vec<u8>>, name: &[u8]) -> Vec<u8> {
    let mut candidate = name.to_vec();
    let mut suffix = 0u64;
    while taken.contains(&candidate) {
        suffix += 1;
        candidate = name.to_vec();
        candidate.extend_from_slice(format!("+{suffix}").as_bytes());
    }
    taken.insert(candidate.clone());
    candidate
}

fn merge_input(
    rewriter: &mut Rewriter<'_>,
    document: &Document,
    is_first: bool,
    outline_root: ObjectId,
    structure_root: ObjectId,
    merged: &mut Merged,
    diagnostics: &mut Diagnostics,
) -> Result<(), PdfError> {
    if document.decryptor().is_some() {
        diagnostics.push(DiagnosticKind::EncryptionNotCarried, 0);
    }
    if !document.signatures()?.is_empty() {
        diagnostics.push(DiagnosticKind::SignaturesInvalidated, 0);
    }
    let catalog = document.catalog()?;
    let catalog_dictionary = catalog.dictionary().cloned().unwrap_or_default();
    let numbers = document.next_object_number().saturating_sub(1);
    let first = rewriter.reserve_many(numbers)?;
    let page_count = document.page_count()?;
    let list = document.page_list()?;

    let mut specials: HashMap<ObjectId, ObjectId> = HashMap::new();
    specials.insert(catalog.id, CATALOG_ID);
    for node in &list.nodes {
        specials.insert(*node, PAGES_ID);
    }
    let old_outline = catalog_dictionary.get(b"Outlines").and_then(reference);
    if let Some(old) = old_outline {
        specials.insert(old, outline_root);
    }
    let old_structure = catalog_dictionary
        .get(b"StructTreeRoot")
        .and_then(reference);
    if let Some(old) = old_structure {
        specials.insert(old, structure_root);
    }

    let mut copier = Copier {
        document,
        first,
        numbers,
        specials,
        copied: HashSet::new(),
        pending: Vec::new(),
        key_offset: merged.key_offset,
        dest_names: HashMap::new(),
        dest_strings: HashMap::new(),
    };

    // Names first: they decide what the destinations in the copied
    // objects are called.
    let names_entry = document.get(&catalog_dictionary, b"Names")?;
    let names_dictionary = names_entry.dictionary().cloned().unwrap_or_default();
    let mut taken_dests: HashSet<Vec<u8>> = merged.dests.keys().cloned().collect();
    let mut dest_entries: Vec<(Vec<u8>, Object)> = Vec::new();
    if let Some(tree) = names_dictionary.get(b"Dests") {
        for (key, value) in name_pairs(document, tree)? {
            let new = unused_name(&mut taken_dests, &key);
            if new != key {
                copier.dest_strings.insert(key, new.clone());
            }
            dest_entries.push((new, value));
        }
    }
    let legacy = document.get(&catalog_dictionary, b"Dests")?;
    if let Some(legacy) = legacy.dictionary() {
        for (key, value) in legacy.iter() {
            let new = unused_name(&mut taken_dests, key.as_bytes());
            copier
                .dest_names
                .insert(key.as_bytes().to_vec(), new.clone());
            dest_entries.push((new, value.clone()));
        }
    }
    for (key, value) in dest_entries {
        let value = copier.object(&value);
        merged.dests.insert(key, value);
    }
    for (category, tree) in names_dictionary.iter() {
        if category.as_bytes() == b"Dests" {
            continue;
        }
        let category_entries = merged
            .names
            .entry(category.as_bytes().to_vec())
            .or_default();
        let mut taken: HashSet<Vec<u8>> = category_entries.keys().cloned().collect();
        for (key, value) in name_pairs(document, tree)? {
            let new = unused_name(&mut taken, &key);
            let value = copier.object(&value);
            category_entries.insert(new, value);
        }
    }

    // The tags: the keys of the later inputs move up.
    let structure = read_structure(document, &catalog_dictionary)?;
    let mut next_key = merged.key_offset;
    if let Some(structure) = &structure {
        let highest = structure
            .parent_tree
            .iter()
            .map(|(key, _)| *key)
            .max()
            .map_or(0, |key| key.saturating_add(1));
        next_key = merged
            .key_offset
            .saturating_add(highest.max(structure.next_key));
    }

    // The pages.
    for number in 1..=page_count {
        let id = document.page_id(number)?;
        copier.copied.insert(id);
    }
    for number in 1..=page_count {
        let id = document.page_id(number)?;
        let page = document.required(id)?;
        let mut dictionary = page.dictionary().cloned().unwrap_or_default();
        for (key, value) in document.inherited_entries(number)? {
            dictionary.insert(key, value);
        }
        dictionary.remove(b"Parent");
        let mut copy = copier.dictionary(&dictionary);
        copy.insert(Name::new(&b"Parent"[..]), reference_object(PAGES_ID));
        let new = copier.map(id);
        rewriter.set(new, direct(Value::Dictionary(copy)))?;
        merged.pages.push(new);
    }

    // The outline.
    let place = merged.outline.len();
    for item in document.outline()? {
        merged.outline.push(super::outline::Node {
            id: copier.map(item.id),
            open: item.open,
            parent: item.parent.map(|parent| parent + place),
        });
        copier.pending.push(item.id);
    }

    // The form.
    if let Some(form) = document.get(&catalog_dictionary, b"AcroForm")?.dictionary() {
        merge_form(rewriter, document, form, &mut copier, merged, diagnostics)?;
    }

    // The structure tree.
    if let Some(structure) = structure {
        merge_structure(&mut copier, structure, merged, diagnostics);
    }

    // Page labels.
    let labels = read_labels(
        document,
        &catalog_dictionary,
        merged.page_offset,
        &mut copier,
    )?;
    merged.any_labels |= labels.is_some();
    let ranges = labels.unwrap_or_else(|| {
        let mut decimal = Dictionary::new();
        decimal.insert(Name::new(&b"S"[..]), name_object(b"D"));
        vec![(merged.page_offset, decimal)]
    });
    merged.labels.push(ranges);

    // Optional content.
    if let Some(properties) = document
        .get(&catalog_dictionary, b"OCProperties")?
        .dictionary()
    {
        merge_optional_content(document, properties, &mut copier, merged)?;
    }

    // The catalog.
    if is_first {
        for key in FIRST_KEYS {
            if let Some(value) = catalog_dictionary.get(key) {
                let value = copier.object(value);
                merged.carried.insert(Name::new(key), value);
            }
        }
    }
    for (key, _) in catalog_dictionary.iter() {
        let key = key.as_bytes();
        let carried = is_first && FIRST_KEYS.contains(&key);
        if !carried && !MERGED_KEYS.contains(&key) && key != VERSION_KEY {
            diagnostics.push(
                DiagnosticKind::CatalogEntryNotCarried {
                    key: Name::new(key),
                },
                0,
            );
        }
    }
    if document.trailer().get(b"Info").is_some() {
        diagnostics.push(
            DiagnosticKind::CatalogEntryNotCarried {
                key: Name::new(&b"Info"[..]),
            },
            0,
        );
    }

    copier.drain(rewriter)?;
    merged.page_offset += i64::from(page_count);
    merged.key_offset = next_key;
    Ok(())
}

/// The pairs of the name tree `root` is. A root that leads to no object
/// is no tree (7.3.10).
fn name_pairs(document: &Document, root: &Object) -> Result<Vec<(Vec<u8>, Object)>, PdfError> {
    if matches!(document.resolve(root)?, Resolved::Null) {
        return Ok(Vec::new());
    }
    NameTree::new(document, root.clone()).entries()
}

/// The pairs of the number tree `root` is, as [`name_pairs`].
fn number_pairs(document: &Document, root: &Object) -> Result<Vec<(i64, Object)>, PdfError> {
    if matches!(document.resolve(root)?, Resolved::Null) {
        return Ok(Vec::new());
    }
    NumberTree::new(document, root.clone()).entries()
}

/// What an input's structure tree gives.
struct InputStructure {
    children: Vec<Object>,
    parent_tree: Vec<(i64, Object)>,
    next_key: i64,
    role_map: Option<Dictionary>,
    class_map: Option<Dictionary>,
    ids: Vec<(Vec<u8>, Object)>,
    suspects: bool,
}

fn read_structure(
    document: &Document,
    catalog: &Dictionary,
) -> Result<Option<InputStructure>, PdfError> {
    let root = document.get(catalog, b"StructTreeRoot")?;
    let Some(root) = root.dictionary() else {
        return Ok(None);
    };
    let children = match root.get(b"K") {
        Some(k) => match document.resolve(k)?.array() {
            Some(items) => items.to_vec(),
            None => vec![k.clone()],
        },
        None => Vec::new(),
    };
    let parent_tree = match root.get(b"ParentTree") {
        Some(tree) => number_pairs(document, tree)?,
        None => Vec::new(),
    };
    let next_key = match document.get(root, b"ParentTreeNextKey")?.value() {
        Value::Integer(key) => *key,
        _ => 0,
    };
    let ids = match root.get(b"IDTree") {
        Some(tree) => name_pairs(document, tree)?,
        None => Vec::new(),
    };
    let mark_info = document.get(catalog, b"MarkInfo")?;
    let suspects = mark_info
        .dictionary()
        .and_then(|info| info.get(b"Suspects"))
        .is_some_and(|value| matches!(value.value(), Value::Boolean(true)));
    Ok(Some(InputStructure {
        children,
        parent_tree,
        next_key,
        role_map: document.get(root, b"RoleMap")?.dictionary().cloned(),
        class_map: document.get(root, b"ClassMap")?.dictionary().cloned(),
        ids,
        suspects,
    }))
}

fn merge_structure(
    copier: &mut Copier<'_>,
    structure: InputStructure,
    merged: &mut Merged,
    diagnostics: &mut Diagnostics,
) {
    let state = merged.structure.get_or_insert_with(StructureState::default);
    state.suspects |= structure.suspects;
    for child in &structure.children {
        state.children.push(copier.object(child));
    }
    for (key, value) in &structure.parent_tree {
        let value = copier.object(value);
        state
            .parent_tree
            .insert(key.saturating_add(copier.key_offset), value);
    }
    for (maps, input) in [
        (&mut state.role_map, structure.role_map),
        (&mut state.class_map, structure.class_map),
    ] {
        for (key, value) in input.iter().flat_map(Dictionary::iter) {
            let value = copier.object(value);
            if maps.get(key.as_bytes()).is_some_and(|kept| *kept != value) {
                diagnostics.push(DiagnosticKind::StructureEntryKept { key: key.clone() }, 0);
            } else {
                maps.insert(key.as_bytes().to_vec(), value);
            }
        }
    }
    for (key, value) in structure.ids {
        let value = copier.object(&value);
        match state.ids.entry(key) {
            std::collections::btree_map::Entry::Occupied(taken) => {
                diagnostics.push(
                    DiagnosticKind::StructureEntryKept {
                        key: Name::new(taken.key().clone()),
                    },
                    0,
                );
            }
            std::collections::btree_map::Entry::Vacant(free) => {
                free.insert(value);
            }
        }
    }
}

fn merge_form(
    rewriter: &mut Rewriter<'_>,
    document: &Document,
    form: &Dictionary,
    copier: &mut Copier<'_>,
    merged: &mut Merged,
    diagnostics: &mut Diagnostics,
) -> Result<(), PdfError> {
    if form.get(b"XFA").is_some() {
        diagnostics.push(DiagnosticKind::XfaNotCarried, 0);
    }
    let state = merged.form.get_or_insert_with(FormState::default);
    if state.default_appearance.is_none() {
        state.default_appearance = form.get(b"DA").map(|value| copier.object(value));
    }
    if state.quadding.is_none() {
        state.quadding = form.get(b"Q").map(|value| copier.object(value));
    }
    if let Value::Boolean(true) = document.get(form, b"NeedAppearances")?.value() {
        state.need_appearances = true;
    }
    if let Value::Integer(flags) = document.get(form, b"SigFlags")?.value() {
        state.signature_flags |= *flags;
    }
    if let Some(resources) = document.get(form, b"DR")?.dictionary() {
        for (category, entries) in resources.iter() {
            let entries = document.resolve(entries)?;
            let Some(entries) = entries.dictionary() else {
                continue;
            };
            let merged_entries = state
                .resources
                .entry(category.as_bytes().to_vec())
                .or_default();
            for (key, value) in entries.iter() {
                let value = copier.object(value);
                match merged_entries.get(key.as_bytes()) {
                    Some(kept) if *kept != value => {
                        diagnostics.push(DiagnosticKind::ResourceNameKept { key: key.clone() }, 0);
                    }
                    Some(_) => {}
                    None => {
                        merged_entries.insert(key.as_bytes().to_vec(), value);
                    }
                }
            }
        }
    }
    let roots = document.get(form, b"Fields")?;
    for root in roots.array().unwrap_or_default() {
        let Some(old) = reference(root) else {
            continue;
        };
        // A top-level field named as an earlier one is renamed; a field
        // with no name of its own is left alone.
        let partial = document.object(old)?.and_then(|field| {
            field
                .dictionary()
                .and_then(|field| field.get(b"T").cloned())
        });
        if let Some(Value::String(partial)) = partial.as_ref().map(Object::value) {
            let text = decode_text_string(partial.as_bytes()).text;
            let mut candidate = text.clone();
            let mut suffix = 0u64;
            while merged.field_names.contains(&candidate) {
                suffix += 1;
                candidate = format!("{text}+{suffix}");
            }
            merged.field_names.insert(candidate.clone());
            if candidate != text {
                let new = copier.map(old);
                rewriter_rename(rewriter, copier, old, new, &candidate)?;
            }
        }
        merged.fields.push(copier.object(root));
    }
    if let Some(order) = document.get(form, b"CO")?.array() {
        for item in order {
            let item = copier.object(item);
            state.calculation_order.push(item);
        }
    }
    Ok(())
}

/// Copies field `old` now, with its partial name changed to `name`.
fn rewriter_rename(
    rewriter: &mut Rewriter<'_>,
    copier: &mut Copier<'_>,
    old: ObjectId,
    new: ObjectId,
    name: &str,
) -> Result<(), PdfError> {
    let Some(field) = copier.document.object(old)? else {
        return Ok(());
    };
    let Some(source) = field.dictionary() else {
        return Ok(());
    };
    copier.copied.insert(old);
    let mut copy = copier.dictionary(source);
    copy.insert(
        Name::new(&b"T"[..]),
        direct(Value::String(PdfString::new(encode_text_string(name)))),
    );
    match &field.content {
        Content::Object(_) => rewriter.set(new, direct(Value::Dictionary(copy))),
        Content::Stream(stream) => {
            let data = copier.document.stream_data(old, stream)?.into_owned();
            rewriter.set_stream(new, copy, data)
        }
    }
}

fn read_labels(
    document: &Document,
    catalog: &Dictionary,
    offset: i64,
    copier: &mut Copier<'_>,
) -> Result<Option<Vec<(i64, Dictionary)>>, PdfError> {
    let Some(root) = catalog.get(b"PageLabels") else {
        return Ok(None);
    };
    if document.resolve(root)?.dictionary().is_none() {
        return Ok(None);
    }
    let mut ranges: Vec<(i64, Dictionary)> = Vec::new();
    for (first, value) in number_pairs(document, root)? {
        let resolved = document.resolve(&value)?;
        let dictionary = resolved.dictionary().cloned().unwrap_or_default();
        let copy = copier.dictionary(&dictionary);
        ranges.push((first.saturating_add(offset), copy));
    }
    ranges.sort_by_key(|(first, _)| *first);
    if ranges.first().is_none_or(|(first, _)| *first > offset) {
        ranges.insert(0, (offset, Dictionary::new()));
    }
    Ok(Some(ranges))
}

fn merge_optional_content(
    document: &Document,
    properties: &Dictionary,
    copier: &mut Copier<'_>,
    merged: &mut Merged,
) -> Result<(), PdfError> {
    let state = merged
        .optional_content
        .get_or_insert_with(OptionalContent::default);
    let groups: Vec<Object> = document
        .get(properties, b"OCGs")?
        .array()
        .unwrap_or_default()
        .to_vec();
    let config = document.get(properties, b"D")?.dictionary().cloned();
    let config = config.unwrap_or_default();
    let refs = |key: &[u8]| -> Result<HashSet<ObjectId>, PdfError> {
        Ok(document
            .get(&config, key)?
            .array()
            .unwrap_or_default()
            .iter()
            .filter_map(reference)
            .collect())
    };
    let on = refs(b"ON")?;
    let off = refs(b"OFF")?;
    let base_off = matches!(
        document.get(&config, b"BaseState")?.value(),
        Value::Name(base) if base.as_bytes() == b"OFF"
    );
    for group in &groups {
        let old = reference(group);
        let hidden = old.is_some_and(|old| {
            if base_off {
                !on.contains(&old)
            } else {
                off.contains(&old)
            }
        });
        let group = copier.object(group);
        if hidden {
            state.off.push(group.clone());
        }
        state.groups.push(group);
    }
    match document.get(&config, b"Order")?.array() {
        Some(order) => {
            state.any_order = true;
            for item in order {
                let item = copier.object(item);
                state.order.push(item);
            }
        }
        None => {
            for group in &groups {
                let group = copier.object(group);
                state.order.push(group);
            }
        }
    }
    for (key, into) in [
        (&b"RBGroups"[..], &mut state.radio_groups),
        (b"Locked", &mut state.locked),
        (b"AS", &mut state.automatic),
    ] {
        for item in document.get(&config, key)?.array().unwrap_or_default() {
            let item = copier.object(item);
            into.push(item);
        }
    }
    for item in document
        .get(properties, b"Configs")?
        .array()
        .unwrap_or_default()
    {
        let item = copier.object(item);
        state.configurations.push(item);
    }
    if state.properties.is_none() {
        let mut first = Dictionary::new();
        for key in [&b"Name"[..], b"Creator", b"ListMode", b"Intent"] {
            if let Some(value) = config.get(key) {
                first.insert(Name::new(key), copier.object(value));
            }
        }
        state.properties = Some(first);
    }
    Ok(())
}

/// Writes the merged structures into the merged document.
fn finish(
    rewriter: &mut Rewriter<'_>,
    merged: Merged,
    outline_root: ObjectId,
    structure_root: ObjectId,
) -> Result<(), PdfError> {
    let count = i64::try_from(merged.pages.len()).unwrap_or(i64::MAX);
    let kids = merged
        .pages
        .iter()
        .map(|&id| reference_object(id))
        .collect();
    rewriter.edit(PAGES_ID, |_, root| {
        root.insert(Name::new(&b"Kids"[..]), direct(Value::Array(kids)));
        root.insert(Name::new(&b"Count"[..]), integer(count));
        Ok(())
    })?;

    let mut catalog_entries: Vec<(&'static [u8], Object)> = Vec::new();

    if !merged.outline.is_empty() {
        super::outline::relink(rewriter, outline_root, &merged.outline)?;
        catalog_entries.push((b"Outlines", reference_object(outline_root)));
    }

    let mut names = Dictionary::new();
    let mut categories: BTreeMap<Vec<u8>, BTreeMap<Vec<u8>, Object>> = merged.names;
    if !merged.dests.is_empty() {
        categories.insert(b"Dests".to_vec(), merged.dests);
    }
    for (category, entries) in categories {
        if entries.is_empty() {
            continue;
        }
        let pairs = entries
            .into_iter()
            .map(|(key, value)| (direct(Value::String(PdfString::new(key))), value))
            .collect();
        let tree = trees::build(rewriter, NAMES_KEY, pairs)?;
        names.insert(Name::new(category), tree);
    }
    if !names.is_empty() {
        catalog_entries.push((b"Names", direct(Value::Dictionary(names))));
    }

    if let Some(form) = merged.form {
        let mut dictionary = Dictionary::new();
        dictionary.insert(
            Name::new(&b"Fields"[..]),
            direct(Value::Array(merged.fields)),
        );
        if form.need_appearances {
            dictionary.insert(
                Name::new(&b"NeedAppearances"[..]),
                direct(Value::Boolean(true)),
            );
        }
        if form.signature_flags != 0 {
            dictionary.insert(Name::new(&b"SigFlags"[..]), integer(form.signature_flags));
        }
        if !form.calculation_order.is_empty() {
            dictionary.insert(
                Name::new(&b"CO"[..]),
                direct(Value::Array(form.calculation_order)),
            );
        }
        if let Some(value) = form.default_appearance {
            dictionary.insert(Name::new(&b"DA"[..]), value);
        }
        if let Some(value) = form.quadding {
            dictionary.insert(Name::new(&b"Q"[..]), value);
        }
        let mut resources = Dictionary::new();
        for (category, entries) in form.resources {
            let mut group = Dictionary::new();
            for (key, value) in entries {
                group.insert(Name::new(key), value);
            }
            resources.insert(Name::new(category), direct(Value::Dictionary(group)));
        }
        if !resources.is_empty() {
            dictionary.insert(Name::new(&b"DR"[..]), direct(Value::Dictionary(resources)));
        }
        catalog_entries.push((b"AcroForm", direct(Value::Dictionary(dictionary))));
    }

    if let Some(structure) = merged.structure {
        write_structure(rewriter, structure_root, structure, &mut catalog_entries)?;
    }

    if merged.any_labels {
        let mut pairs: Vec<(Object, Object)> = Vec::new();
        for ranges in merged.labels {
            for (first, dictionary) in ranges {
                pairs.push((integer(first), direct(Value::Dictionary(dictionary))));
            }
        }
        let tree = trees::build(rewriter, NUMS_KEY, pairs)?;
        catalog_entries.push((b"PageLabels", tree));
    }

    if let Some(content) = merged.optional_content {
        let mut properties = Dictionary::new();
        properties.insert(
            Name::new(&b"OCGs"[..]),
            direct(Value::Array(content.groups)),
        );
        let mut config = content.properties.unwrap_or_default();
        if !content.off.is_empty() {
            config.insert(Name::new(&b"OFF"[..]), direct(Value::Array(content.off)));
        }
        if content.any_order {
            config.insert(
                Name::new(&b"Order"[..]),
                direct(Value::Array(content.order)),
            );
        }
        for (key, items) in [
            (&b"RBGroups"[..], content.radio_groups),
            (b"Locked", content.locked),
            (b"AS", content.automatic),
        ] {
            if !items.is_empty() {
                config.insert(Name::new(key), direct(Value::Array(items)));
            }
        }
        properties.insert(Name::new(&b"D"[..]), direct(Value::Dictionary(config)));
        if !content.configurations.is_empty() {
            properties.insert(
                Name::new(&b"Configs"[..]),
                direct(Value::Array(content.configurations)),
            );
        }
        catalog_entries.push((b"OCProperties", direct(Value::Dictionary(properties))));
    }

    let carried = merged.carried;
    rewriter.edit(CATALOG_ID, |_, catalog| {
        for (key, value) in carried.iter() {
            catalog.insert(key.clone(), value.clone());
        }
        for (key, value) in catalog_entries {
            catalog.insert(Name::new(key), value);
        }
        Ok(())
    })?;
    Ok(())
}

fn write_structure(
    rewriter: &mut Rewriter<'_>,
    root: ObjectId,
    structure: StructureState,
    catalog_entries: &mut Vec<(&'static [u8], Object)>,
) -> Result<(), PdfError> {
    let parent_pairs: Vec<(Object, Object)> = structure
        .parent_tree
        .into_iter()
        .map(|(key, value)| (integer(key), value))
        .collect();
    let next_key = parent_pairs
        .iter()
        .filter_map(|(key, _)| match key.value() {
            Value::Integer(key) => Some(key.saturating_add(1)),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let parent_tree = trees::build(rewriter, NUMS_KEY, parent_pairs)?;
    let id_tree = if structure.ids.is_empty() {
        None
    } else {
        let pairs = structure
            .ids
            .into_iter()
            .map(|(key, value)| (direct(Value::String(PdfString::new(key))), value))
            .collect();
        Some(trees::build(rewriter, NAMES_KEY, pairs)?)
    };
    let children = structure.children;
    let role_map = structure.role_map;
    let class_map = structure.class_map;
    rewriter.edit(root, |_, dictionary| {
        if !children.is_empty() {
            dictionary.insert(Name::new(&b"K"[..]), direct(Value::Array(children)));
        }
        dictionary.insert(Name::new(&b"ParentTree"[..]), parent_tree);
        dictionary.insert(Name::new(&b"ParentTreeNextKey"[..]), integer(next_key));
        if let Some(tree) = id_tree {
            dictionary.insert(Name::new(&b"IDTree"[..]), tree);
        }
        for (key, map) in [(&b"RoleMap"[..], role_map), (b"ClassMap", class_map)] {
            if !map.is_empty() {
                let mut entries = Dictionary::new();
                for (name, value) in map {
                    entries.insert(Name::new(name), value);
                }
                dictionary.insert(Name::new(key), direct(Value::Dictionary(entries)));
            }
        }
        Ok(())
    })?;
    catalog_entries.push((b"StructTreeRoot", reference_object(root)));
    let mut mark_info = Dictionary::new();
    mark_info.insert(Name::new(&b"Marked"[..]), direct(Value::Boolean(true)));
    if structure.suspects {
        mark_info.insert(Name::new(&b"Suspects"[..]), direct(Value::Boolean(true)));
    }
    catalog_entries.push((b"MarkInfo", direct(Value::Dictionary(mark_info))));
    Ok(())
}
