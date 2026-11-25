// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The `Java_dev_kern_pdfbridge_KernkraftPdf_*` symbols. Each reads its
//! request string, calls the function of this crate that does the work and
//! returns its answer as a Java string. The work, and every panic net, is
//! in the functions they call: these are shells.

use jni::objects::{JByteBuffer, JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use kernkraft_pdf_engine::WireError;

use crate::bridge::bridge;
use crate::guard::respond;
use crate::stateless;

/// Reads `request` and gives the answer of `operation` to it. A request
/// string the JVM cannot give is answered as `BadRequest`; an answer the JVM
/// cannot take leaves a pending exception for Kotlin and returns null.
fn answer(
    env: &mut JNIEnv<'_>,
    request: &JString<'_>,
    operation: impl FnOnce(&str) -> String,
) -> jstring {
    let reply = match read(env, request) {
        Ok(text) => operation(&text),
        Err(error) => respond(|| Err(error)),
    };
    give(env, &reply)
}

fn read(env: &mut JNIEnv<'_>, request: &JString<'_>) -> Result<String, WireError> {
    env.get_string(request).map(String::from).map_err(|error| {
        WireError::bad_request(format!("the request string could not be read: {error}"))
    })
}

fn give(env: &mut JNIEnv<'_>, reply: &str) -> jstring {
    match env.new_string(reply) {
        Ok(string) => string.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Defines the export `$symbol`, which answers `$operation` applied to the
/// request string.
macro_rules! export {
    ($symbol:ident, $operation:expr) => {
        #[no_mangle]
        pub extern "system" fn $symbol<'local>(
            mut env: JNIEnv<'local>,
            _class: JClass<'local>,
            request: JString<'local>,
        ) -> jstring {
            answer(&mut env, &request, $operation)
        }
    };
}

export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeMerge,
    stateless::merge_files
);
export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeSplitRanges,
    stateless::split_by_ranges
);
export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeExtract,
    stateless::extract_pages
);
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeOpen, |r| bridge(
)
.open(r));
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeClose, |r| {
    bridge().close(r)
});
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativePageInfo, |r| {
    bridge().page_info(r)
});
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeText, |r| bridge(
)
.text(r));
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeSearch, |r| {
    bridge().search(r)
});
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeLinks, |r| {
    bridge().links(r)
});
export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeAnnotations,
    |r| bridge().annotations(r)
);
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeApply, |r| {
    bridge().apply(r)
});
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeUndo, |r| bridge(
)
.undo(r));
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeRedo, |r| bridge(
)
.redo(r));
export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeAppendSince,
    |r| bridge().append_since(r)
);
export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeConfirmAppended,
    |r| bridge().confirm_appended(r)
);
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeCompact, |r| {
    bridge().compact(r)
});
export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeSetSystemFonts,
    |r| bridge().set_system_fonts(r)
);
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeCancelJob, |r| {
    bridge().cancel_job(r)
});
export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeJobProgress,
    |r| { bridge().job_progress(r) }
);
export!(
    Java_dev_kern_pdfbridge_KernkraftPdf_nativeSetCmykProfile,
    |r| bridge().set_cmyk_profile(r)
);
export!(Java_dev_kern_pdfbridge_KernkraftPdf_nativeRenderSize, |r| {
    bridge().render_size(r)
});

/// Draws a page into a direct `ByteBuffer`. The application allocates it
/// after `renderSize` and holds it, unused by any other thread, until this
/// returns.
#[no_mangle]
pub extern "system" fn Java_dev_kern_pdfbridge_KernkraftPdf_nativeRender<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    request: JString<'local>,
    buffer: JByteBuffer<'local>,
) -> jstring {
    let reply = match read(&mut env, &request) {
        Err(error) => respond(|| Err(error)),
        Ok(text) => match direct(&mut env, &buffer) {
            Err(error) => respond(|| Err(error)),
            Ok((address, capacity)) => {
                // SAFETY: the JVM guarantees that `capacity` bytes from
                // `address` belong to `buffer` while the buffer object is
                // alive, and `buffer` is a local reference that is alive
                // until this function returns. The application does not
                // touch the buffer during the call (documented on this
                // export), so nothing else reads or writes these bytes.
                let pixels = unsafe { std::slice::from_raw_parts_mut(address, capacity) };
                bridge().render(&text, pixels)
            }
        },
    };
    give(&mut env, &reply)
}

/// The address and capacity of a direct buffer.
fn direct(env: &mut JNIEnv<'_>, buffer: &JByteBuffer<'_>) -> Result<(*mut u8, usize), WireError> {
    let not_direct = |error: jni::errors::Error| {
        WireError::bad_request(format!("the buffer is not a direct ByteBuffer: {error}"))
    };
    let address = env.get_direct_buffer_address(buffer).map_err(not_direct)?;
    let capacity = env.get_direct_buffer_capacity(buffer).map_err(not_direct)?;
    Ok((address, capacity))
}
