// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! System theme sources and the token resolver for Kern-family apps.
//!
//! Follows the parent design spec, section 6.5 (System theming): Hyprland's
//! `hyprtoolkit.conf`, the xdg-desktop-portal appearance settings, GTK's
//! `settings.ini` and `gtk.css`, and fontconfig defaults, resolved into one
//! [`Tokens`] set over Kern's own design tokens ([`kern_tokens`]) and emitted
//! as CSS custom properties by [`to_css`]. Contrast follows WCAG 2.1
//! (see [`color`]).
//!
//! Parsing never fails: an unusable value is left out. [`resolve`] is pure.

pub mod color;
mod css;
mod error;
mod gather;
pub mod kern_tokens;
mod portal;
mod resolve;
mod sources;
mod watch;

pub use color::{contrast_ratio, Rgba, WCAG_AA_BODY};
pub use css::to_css;
pub use error::ThemeError;
pub use gather::{gather, Gathered};
pub use kern_tokens::{Density, KernAccent, Scheme};
pub use portal::read_portal;
pub use resolve::{resolve, Follow, Mode, Origin, Tokens, UserChoice};
pub use sources::{
    fc_default, fc_resolves, parse_gtk, parse_hyprtoolkit, read_gtk, read_hyprtoolkit, Fontconfig,
    Gtk, HyprToolkit, Portal, Sources,
};
pub use watch::{watch, watch_with, Watcher, DEBOUNCE};
