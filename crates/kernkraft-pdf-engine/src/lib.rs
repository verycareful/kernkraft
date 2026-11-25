// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The PDF operations Kern Android and Kern Desktop both expose, over
//! `kernkraft-pdf`.
//!
//! An [`Engine`] holds the open documents: each is an `EditSession` with a
//! revision number and a record of the pages every step changed, reached by
//! an `i64` handle. Every operation takes a typed request (the request types
//! implement `serde::Deserialize`, so an adapter can read one from JSON or
//! from its own IPC) and answers with the fields of a JSON object
//! ([`Fields`]) or a [`WireError`] whose `kind` is the name of the error
//! variant. Geometry is fractions of the displayed page (see [`wire`]).
//!
//! A long request may name a job (the `job` field), which the application
//! can cancel and watch: see [`jobs`].
//!
//! The operations that keep nothing (merge, split by ranges, extract) are
//! free functions in [`stateless`]. Every file is opened under the
//! [`OpenOptions`](kernkraft_pdf::doc::OpenOptions) the adapter gives:
//! what a document may cost depends on the device.
//!
//! This crate knows no platform. An adapter (`kernkraft-jni`,
//! `kernkraft-tauri`) turns its own calls into these requests and these
//! answers into its own replies.

pub mod colour;
pub mod edit;
pub mod engine;
pub mod error;
pub mod fields;
pub mod fonts;
pub mod jobs;
pub mod open;
pub mod read;
pub mod registry;
pub mod render;
pub mod save;
pub mod stateless;
pub mod wire;

pub use engine::{Changed, Engine, HandleRequest, PageRequest, Session};
pub use error::{WireDetails, WireError};
pub use fields::{field, path_field, paths_field, Fields};
pub use jobs::{JobId, JobRequest};
