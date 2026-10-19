// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The xdg-desktop-portal appearance settings (parent spec section 6.5,
//! source 2), read over the session bus.
//!
//! Follows the xdg-desktop-portal Settings interface
//! (`org.freedesktop.portal.Settings`, `ReadOne` from interface version 2 and
//! `Read` before it) and the `org.freedesktop.appearance` namespace:
//! `color-scheme` (`u`: 0 no preference, 1 prefer dark, 2 prefer light),
//! `accent-color` (`(ddd)`, each component in 0..1, out of range meaning
//! unset) and `contrast` (`u`: 0 no preference, 1 more contrast).

use zbus::blocking::Connection;
use zbus::zvariant::{Structure, Value};
use zbus::DBusError;

use crate::color::Rgba;
use crate::error::ThemeError;
use crate::kern_tokens::Scheme;
use crate::sources::Portal;

pub(crate) const PORTAL_BUS_NAME: &str = "org.freedesktop.portal.Desktop";
pub(crate) const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
pub(crate) const SETTINGS_IFACE: &str = "org.freedesktop.portal.Settings";
pub(crate) const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";

/// `color-scheme` value for "prefer dark".
const SCHEME_DARK: u32 = 1;
/// `color-scheme` value for "prefer light".
const SCHEME_LIGHT: u32 = 2;
/// `contrast` value for "more contrast".
const CONTRAST_HIGH: u32 = 1;

/// D-Bus error names (D-Bus specification, "Message Bus Message Protocol",
/// and xdg-desktop-portal's `org.freedesktop.portal.Error.NotFound`).
const ERR_SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";
const ERR_NAME_HAS_NO_OWNER: &str = "org.freedesktop.DBus.Error.NameHasNoOwner";
const ERR_UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const ERR_PORTAL_NOT_FOUND: &str = "org.freedesktop.portal.Error.NotFound";

/// Maps a `color-scheme` value: 1 dark, 2 light, anything else (0, no
/// preference, and values a future portal may add) is `None`.
pub(crate) fn decode_scheme(value: u32) -> Option<Scheme> {
    match value {
        SCHEME_DARK => Some(Scheme::Dark),
        SCHEME_LIGHT => Some(Scheme::Light),
        _ => None,
    }
}

/// Maps an `accent-color` triple. Every component must lie in 0..=1; any
/// other value (the portal sends out-of-range numbers for "no accent") is
/// `None`.
pub(crate) fn decode_accent((r, g, b): (f64, f64, f64)) -> Option<Rgba> {
    let channel = |x: f64| (0.0..=1.0).contains(&x).then(|| (x * 255.0).round() as u8);
    Some(Rgba::rgb(channel(r)?, channel(g)?, channel(b)?))
}

/// Strips variant wrappers. `Read` (interface version 1) wraps its reply in
/// one variant more than `ReadOne` does.
fn peel<'a, 'b>(mut v: &'a Value<'b>) -> &'a Value<'b> {
    while let Value::Value(inner) = v {
        v = inner;
    }
    v
}

pub(crate) fn value_u32(v: &Value<'_>) -> Option<u32> {
    match peel(v) {
        Value::U32(n) => Some(*n),
        _ => None,
    }
}

fn value_f64(v: &Value<'_>) -> Option<f64> {
    match peel(v) {
        Value::F64(x) => Some(*x),
        _ => None,
    }
}

