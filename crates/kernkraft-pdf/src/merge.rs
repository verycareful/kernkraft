// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 the Qyra authors (github.com/zParik/Qyra)
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Merging documents, after Qyra's merge command: each input's objects are
//! renumbered past the previous input's, all objects are pooled, and one
//! new catalog and page-tree root take every page in input order.

use kernkraft_fs::{FsError, Progress};
use lopdf::{dictionary, Document, Object, ObjectId};

use crate::tree::{flatten, prune, renumber};
use crate::PdfError;

/// PDF version written when every input is older.
const MIN_VERSION: &str = "1.4";

/// Concatenates `docs` in order into one document.
///
/// Each input is flattened first, so its pages keep the attributes they
/// inherited from its own page tree. The result's catalog holds only the
/// page tree: the inputs' outlines, named destinations, form dictionaries
/// (`/AcroForm`) and structure trees are not carried over, while each
/// page's own annotations are. The result takes the highest PDF version
/// among the inputs.
///
/// Fewer than two inputs is [`PdfError::Invalid`]. `progress` gets inputs
/// done out of the input count; a cancel stops with
/// `PdfError::Fs(FsError::Cancelled)`.
pub fn merge(docs: Vec<Document>, progress: &dyn Progress) -> Result<Document, PdfError> {
    if docs.len() < 2 {
        return Err(PdfError::Invalid(
            "merging needs at least two documents".to_string(),
        ));
    }
    let total = docs.len() as u64;
    let version = docs
        .iter()
        .map(|d| d.version.as_str())
        .chain([MIN_VERSION])
        .max_by(|a, b| version_key(a).cmp(&version_key(b)))
        .unwrap_or(MIN_VERSION)
        .to_string();
    let mut merged = Document::with_version(version);
    let mut pages: Vec<ObjectId> = Vec::new();
    let mut next_id = 1;

    for (i, mut doc) in docs.into_iter().enumerate() {
        if progress.is_cancelled() {
            return Err(FsError::Cancelled.into());
        }
        next_id = renumber(&mut doc, next_id)?;
        let (_, doc_pages) = flatten(&mut doc)?;
        pages.extend(doc_pages);
        merged.objects.append(&mut doc.objects);
        progress.report(i as u64 + 1, total, "Merging");
    }

    let root: ObjectId = (next_id, 0);
    let catalog: ObjectId = (next_id + 1, 0);
    for &page in &pages {
        merged
            .get_dictionary_mut(page)
            .map_err(|e| PdfError::Parse(format!("page {page:?}: {e}")))?
            .set("Parent", Object::Reference(root));
    }
    merged.objects.insert(
        root,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => pages.iter().map(|&p| Object::Reference(p)).collect::<Vec<_>>(),
            "Count" => pages.len() as i64,
        }),
    );
    merged.objects.insert(
        catalog,
        Object::Dictionary(dictionary! { "Type" => "Catalog", "Pages" => root }),
    );
    merged.trailer.set("Root", catalog);
    prune(&mut merged);
    renumber(&mut merged, 1)?;
    Ok(merged)
}

/// `"1.7"` as `(1, 7)`; anything unparsable sorts lowest.
fn version_key(v: &str) -> (u32, u32) {
    let mut parts = v.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}
