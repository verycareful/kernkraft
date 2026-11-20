// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The engine behind request strings: each method reads one JSON request,
//! runs the engine's operation and answers one JSON string (see
//! [`crate::guard`]).

use std::sync::OnceLock;

use kernkraft_pdf_engine::edit::ApplyRequest;
use kernkraft_pdf_engine::fonts::FontsRequest;
use kernkraft_pdf_engine::open::Source;
use kernkraft_pdf_engine::read::SearchRequest;
use kernkraft_pdf_engine::render::RenderRequest;
use kernkraft_pdf_engine::save::{AppendRequest, CompactRequest, ConfirmRequest};
use kernkraft_pdf_engine::{Engine, HandleRequest, PageRequest};

use crate::guard::run;
use crate::open::OPEN_OPTIONS;

/// The open documents and the application's fonts, reached by request
/// strings.
pub struct Bridge {
    engine: Engine,
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Bridge {
    pub fn new() -> Self {
        Self {
            engine: Engine::new(OPEN_OPTIONS),
        }
    }

    pub fn open(&self, request: &str) -> String {
        run(request, |source: Source| self.engine.open(source))
    }

    pub fn close(&self, request: &str) -> String {
        run(request, |request: HandleRequest| self.engine.close(request))
    }

    pub fn page_info(&self, request: &str) -> String {
        run(request, |request: PageRequest| {
            self.engine.page_info(request)
        })
    }

    pub fn text(&self, request: &str) -> String {
        run(request, |request: PageRequest| self.engine.text(request))
    }

    pub fn search(&self, request: &str) -> String {
        run(request, |request: SearchRequest| {
            self.engine.search(request)
        })
    }

    pub fn links(&self, request: &str) -> String {
        run(request, |request: PageRequest| self.engine.links(request))
    }

    pub fn annotations(&self, request: &str) -> String {
        run(request, |request: PageRequest| {
            self.engine.annotations(request)
        })
    }

    pub fn apply(&self, request: &str) -> String {
        run(request, |request: ApplyRequest| self.engine.apply(request))
    }

    pub fn undo(&self, request: &str) -> String {
        run(request, |request: HandleRequest| self.engine.undo(request))
    }

    pub fn redo(&self, request: &str) -> String {
        run(request, |request: HandleRequest| self.engine.redo(request))
    }

    pub fn append_since(&self, request: &str) -> String {
        run(request, |request: AppendRequest| {
            self.engine.append_since(request)
        })
    }

    pub fn confirm_appended(&self, request: &str) -> String {
        run(request, |request: ConfirmRequest| {
            self.engine.confirm_appended(request)
        })
    }

    pub fn compact(&self, request: &str) -> String {
        run(request, |request: CompactRequest| {
            self.engine.compact(request)
        })
    }

    pub fn set_system_fonts(&self, request: &str) -> String {
        run(request, |request: FontsRequest| {
            self.engine.set_system_fonts(request)
        })
    }

    pub fn render_size(&self, request: &str) -> String {
        run(request, |request: RenderRequest| {
            self.engine.render_size(request)
        })
    }

    /// Draws the page into `buffer`, which must hold the whole picture.
    pub fn render(&self, request: &str, buffer: &mut [u8]) -> String {
        run(request, |request: RenderRequest| {
            self.engine.render(request, buffer)
        })
    }
}

/// The bridge the JNI exports use.
pub fn bridge() -> &'static Bridge {
    static BRIDGE: OnceLock<Bridge> = OnceLock::new();
    BRIDGE.get_or_init(Bridge::new)
}