pub(crate) fn value_triple(v: &Value<'_>) -> Option<(f64, f64, f64)> {
    let Value::Structure(s) = peel(v) else {
        return None;
    };
    let f: &[Value<'_>] = Structure::fields(s);
    match f {
        [a, b, c] => Some((value_f64(a)?, value_f64(b)?, value_f64(c)?)),
        _ => None,
    }
}

/// The D-Bus error name of `e`, when it is a reply from the peer.
fn error_name(e: &zbus::Error) -> Option<String> {
    match e {
        zbus::Error::MethodError(name, _, _) => Some(name.to_string()),
        zbus::Error::FDO(f) => Some(f.name().to_string()),
        _ => None,
    }
}

fn portal_err(what: &str, e: &zbus::Error) -> ThemeError {
    ThemeError::Portal(format!("{what}: {e}"))
}

/// `true` when `e` says there is no portal service on the bus.
pub(crate) fn is_no_portal(e: &zbus::Error) -> bool {
    matches!(
        error_name(e).as_deref(),
        Some(ERR_SERVICE_UNKNOWN | ERR_NAME_HAS_NO_OWNER)
    )
}

/// Outcome of asking for one key.
enum Read<T> {
    NoPortal,
    Unset,
    Got(T),
}

/// Reads one key of the appearance namespace and decodes it with `decode`.
/// Tries `ReadOne`; a portal that does not know it (interface version 1)
/// is asked with `Read` instead, and `legacy` remembers that for the keys
/// that follow. A key the portal does not have is `Unset`.
fn read_key<T>(
    conn: &Connection,
    key: &str,
    legacy: &mut bool,
    decode: impl Fn(&Value<'_>) -> Result<T, ThemeError>,
) -> Result<Read<T>, ThemeError> {
    loop {
        let method = if *legacy { "Read" } else { "ReadOne" };
        let reply = conn.call_method(
            Some(PORTAL_BUS_NAME),
            PORTAL_PATH,
            Some(SETTINGS_IFACE),
            method,
            &(APPEARANCE_NAMESPACE, key),
        );
        let msg = match reply {
            Ok(m) => m,
            Err(e) if is_no_portal(&e) => return Ok(Read::NoPortal),
            Err(e) if !*legacy && error_name(&e).as_deref() == Some(ERR_UNKNOWN_METHOD) => {
                *legacy = true;
                continue;
            }
            Err(e) if error_name(&e).as_deref() == Some(ERR_PORTAL_NOT_FOUND) => {
                return Ok(Read::Unset)
            }
            Err(e) => return Err(portal_err(&format!("{method} {key}"), &e)),
        };
        let body = msg.body();
        let value: Value<'_> = body
            .deserialize()
            .map_err(|e| portal_err(&format!("{method} {key} reply"), &e))?;
        return decode(&value).map(Read::Got);
    }
}

fn wrong_type(key: &str) -> ThemeError {
    ThemeError::Portal(format!("{key}: value has an unexpected type"))
}

/// Reads `color-scheme`, `accent-color` and `contrast` from the portal.
///
/// `Ok(None)` when no portal is on the bus (`ServiceUnknown` or
/// `NameHasNoOwner`). A key the portal does not provide is left unset. Any
/// other D-Bus error, or a value of the wrong type, is an error. The calls
/// block for the connection's method timeout, if it has one.
pub fn read_portal(conn: &Connection) -> Result<Option<Portal>, ThemeError> {
    let mut legacy = false;
    let scheme = match read_key(conn, "color-scheme", &mut legacy, |v| {
        value_u32(v).ok_or_else(|| wrong_type("color-scheme"))
    })? {
        Read::NoPortal => return Ok(None),
        Read::Unset => None,
        Read::Got(n) => decode_scheme(n),
    };
    let accent = match read_key(conn, "accent-color", &mut legacy, |v| {
        value_triple(v).ok_or_else(|| wrong_type("accent-color"))
    })? {
        Read::NoPortal => return Ok(None),
        Read::Unset => None,
        Read::Got(t) => decode_accent(t),
    };
    let high_contrast = match read_key(conn, "contrast", &mut legacy, |v| {
        value_u32(v).ok_or_else(|| wrong_type("contrast"))
    })? {
        Read::NoPortal => return Ok(None),
        Read::Unset => false,
        Read::Got(n) => n == CONTRAST_HIGH,
    };
    Ok(Some(Portal {
        color_scheme: scheme,
        accent,
        high_contrast,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use zbus::zvariant::StructureBuilder;

    #[test]
    fn portal_values_decode() {
        // Portal spec: 0 none, 1 dark, 2 light; anything else is unset.
        assert_eq!(decode_scheme(0), None);
        assert_eq!(decode_scheme(1), Some(Scheme::Dark));
        assert_eq!(decode_scheme(2), Some(Scheme::Light));
        assert_eq!(decode_scheme(3), None);
        // 0.5 of 255 is 127.5, which rounds up to 128.
        assert_eq!(decode_accent((1.0, 0.0, 0.5)), Some(Rgba::rgb(255, 0, 128)));
        assert_eq!(decode_accent((0.0, 0.0, 0.0)), Some(Rgba::rgb(0, 0, 0)));
        // Out of range in any component, or not a number, means unset.
        assert_eq!(decode_accent((-1.0, -1.0, -1.0)), None);
        assert_eq!(decode_accent((0.5, 1.5, 0.5)), None);
        assert_eq!(decode_accent((f64::NAN, 0.0, 0.0)), None);
    }

    fn triple(a: f64, b: f64, c: f64) -> Value<'static> {
        Value::Structure(
            StructureBuilder::new()
                .add_field(a)
                .add_field(b)
                .add_field(c)
                .build()
                .unwrap(),
        )
    }

    #[test]
    fn variant_levels_are_peeled() {
        let once = Value::Value(Box::new(Value::U32(2)));
        let twice = Value::Value(Box::new(once.try_clone().unwrap()));
        assert_eq!(value_u32(&Value::U32(2)), Some(2));
        assert_eq!(value_u32(&once), Some(2));
        assert_eq!(value_u32(&twice), Some(2));
        assert_eq!(value_u32(&Value::F64(2.0)), None);
        let t = Value::Value(Box::new(triple(0.25, 0.5, 0.75)));
        assert_eq!(value_triple(&t), Some((0.25, 0.5, 0.75)));
        assert_eq!(value_triple(&Value::U32(1)), None);
        let two = Value::Structure(
            StructureBuilder::new()
                .add_field(0.1f64)
                .add_field(0.2f64)
                .build()
                .unwrap(),
        );
        assert_eq!(value_triple(&two), None);
        let mixed = Value::Structure(
            StructureBuilder::new()
                .add_field(0.1f64)
                .add_field(1u32)
                .add_field(0.2f64)
                .build()
                .unwrap(),
        );
        assert_eq!(value_triple(&mixed), None);
    }
}
