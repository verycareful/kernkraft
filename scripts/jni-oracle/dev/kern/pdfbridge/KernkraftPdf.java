// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

package dev.kern.pdfbridge;

import java.nio.ByteBuffer;

/**
 * The native declarations of the exports in libkernkraft_jni, as Kern Android
 * declares them. scripts/check-jni.sh compiles this beside Oracle.java and
 * calls the exports through a JVM.
 */
final class KernkraftPdf {
    static native String nativeMerge(String request);
    static native String nativeSplitRanges(String request);
    static native String nativeExtract(String request);
    static native String nativeOpen(String request);
    static native String nativeClose(String request);
    static native String nativePageInfo(String request);
    static native String nativeText(String request);
    static native String nativeSearch(String request);
    static native String nativeLinks(String request);
    static native String nativeAnnotations(String request);
    static native String nativeApply(String request);
    static native String nativeUndo(String request);
    static native String nativeRedo(String request);
    static native String nativeAppendSince(String request);
    static native String nativeConfirmAppended(String request);
    static native String nativeCompact(String request);
    static native String nativeSetSystemFonts(String request);
    static native String nativeRenderSize(String request);
    static native String nativeRender(String request, ByteBuffer pixels);

    private KernkraftPdf() {}
}
