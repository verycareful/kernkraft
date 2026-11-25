// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

package dev.kern.pdfbridge;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Drives the JNI exports the way Kern Android does, for scripts/check-jni.sh:
 *
 *     Oracle LIBRARY PDF OUT_PREFIX
 *
 * Opens the PDF, draws its first page 400 pixels wide into a direct buffer
 * (OUT_PREFIX.ppm), turns the first page 90 degrees, writes the update to
 * append (OUT_PREFIX.update), confirms it and closes. Prints the answers it
 * checked, one per line, and exits with 1 on the first answer that is not ok.
 */
public final class Oracle {
    private static final int WIDTH_PX = 400;
    private static final String DATE = "D:20261121103000Z";

    public static void main(String[] args) throws IOException {
        System.load(Path.of(args[0]).toAbsolutePath().toString());
        String pdf = Path.of(args[1]).toAbsolutePath().toString();
        String prefix = args[2];

        String opened = ok(KernkraftPdf.nativeOpen("{\"path\":" + quote(pdf) + "}"));
        long handle = number(opened, "handle");
        System.out.println("open pages=" + number(opened, "pages"));

        String info = ok(KernkraftPdf.nativePageInfo("{\"handle\":" + handle + ",\"page\":1}"));
        System.out.println("page_info rotate=" + number(info, "rotate"));

        String request = "{\"handle\":" + handle + ",\"page\":1,\"width_px\":" + WIDTH_PX + "}";
        String size = ok(KernkraftPdf.nativeRenderSize(request));
        int width = (int) number(size, "width");
        int height = (int) number(size, "height");
        ByteBuffer pixels = ByteBuffer.allocateDirect((int) number(size, "bytes"));
        ok(KernkraftPdf.nativeRender(request, pixels));
        writePpm(Path.of(prefix + ".ppm"), pixels, width, height);
        System.out.println("render " + width + "x" + height);

        ok(KernkraftPdf.nativeText("{\"handle\":" + handle + ",\"page\":1}"));
        ok(KernkraftPdf.nativeLinks("{\"handle\":" + handle + ",\"page\":1}"));
        ok(KernkraftPdf.nativeAnnotations("{\"handle\":" + handle + ",\"page\":1}"));

        ok(KernkraftPdf.nativeApply("{\"handle\":" + handle
            + ",\"op\":{\"op\":\"rotate\",\"pages\":[1],\"degrees\":90}}"));
        String appended = ok(KernkraftPdf.nativeAppendSince("{\"handle\":" + handle
            + ",\"base_length\":" + Files.size(Path.of(pdf))
            + ",\"out\":" + quote(prefix + ".update") + ",\"modified\":\"" + DATE + "\"}"));
        System.out.println("append bytes=" + number(appended, "appended"));
        ok(KernkraftPdf.nativeConfirmAppended("{\"handle\":" + handle
            + ",\"length\":" + number(appended, "length") + "}"));

        // A job cancelled before its request begins stops the request, and
        // the compacted copy it would have made is not there.
        String copy = prefix + ".compact";
        String asked = ok(KernkraftPdf.nativeCancelJob("{\"job\":1}"));
        if (!asked.contains("\"running\":false")) {
            throw new IllegalStateException("a job that has not begun was running: " + asked);
        }
        String stopped = KernkraftPdf.nativeCompact("{\"handle\":" + handle
            + ",\"out\":" + quote(copy) + ",\"job\":1}");
        if (!stopped.contains("\"kind\":\"Cancelled\"") || Files.exists(Path.of(copy))) {
            throw new IllegalStateException("a cancelled compact was not stopped: " + stopped);
        }
        ok(KernkraftPdf.nativeJobProgress("{\"job\":1}"));
        System.out.println("cancelled compact stopped");

        ok(KernkraftPdf.nativeClose("{\"handle\":" + handle + "}"));

        // A failure comes back as an answer, never as an exception.
        String refused = KernkraftPdf.nativeText("{\"handle\":" + handle + ",\"page\":1}");
        if (!refused.contains("\"kind\":\"UnknownHandle\"")) {
            throw new IllegalStateException("a closed handle was not refused: " + refused);
        }
        System.out.println("closed handle refused");
    }

    private static String ok(String answer) {
        if (answer == null || !answer.contains("\"ok\":true")) {
            System.err.println("not ok: " + answer);
            System.exit(1);
        }
        return answer;
    }

    private static long number(String json, String field) {
        Matcher match = Pattern.compile("\"" + field + "\":(\\d+)").matcher(json);
        if (!match.find()) {
            throw new IllegalStateException(field + " is not in " + json);
        }
        return Long.parseLong(match.group(1));
    }

    private static String quote(String text) {
        return "\"" + text.replace("\\", "\\\\").replace("\"", "\\\"") + "\"";
    }

    private static void writePpm(Path path, ByteBuffer rgba, int width, int height) throws IOException {
        byte[] header = ("P6\n" + width + " " + height + "\n255\n").getBytes(StandardCharsets.US_ASCII);
        byte[] out = new byte[header.length + width * height * 3];
        System.arraycopy(header, 0, out, 0, header.length);
        int at = header.length;
        for (int pixel = 0; pixel < width * height; pixel++) {
            out[at++] = rgba.get(pixel * 4);
            out[at++] = rgba.get(pixel * 4 + 1);
            out[at++] = rgba.get(pixel * 4 + 2);
        }
        Files.write(path, out);
    }
}
