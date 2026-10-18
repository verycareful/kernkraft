// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Copying extended attributes (xattr(7)), POSIX ACLs among them: Linux
//! stores a file's access and default ACLs as the `system.posix_acl_access`
//! and `system.posix_acl_default` attributes.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::path::Path;

use rustix::fs::{fsetxattr, getxattr, listxattr, XattrFlags};
use rustix::io::Errno;

/// Every attribute of `from` set on `to`, which is open for writing.
///
/// Returns the names `to` refused because its filesystem does not hold
/// them (`EOPNOTSUPP`) or this process may not set them (`EPERM`, as for
/// `security.capability` without `CAP_SETFCAP`). Any other failure is an
/// error. An attribute removed from `from` while it is copied is not
/// copied.
pub(crate) fn copy_all(from: &Path, to: &File) -> io::Result<Vec<String>> {
    let mut refused = Vec::new();
    for name in names(from)? {
        let Some(value) = value(from, &name)? else {
            continue;
        };
        match fsetxattr(to, name.as_c_str(), &value, XattrFlags::empty()) {
            Ok(()) => {}
            Err(Errno::OPNOTSUPP | Errno::PERM) => {
                refused.push(name.to_string_lossy().into_owned());
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(refused)
}

/// The attribute names of `path`. A filesystem without attributes has none.
fn names(path: &Path) -> io::Result<Vec<CString>> {
    let list = read_sized(|buf| listxattr(path, buf))?.unwrap_or_default();
    Ok(list
        .split(|&b| b == 0)
        .filter(|n| !n.is_empty())
        .filter_map(|n| CString::new(n).ok())
        .collect())
}

/// The value of attribute `name` of `path`; `None` if it is gone.
fn value(path: &Path, name: &CStr) -> io::Result<Option<Vec<u8>>> {
    read_sized(|buf| getxattr(path, name, buf))
}

/// Calls `read` with an empty buffer for the size, then with a buffer of
/// that size, again if the value grew in between. `ENODATA` (gone) and
/// `EOPNOTSUPP` (no attributes here) are `None`.
fn read_sized(
    read: impl Fn(&mut [u8]) -> rustix::io::Result<usize>,
) -> io::Result<Option<Vec<u8>>> {
    loop {
        let size = match read(&mut []) {
            Ok(n) => n,
            Err(Errno::NODATA | Errno::OPNOTSUPP) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut buf = vec![0u8; size];
        match read(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                return Ok(Some(buf));
            }
            Err(Errno::RANGE) => continue,
            Err(Errno::NODATA) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
}
