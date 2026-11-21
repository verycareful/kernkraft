// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Error names as the application receives them.

use std::path::PathBuf;

use kernkraft_font::FontError;
use kernkraft_fs::FsError;
use kernkraft_pdf::{EditSession, PdfError};
use kernkraft_pdf_engine::error::{kind_of, variant_name, WireDetails, WireError};

/// A session is held in a registry shared by every thread the application calls from,
/// so it has to be `Send`. This fails to compile when it is not.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<EditSession>();
};

#[test]
fn a_pdf_error_is_named_by_its_variant() {
    assert_eq!(kind_of(&PdfError::Cancelled), "Cancelled");
    assert_eq!(
        kind_of(&PdfError::PageOutOfRange { page: 9, count: 3 }),
        "PageOutOfRange"
    );
    assert_eq!(kind_of(&PdfError::DuplicatePage(2)), "DuplicatePage");
    assert_eq!(kind_of(&PdfError::BadRange("x".into())), "BadRange");
}

#[test]
fn a_forwarding_variant_reports_the_inner_name_and_the_outer_message() {
    let font = PdfError::Font(FontError::NoVariationAxes);
    let wire = WireError::from(&font);

    assert_eq!(wire.kind, "NoVariationAxes");
    assert_eq!(wire.message, font.to_string());
}

#[test]
fn variant_name_stops_at_the_field_list() {
    assert_eq!(variant_name(&FontError::NoVariationAxes), "NoVariationAxes");
    assert_eq!(
        variant_name(&PdfError::PageOutOfRange { page: 1, count: 1 }),
        "PageOutOfRange"
    );
    assert_eq!(variant_name(&PdfError::DuplicatePage(1)), "DuplicatePage");
}

#[test]
fn a_forwarded_filesystem_error_keeps_its_path() {
    let path = PathBuf::from("/home/user/doc.pdf");
    for (error, kind) in [
        (
            FsError::ExternalModification { path: path.clone() },
            "ExternalModification",
        ),
        (FsError::Exists { path: path.clone() }, "Exists"),
        (FsError::NotFound { path: path.clone() }, "NotFound"),
        (
            FsError::Io {
                path: path.clone(),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
            "Io",
        ),
    ] {
        let wire = WireError::from(&PdfError::Fs(error));

        assert_eq!(wire.kind, kind);
        assert_eq!(wire.path, Some(path.clone()));
        assert_eq!(wire.details, None);
    }
}

#[test]
fn the_attributes_a_save_could_not_keep_are_the_details() {
    let wire = WireError::from(&PdfError::Fs(FsError::XattrsNotKept {
        path: PathBuf::from("/home/user/doc.pdf"),
        names: vec!["user.xdg.tags".to_owned(), "security.selinux".to_owned()],
    }));

    assert_eq!(wire.kind, "XattrsNotKept");
    assert_eq!(wire.path, Some(PathBuf::from("/home/user/doc.pdf")));
    assert_eq!(
        wire.details,
        Some(WireDetails::XattrsNotKept {
            names: vec!["user.xdg.tags".to_owned(), "security.selinux".to_owned()]
        })
    );
}

#[test]
fn a_filesystem_error_about_no_path_has_none() {
    let wire = WireError::from(&PdfError::Fs(FsError::Cancelled));

    assert_eq!(wire.kind, "Cancelled");
    assert_eq!(wire.path, None);
    assert_eq!(wire.details, None);
}

#[test]
fn a_pdf_error_that_is_not_a_filesystem_error_has_no_path_or_details() {
    for error in [
        PdfError::PageOutOfRange { page: 9, count: 3 },
        PdfError::Font(FontError::NoVariationAxes),
    ] {
        let wire = WireError::from(&error);

        assert_eq!(wire.path, None);
        assert_eq!(wire.details, None);
    }
}

#[test]
fn a_save_over_a_file_changed_since_it_was_opened_names_the_file() {
    use kernkraft_fs::Expect;
    use kernkraft_pdf::doc::OpenOptions;
    use kernkraft_pdf::file::ReadLimits;
    use kernkraft_pdf_engine::open::Source;
    use kernkraft_pdf_engine::Engine;

    let options = OpenOptions {
        limits: ReadLimits {
            max_object_elements: 1 << 20,
            max_xref_entries: 1 << 24,
            max_object_stream_bytes: 1 << 26,
        },
        cached_objects: 64,
        cached_object_streams: 4,
        max_diagnostics: 100,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("work.pdf");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf"),
        &path,
    )
    .unwrap();
    let engine = Engine::new(options);
    let handle = engine
        .open(Source {
            path: path.clone(),
            password: String::new(),
        })
        .unwrap()["handle"]
        .as_i64()
        .unwrap();
    let stamp = kernkraft_fs::FileStamp::of(&path).unwrap();
    std::fs::write(&path, b"changed under the session").unwrap();

    let refused = engine
        .save(
            handle,
            &path,
            Expect::Unchanged(&stamp),
            "D:20261121103000Z",
        )
        .unwrap_err();

    assert_eq!(refused.kind, "ExternalModification");
    assert_eq!(refused.path, Some(path));
}
