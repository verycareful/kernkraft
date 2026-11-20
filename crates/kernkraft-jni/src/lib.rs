// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! JNI adapter that exposes `kernkraft-pdf` to Kern Android as
//! `dev.kern.pdfbridge.KernkraftPdf`, in `libkernkraft_jni.so`.
//!
//! Every operation takes one JSON request string and answers one JSON
//! string: the answer's fields with `"ok":true`, or `{"ok":false,"kind":
//! ...,"message":...}` (see [`error`]). The pixels of a render are the one
//! thing that does not travel as JSON. A panic in any operation is caught
//! and answered as a `Panic` failure.
//!
//! This is an adapter crate: it is the only kernkraft crate that depends on
//! `jni`. The core crates never do. Each `Java_*` export in [`exports`] is a
//! thin shell over a function of this crate that takes and gives strings, so
//! the operations are tested on the host without a JVM.

pub mod edit;
pub mod engine;
pub mod error;
pub mod exports;
pub mod fonts;
pub mod guard;
pub mod open;
pub mod read;
pub mod registry;
pub mod render;
pub mod save;
pub mod stateless;
pub mod wire;
