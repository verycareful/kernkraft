# Changelog - kernkraft

All notable changes documented here. Format: [Keep a Changelog](https://keepachangelog.com/), with one deviation: dated release entries are timestamped to the minute with timezone (`YYYY-MM-DD HH:MM (TZ)`).

## [0.2.3.0] - 2026-11-26 19:11 (IST)

The size and place of an existing block of text can be set by an edit: both apps drag both sides of a block's box. Still alpha; nothing existing changes except as listed under Changed.

### Added
- **`left` and `width` on an edit of text**: `kernkraft-pdf`'s `TextEdit` gains `with_left` and `with_width`, and the `editText` op of `kernkraft-pdf-engine` gains the optional fields `left` and `width`. Both are fractions of the displayed page's width, the units of `addText`'s `at` and `width`. `width` is the width the text is broken to (it was the width of the block's widest line), and where a right-to-left paragraph ends: the start of its leftmost line plus the width. `left` is where the left edge of the block's rectangle stands: the whole block moves sideways by the distance from its rectangle's left edge to `left`, the first glyph keeping its place in the block, so a `left` read from the text layer moves nothing. A `width` alone keeps the left edge and a `left` alone keeps the width. The vertical position never changes. Both are measured on the displayed page: a turned page (`Rotate` 90, 180 and 270), a scaled or skewed text matrix and a form drawn under a `cm` are carried through the matrices to the units the lines are set in. With neither, an edit is as it was, byte for byte, and an empty text, which deletes the block, ignores both. A `left` or `width` that is not finite, a `width` of 0 or less, or a box wholly to the left or to the right of the page is `PdfError::Invalid`, which is kind `Invalid` on the wire as for `addText`; an unknown field is `BadRequest`, as every op's is. No adapter changes: `kernkraft-tauri` and `kernkraft-jni` pass the op as JSON.
  - Limit: an edit sets every line from the first glyph's origin, as it always did, so for a block whose first glyph is not its leftmost (an indented first line, or right-to-left text) the new rectangle's left can differ from the `left` asked for by that indent.

### Changed
- `Op::EditText` gains `left` and `width`, so Rust code that builds the op by hand needs the two fields. `TextEdit` is `non_exhaustive`, so its new fields break nobody.

### Tests
- `crates/kernkraft-pdf/tests/edit_text/edit.rs` (9): a width breaks the text at 72, 31 and 29 points, a width wider than the block's, a `left` moves every line by one distance and nothing else (against the plain edit's bytes), both, a `left` equal to the block's own is no move, a `left` moves by the rectangle's distance and not the first glyph's, an empty text ignores the box, nine boxes that are refused, and a box partly off the page is allowed.
- `rotated.rs` (3): `left` and `width` on a page turned 90, 180 and 270 with upright text, and on a page not turned. `bidi.rs` (4): a right-to-left line follows a wider box's right edge, `left` moves the block as a whole, both, and a narrower box breaks a paragraph with its right edge in the box. `form.rs` (1): a block in a form drawn under a `cm`, the form object unchanged.
- `crates/kernkraft-pdf-engine/tests/edit.rs` (7, new): the op parses with both fields, absent fields are unset, an unknown or mistyped field is refused, a width changes the lines, a `left` puts the block's left edge there, a box off the page is `Invalid` and changes nothing, and an empty text ignores the box.
- `crates/kernkraft-jni/tests/edit.rs` (2): both fields as JSON through the bridge, and a box off the page is `Invalid` while an unknown field is `BadRequest`.

### Handover
For Kern Desktop and Kern Android, tag `0.2.3.0`. The op: `{"op":"editText","page":1,"block":3,"revision":12,"text":"...","left":0.1,"width":0.5}`, `left` and `width` optional. Send the `rect` left the text read gave as `left` to move nothing; the left side of a box dragged sets `left`, the right side sets `width` (the new right edge minus `left`, or minus the rectangle's left when `left` is not sent). Kern Desktop bumps the five `tag =` lines in `src-tauri/Cargo.toml`, runs `tools/lock-git-form.sh`, adds `left` and `width` to `editText` in `src/ipc/pdf.ts` and builds the block box's two side handles. Type 3 text is still refused (`Type3Font`), so the apps keep their message for it.

### Results
- 2727 tests across 109 suites, all passed (305s, CachyOS x86-64, rustc 1.99.1).

## [0.2.2.0] - 2026-11-26 16:34 (IST)

What Kern Android's PDF editor needs from the engine: new text on a page, the fonts of a page, which lines of a block wrapped, and a substitute-font note that is no longer empty. Still alpha; nothing existing changes except as listed under Changed.

### Added
- **`add_text`, new text on a page**: `kernkraft-pdf` gains `add_text(document, page, follow, new, progress)`, with `Follow` (`Nearest(&TextPage)` or `Block(&TextBlock)`), `NewText` (the text, `at`, `width`, a `TextStyle` and the font list) and `TextAdded` (the document, the lines, the `family` used, the block followed and the font substitutions). The box is `at`, its top left corner, and `width`, as fractions of the displayed page; the lines grow downward from the top, the first baseline the face's ascent below it, and the text stands upright on a turned page. The text is a `q ... Q` in a content stream of its own, added after the page's last stream, so the page's streams are not changed. Its font, size and colour are those of the text block nearest the box (the nearest block that can be edited, is horizontal and is not invisible), or of the block `Follow::Block` names: the block's font is used again when it can show the text, otherwise the list face of its family is embedded as a subset (a `FontSubstituted` diagnostic and a `substitutions` entry); the size is the one the reader sees, the font size carried through the text matrix and the CTM; the colour is converted to RGB through the document's colour spaces. `TextStyle` replaces the family, the size or the colour. A page with no text to follow takes the list face closest to Helvetica in black and needs the size (`PdfError::NoSizeForNewText`). A character no face has is `NoGlyphForCharacter`, and a block named by the caller that cannot be followed is `TextNotEditable`. Limits: the text is not in a tagged file's structure tree; it is one font, one size and one colour, with no shaping, as an edit is; a page whose content streams leave the graphics state changed places it by that state; a block painted with a pattern is refused until a colour is named.
  - `kernkraft-pdf-engine`: the op `addText {page, at, width, text, like?, family?, size?, colour?}` (`like` is `{block, revision}`, and a revision that is not current is `StaleBlock`), one undo step. The answer adds `lines`, `family`, `followed` (a block id, or null when no block was followed) and `notes`.
- **`block_style` and `blocks_by_distance`**: the font, whether it is embedded, the size on the page and the colour new text would take from a block, and the blocks of a page in order of their distance from a point.
- **`pageFonts`, the fonts of a page**: a read of `kernkraft-pdf-engine` (`Engine::page_fonts`) and the native `nativePageFonts` of `kernkraft-jni`. Each entry has the font, whether it is embedded, the size, the colour, the ids of the blocks that use it and their characters, most used first; with `near`, the entry whose block `addText` would follow there is marked `nearest`. Sizes are grouped to a hundredth of a point. It reads each block's font by running the page's content, which takes about 28 ms a page on the ISO 32000 specification, so it is a read of its own and `text` stays as fast. `kernkraft-tauri` has no command for it yet.
- **`TextBlock::wrapped_lines` and `wrapped` in `text`**: for each line of a block, whether it ended where the text wrapped. A PDF holds only where each glyph was drawn, so it is a judgement from the geometry: a line is a wrap when it stops short of the block's end edge (the left edge in right-to-left text) by less than the first word of the next line and a space would take. A block with one line its writer certainly ended (the next word would have fitted, or the next line starts with a list marker) has no wrapped line, as an address or a list has none, and neither has a block that does not run horizontally in one direction; the last line is never one. Centred and right-aligned text, and a block of two lines whose first is the longer, read as lines or as a two-line paragraph. A block is what the text layer makes of the lines: lines further apart than 1.7 times their size are blocks of their own, so on a double-spaced page every line is a block and every `wrapped` is false.

### Changed
- `Edited` gains `substitutions`, and `FontSubstitution` is new. Direct Rust callers that build an `Edited` need the new field.
- `PdfError` gains `NoSizeForNewText`; a `match` on it with no wildcard needs the arm.

### Fixed
- **The `notes` of an edited block were always empty**: the engine found the fonts an edit replaced by comparing the document's diagnostics before and after, but both shared one log and the edit had already written its diagnostic into it. A repeated substitution of the same font on the same block is also logged once, so a comparison would have missed it. `edit_text` now returns the substitutions it made, and the engine writes `notes` from them: a block of Helvetica edited with letters it cannot show answers a note naming the page's font and the family used, the same edit again answers it again, and an edit in the block's own font answers none.

### Tests
- `crates/kernkraft-pdf/tests/edit_text/add.rs` (18 tests): the font, size and colour of the nearest block, a named block, each part of the style replaced, the first baseline against the ascent, the wrap to the width of the box, the size on a page that scales its text, invisible text passed over, the new stream beside the page's own, a list face embedded when the block's font cannot show the text, the family of a font used again, a page with no text (the size and the list face), a turned page, boxes that are not on a page, the order by distance, and the style of a block.
- `crates/kernkraft-pdf/tests/text/wrapped.rs` and `bidi.rs`: a paragraph set to a column, a line that stops where the next word fits, an address, lists marked with a dash, `1.` and `a)`, what is and is not a marker, one line, two lines, a turned page, and a right-to-left paragraph and block.
- `crates/kernkraft-pdf/tests/edit_text/edit.rs`: a substitution is reported with its family, an edit in the block's own font and a named family report none, and the same edit made twice reports the substitution both times though the log keeps it once.
- `crates/kernkraft-jni/tests/edit.rs`: through the bridge, `notes` of an edit that needs a list face (also after an undo), `wrapped` of a paragraph and an address, the fonts of the sample with the nearest marked, one entry for a paragraph of one font, new text on the sample followed by one undo and a redo, a named block with the style replaced, a stale named block, and a page with no text.

### Results
- 2701 tests across 108 suites, all passed (346s, CachyOS x86-64, rustc 1.99.1).

## [0.2.1.0] - 2026-11-25 23:18 (IST)

A file whose cross-reference was rebuilt can now be repaired by an application. Still alpha; one operation is added and nothing existing changes.

### Added
- **`compact_file`, a compacted copy of a file by path**: a file whose cross-reference was rebuilt is refused by `open` with `RebuiltNotEditable`, so it has no session and `compact` could not be reached for the very files the error says to compact. `compact_file` takes `path`, `password`, `out` and an optional `job`, reads the file as `merge` and `extract` do, and writes it whole as the new file `out`, with a cross-reference that is not rebuilt. The source is read and never changed, `out` is created and never replaces a file (`Exists`), and a failure or a cancelled job leaves no file behind. The copy opens as an editable session, so an application repairs a file by writing the copy to a new path and editing that.
  - `kernkraft-pdf-engine`: `stateless::{CompactFileRequest, compact_file}`, which returns `out`.
  - `kernkraft-jni`: the native `nativeCompactFile`, and `Bridge::compact_file` and `stateless::compact_file` as request strings; the answer has `path`, as `compact` does.
  - `kernkraft-tauri`: the command `pdf_compact_file`, `Kernkraft::pdf_compact_file`, and the permission `allow-pdf-compact-file`, which the default permission set grants.

### Tests
- `a_file_with_a_rebuilt_cross_reference_compacts_to_a_new_file_that_opens_for_editing` in `kernkraft-jni`: a copy of `testdata/kern-sample.pdf` whose `startxref` names an offset past the end of the file is refused by `open` with `RebuiltNotEditable`; `compact_file` writes a copy of 2 pages that opens as a session, and the source is byte for byte what it was.
- `compact_of_a_file_never_replaces_a_file_that_is_there` and `compact_of_a_file_that_is_not_there_makes_no_file` in `kernkraft-jni`: a taken `out` is `Exists` and keeps its bytes; a missing source is `NotFound` and makes no file.
- `a_file_with_a_rebuilt_cross_reference_compacts_to_a_new_file_and_the_original_is_untouched` in `kernkraft-tauri`: the same through `pdf_open` and `pdf_compact_file`, with the page count of the copy and a second call refused as `Exists`.
- `a_cancelled_compact_of_a_file_makes_no_file` in `kernkraft-pdf-engine`: a job cancelled before the request begins stops it with `Cancelled` and writes no file.

### Results
- 2659 tests across 108 suites, all passed (326s, CachyOS x86-64, rustc 1.99.1).

## [0.2.0.0] - 2026-11-25 19:15 (IST)

kernkraft reads, edits, writes and renders PDF with code of its own. `lopdf`, MuPDF and the code derived from Qyra are gone, and no part of the PDF stack is ported from another suite: every format is implemented from its specification, cited by clause in the file headers. Three crates are new (`kernkraft-font`, `kernkraft-pdf-engine`, `kernkraft-jni`), `kernkraft-pdf` is rewritten and its API changes throughout, and `kernkraft-tauri` gains the PDF commands. Still alpha: a direct Rust caller of `kernkraft-pdf` must port to the new API.

### Added
- **`kernkraft-pdf`, a PDF library written from ISO 32000-1.** What it holds:
  - **Syntax and sources**: a lexer, parser and serialiser that keep the byte span of every object; `Source` with `MemorySource` and `FileSource` (random access by offset, a file that shrinks is `ExternalModification`); a `Diagnostic` for every defect that is tolerated, so a file is never silently repaired. Table C.1 values passed are diagnostics, the library's own caps (nesting, name and token length, string size, decoded size, operation count, glyphs per page, image pixels) are `LimitExceeded` errors.
  - **Stream filters**: ASCIIHex, ASCII85, RunLength, LZW, Flate (our RFC 1950 framing over `flate2`'s Rust backend) and the TIFF 2 and PNG predictors, with the cap on decoded size an argument of every decode; and the image codecs DCT (JPEG), JPX (JPEG 2000), JBIG2 and CCITT fax, strict where they can be (damage they would repair is an unusable image with a diagnostic).
  - **File structure**: cross-reference tables and streams, hybrid files, object streams, the `Prev` chain, and a rebuilt cross-reference when the file's own is damaged (`XrefRebuilt` says where). The element cap and the entry cap are set by the application (`ReadLimits`, `OpenOptions`), since a desktop affords more than a phone.
  - **Encryption**: the standard security handler, revisions 2 to 6 (RC4 40 and 128 bit, AES-128, AES-256), owner and user passwords tried in the form each revision defines, PDFDocEncoding and our own SASLprep (RFC 4013), crypt filters, files whose attachments only are encrypted (opened with the empty password, attachments locked until `unlock_attachments`). Permission bits are reported, never enforced. An encrypted document is written encrypted.
  - **Document model and writer**: immutable `Document` snapshots over one file with a bounded object cache, `ObjectBuilder` and `ContentBuilder`, the page tree with inherited attributes, name and number trees, text strings and dates. The full writer makes a compacted copy that keeps untouched objects as their original bytes, matches the source's cross-reference form, and renumbers only a sparse file; the incremental writer appends one update of every difference from the opened bytes.
  - **`EditSession`**: snapshots with undo and redo, `save` (the opened bytes plus one update, atomically, with `ModDate` set from a date the caller gives), and `append_since` and `confirm_appended` for an append-only provider. A rebuilt file and an earlier revision view are refused for editing (`RebuiltNotEditable`, `RevisionNotEditable`). An `Info` that is not a dictionary is kept as it is, no `ModDate` is set, and `InfoNotDictionary` says so.
  - **Document features**: page labels, destinations (explicit, named, by string), actions, file specifications, the outline as a flat list in reading order, attachments (the `EmbeddedFiles` tree and file attachment annotations, checked against `Size` and `CheckSum`), an active-content report (scripts, `Launch`, `GoToR`, `GoToE`, `SubmitForm`, `ImportData`, `URI`, XFA), revisions, signature fields with their coverage and DocMDP `P` (no certificate is read and no digest computed), and links with `Document::locate`.
  - **Page operations**: `rotate`, `reorder`, `remove`, `extract`, `split_ranges`, `split_per_page`, `split_by_bookmarks` and `merge`, each taking `&Document` and giving a new one. The outline, destinations, form, tagged structure, page labels and every other reference to a removed page are kept in agreement; a page keeps its label. `merge` carries everything document-wide (outline, named destinations and the other name trees, the form with colliding names renamed, labels, the tag tree, optional content) and reports what it cannot carry.
  - **Fonts, read side**: font dictionaries of every kind, encodings, `Differences`, the 59 predefined CMaps of Table 118 and embedded CMaps, ToUnicode, widths, vertical metrics, Type 3, and the text of a character by the order of 9.10.2. Bundled data: the Adobe Glyph List, the predefined CMaps (about 1.5 MB of read-only data in the binary), and the metrics of the Core 14 fonts.
  - **Content interpreter**: every operator of Table 51 through a `Device` trait, with operator spans back into the stream, forms, marked content and optional content (`BaseState`, `ON`, `OFF`, `AS`, `OCMD` with `P` and `VE`).
  - **Colour**: Device spaces, CalGray, CalRGB, Lab, ICCBased (through `moxcms`), Indexed, Separation, DeviceN and Pattern spaces, and functions of types 0, 2, 3 and 4.
  - **Images**: image XObjects and inline images, stencil masks, colour key, explicit and soft masks, `Matte`, JPX opacity, `Decode`.
  - **Renderer**: `render::render_page` to straight RGBA8 over `tiny-skia`: paths, clips, strokes (a stroke under one device pixel is drawn one pixel wide), text in every rendering mode, Type 3 glyphs, images, shadings of types 1 to 7, tiling and shading patterns, all 15 blend modes, isolated, non-isolated and knockout groups, soft masks, and annotation appearances (`NoRotate` and `NoZoom` honoured). A font with no usable program is drawn from a face of the application's `FontList` matched by name and style; bold and italic are not synthesised and the glyphs are not scaled to the PDF's widths. Content that is not drawn is reported as `NotRendered`, once a page. `Document::page_geometry` and `render::raster_size` give the size every fraction and render is measured against.
  - **Text layer**: `TextExtractor` and `Document::text_page` give blocks of lines of characters, each with the rectangle and the glyph's own `quad` on the displayed page (fractions, y down), in the reading order of the structure tree on a tagged page and by cuts at the widest empty band otherwise. Superscripts and subscripts stay on their line, a fraction bar parts a numerator from the line below, text drawn twice is read once, `ActualText` is read. Search folds case and compatibility forms, takes whole words, and a match may span a line-end hyphen and the break.
  - **Annotations**: `Document::annotations`, and `add_annotation`, `edit_annotation`, `delete_annotation` for highlight, underline, strikeout, ink and note annotations with appearance streams; an edit keeps every entry it does not name; delete takes replies, the pop-up and the structure tree entries with it.
  - **Font embedding**: `FontEmbedder` writes a Type 0 font (CIDFontType2 for TrueType, CIDFontType0 for OpenType CFF) of the glyphs shown, with `ToUnicode`, measured descriptor values and a deterministic subset tag; a variable font or a CFF2 font embeds at the instance asked for, named by Adobe Technical Note 5902.
  - **Text editing**: `Document::text_block_editability`, `delete_text_block` and `edit_text` replace a block's text, set in the block's width with line breaks from UAX #14, in the block's own font when its encoding and program cover the new text, else in a face of the `FontList` (embedded, with a `FontSubstituted` note). Removed glyphs are made up for by `TJ` numbers so the text after them does not move, and the glyphs of the block's own showing operations that lie wholly off the page go with it. A shared stream, form or `Resources` dictionary is copied, never changed in place. Rotated, skewed and mirrored blocks edit on the first glyph's own matrix. A block that cannot be edited says why (`EditBlocker`): no glyphs, not on the page, a Type 3 font, a clipping render mode, a split across streams, vertical writing, an unterminated text object, a change of state before the end of the text, text of no size, a form whose content the structure tree names (`TaggedFormContent`). There is no shaping: ligatures, kerning, combining marks and scripts that join are drawn one character to one glyph.
- **Right-to-left and mixed text**: the text of a page is in logical order (UAX #9 through `unicode-bidi`), each glyph keeping the rectangle where it is drawn, so an application's hit testing and selection must not assume that the characters of a `TextLine` run left to right (`TextLine::visual_index`, `visual_chars`; `Selection` and match rectangles are one rectangle for each stretch of a line). An edit of such text is reordered to visual order and set against the block's right edge, a mirrored bracket in a right-to-left run is set as its mirror image with the character typed as its `ActualText`, and a character with only a best-fit mirror image or none is refused with `PdfError::MirroredGlyphNotSet`. The mirroring table is generated from Unicode 16.0.0 data.
- **Text of no size**: a glyph of font size 0, or under a matrix that leaves it no height or no width, is listed in a block of its own at the end of the page, flagged `TextChar::sizeless` (a point for `rect` and `quad`). A search finds it, a point never lands on it, and an edit of that block is refused with `EditBlocker::NoSize`; its glyphs stay in the file. Direct Rust callers that build a `TextChar` need the new field.
- **The text of TeX fonts**: the glyph names of Computer Modern, the math extension and symbol fonts and the XY-pic fonts' unnumbered names that the Adobe Glyph List lacks (`mapsto`, `parenleftbig`, `vextendsingle`, `braceex`, 132 names) have text. A name that is a letter and a number (`a1`, `d47`) is left as it is, since another font gives it another meaning.
- **DeviceCMYK through a profile**: DeviceCMYK, and every colour that falls back to it, is converted through the first usable CMYK profile of the file's output intents, else the application's (`Document::set_cmyk_profile`, `CmykProfile::{from_bytes, from_file}`), else the bundled `FOGRA39L_coated.icc`, so a printed colour looks printed (cyan is about (0, 161, 228), not (0, 255, 255)). `ColourSpace::DeviceCmyk` now carries its `CmykProfile`; a page's `DefaultCMYK` still comes first. There is no black point compensation and the intent is relative colorimetric, as for ICCBased. Added: `CmykProfile`, `CmykSource`, `BUNDLED_PROFILE`, `Document::{cmyk_profile, device_cmyk, use_cmyk_profile}` and `IccBased::from_profile` taking the profile.
- **`kernkraft-font`, a core crate for font files**: `Program` reads TrueType and OpenType (through `skrifa`), bare CFF (through `read-fonts`) and Type 1 (our reader: PFA, PFB and the PDF `FontFile`, eexec and charstring ciphers, flex, `seac`; nothing is run) behind one interface of glyph lookup (by name, Unicode, `cmap` subtable, built-in code or CID), advances, outlines and metrics. `FontList::index` builds a list from the paths an application hands in by reading only the directory and the small tables, so the 1199 faces of a desktop's font folders index in under half a second. `Query` and `best`, `by_family` and `covering` choose from it by the CSS Fonts Level 4 matching order, with a variable font's named instances as faces and a requested weight matched on the `wght` axis. `subset` reduces an sfnt to the glyphs a document shows, at a chosen instance (`Variations`, `Axis`, `AXIS_WEIGHT`, `AXIS_ITALIC`, `AXIS_SLANT`). WOFF and WOFF2 are not read, a bitmap-only font has no outlines (`FontError::NoOutline`), and a bare CFF with several fonts reads the first.
- **`kernkraft-pdf-engine`, a core crate for a PDF session**: `Engine` holds open documents behind handles that are never reused and gives each adapter the same typed operations (open, close, page info, text, search, links, annotations, apply, undo, redo, save, append, compact, render and its size, system fonts, CMYK profile, merge, extract, split), each taking a request type that denies unknown fields and answering `Fields` or a `WireError` with a `kind`, a `message`, and the `path` and `details` of a filesystem failure. A session keeps one text extractor for each revision of its document. Paths are `PathBuf`, so a file whose name is not UTF-8 opens, merges, splits and saves. The adapter hands in the `OpenOptions`, so each device sets its own limits.
- **Jobs**: the long requests (`text`, `search`, `apply` of a text edit, `save`, `append`, `compact`, `render`, font indexing, `merge`, `extract`, `split`) take an optional `job` number the application chooses. `Engine::cancel_job` stops it and `Engine::job_progress` reports `done`, `total` and `stage`. A cancelled request fails with `Cancelled` and leaves nothing behind: `save`, `append`, `compact` and a new file write nothing, an edit leaves the revision as it was. Closing a document stops its jobs, and a closing window stops its merges, extracts and splits. A cancel that reaches the engine before its request is kept (at most 64) and applies when the job begins. Added: `jobs::{JobId, JobRequest}`, `TextRequest`, and the `job` field of the request types; `Engine::save` and `Engine::text_page` take the new parameters, and the stateless functions take `&Engine`.
- **`kernkraft-jni`, the JNI adapter for Kern Android**: `libkernkraft_jni.so` exports `dev.kern.pdfbridge.KernkraftPdf` with 22 natives (`nativeOpen`, `nativeClose`, `nativePageInfo`, `nativeText`, `nativeSearch`, `nativeLinks`, `nativeAnnotations`, `nativeApply`, `nativeUndo`, `nativeRedo`, `nativeAppendSince`, `nativeConfirmAppended`, `nativeCompact`, `nativeSetSystemFonts`, `nativeSetCmykProfile`, `nativeRenderSize`, `nativeRender`, `nativeMerge`, `nativeSplitRanges`, `nativeExtract`, `nativeCancelJob`, `nativeJobProgress`). Each takes one JSON request string and answers one JSON string, with `"ok":true` or `{"ok":false,"kind","message"}`; a render goes into a direct buffer. A panic is caught and answered as `Panic`. Android limits: 1,048,576 elements to an object and 16,777,216 cross-reference entries. It builds for `aarch64-linux-android`, `armv7-linux-androideabi` and `x86_64-linux-android`. It is the one crate that depends on `jni`; `deny.toml` bans it elsewhere.
- **`kernkraft-tauri` PDF commands**: `pdf_open`, `pdf_close`, `pdf_page_info`, `pdf_text`, `pdf_search`, `pdf_links`, `pdf_annotations`, `pdf_apply`, `pdf_undo`, `pdf_redo`, `pdf_save`, `pdf_compact`, `pdf_set_system_fonts`, `pdf_set_cmyk_profile`, `pdf_render_size`, `pdf_render` (raw bytes), `pdf_merge`, `pdf_extract`, `pdf_split`, `pdf_cancel_job` and `pdf_job_progress`, each with a permission of its own. `pdf_open` answers the file's stamp, taken before the file is read, and `pdf_save` takes an `Expect` (`unchanged`, `nothing`, `anything`) and answers the stamp of what it wrote, so a file replaced in between is refused and never overwritten. A document belongs to the window that opened it and closes with it. Desktop limits: 4,194,304 elements to an object and 67,108,864 cross-reference entries. The font folders are `~/.fonts`, `$XDG_DATA_HOME/fonts` and `fonts` under each `$XDG_DATA_DIRS`.
- **Dependencies**: `flate2` (Rust backend only, no C zlib), `weezl`, `aes`, `cbc`, `rc4`, `sha2`, `unicode-normalization`, `unicode-linebreak`, `unicode-bidi`, `moxcms`, `tiny-skia`, `skrifa`, `read-fonts`, `subsetter`, `zune-jpeg`, `hayro-jpeg2000`, `hayro-jbig2`, `hayro-ccitt`, `jni`, `getrandom` and `jiff`. None is an HTTP, socket or async runtime crate, and `cargo deny` still enforces it.
- **Checks against other tools**: `scripts/check-interop.sh`, `check-page-ops.sh`, `check-session.sh`, `check-render.sh`, `check-annot.sh`, `check-font.sh`, `check-edit-text.sh`, `check-text.sh` and `check-jni.sh` hold what kernkraft writes or draws to `qpdf --check`, `mutool info`, `pdftotext`, `pdffonts` and `pdftoppm`; `check-text.sh` compares word positions with `pdftotext -bbox` through `scripts/compare_bbox.py`, which judges each word along its own axis and allows for turned, leaning, split and regrouped words. Each exits 1 when it checked nothing. The CI job `oracles` runs them on `testdata/`. MuPDF is not a dependency of any crate: `mutool` is only a checking program.
- **Notices** for what is bundled: `NOTICE` lists the Unicode data and `unicode-bidi`, `FOGRA39L_coated.icc`, the TeX glyph names, the Adobe Glyph List, `cmap-resources` and the Core 14 AFM metrics; the profile also carries its licence beside it in `crates/kernkraft-pdf/assets/`.

### Changed
- **Breaking: `kernkraft-pdf` has a new API.** The page operations (`rotate`, `reorder`, `remove`, `extract`, `split_ranges`, `split_per_page`, `split_by_bookmarks`) take a `&Document` and give a `Document`; `merge` takes `&[&Document]`, the `OpenOptions` of the result and a `Progress`; a result is saved with `write::save_compact` or pushed onto an `EditSession`. `Document::open(path, &OpenOptions, password)` replaces `load`.
- **The trash's `DeletionDate` is read through `jiff`** instead of `chrono`: the same 19-character `YYYY-MM-DDThh:mm:ss`, local time from `TZ` or `/etc/localtime`, equal to `date` for named zones and for an unset or invalid `TZ` (UTC).
- **`deny.toml`** bans `jni` except as a dependency of `kernkraft-jni`, and CI runs `cargo deny` on the Android graph of `kernkraft-jni` alone, for the three Android targets.
- **CI** installs `qpdf`, `mupdf-tools`, `poppler-utils`, a headless JDK and `fonts-dejavu-core` for the `oracles` job; `libclang-dev`, `libfontconfig1-dev` and `libfreetype-dev`, which were MuPDF's build, are no longer installed.
- `CONTRIBUTING.md` names `kernkraft-jni` as an adapter crate, and `SECURITY.md` no longer names MuPDF.

### Fixed
- **Extended attributes and POSIX ACLs were lost under `rustix` 1.1.6** (RUSTSEC-2026-0486): `getxattr` and `listxattr` return an empty result for an empty buffer there, so the size query of `kernkraft-fs` read every attribute as empty and an atomic save or a copy dropped them. The size is now asked with `getxattr_query` and `listxattr_query`, and a size of 0 returns an empty value without a read. `kernkraft-fs` passed slices, never a spare-capacity buffer, so the advisory's unsound path was not reached.

### Removed
- The `lopdf` dependency and its re-export (`pub use lopdf`), the functions `load`, `load_bytes`, `save`, `to_bytes`, `page_count` and `flatten_page_tree`, and `PdfError::Parse`.
- MuPDF: the `render` feature, the `mupdf` dependency, `render_page(path, page, width)`, `Renderer`, `RenderedPage`, `page_size_pt`, `MAX_RENDER_DIMENSION` and `PdfError::Render`. `kernkraft_pdf::render` is the new renderer and takes a `&Document`.
- The code derived from Qyra (`pages.rs`, `split.rs` and `merge.rs`) with its copyright lines, and the Qyra sentence of the README's licence section.
- `chrono`.

### Security
- **`rustix` 1.1.6** (RUSTSEC-2026-0486), **`url` 2.5.9** (RUSTSEC-2026-0592, a stack overflow in `Url::origin()` on nested `blob:` URLs, reached only through Tauri's tooling; no code of ours calls it) and **`cookie` 0.18.3** (RUSTSEC-2026-0594). `chrono` is replaced by `jiff` (RUSTSEC-2026-0543, unmaintained).
- **A file cannot make the library panic or run without bound.** The format layers have a mutation test (every truncation and one-byte substitution of a fixture) that asserts a typed error or a success; nesting, element counts, cross-reference entries, decoded stream size, operation counts, glyphs per page, image pixels, mesh elements and render size are capped, and passing a cap is an error, never a shortened result.

### Tests
- **Format layers**, each with integration tests in `tests/` mirroring `src/` and unit tests beside the code, expectations worked out from the clause or from how the fixture is built: syntax (110), filters (120, and 32 for the image codecs, with the CCITT bits written from the T.4 and T.6 tables and the JBIG2 stream of the specification's own examples), file structure (104), encryption (51, with RC4 against RFC 6229, CBC against NIST SP 800-38A, SASLprep against RFC 4013), the document model and writer (65), document features (74), page operations (73, merged outputs checked with `qpdf`), the edit session (51).
- **Fixtures**: `testdata/encrypted/` holds 20 PDFs (the plain file and 19 variants: RC4 40 and 128 bit, V4 with RC4 and AES-128, AES-256 revisions 5 and 6, restricted permissions, cleartext metadata, object streams, no user password, accented and UTF-8 passwords, attachments only), made by `scripts/make-encrypted-fixtures.py`; `testdata/image/` holds codec fixtures made from synthetic images; `testdata/icc/cmyk-grey-by-black.icc` is a CMYK profile whose lightness follows black ink alone, so its colours are derived from its table. The pixel arithmetic of every renderer test is worked out on the numbers of the page.
- **Fonts**: `kernkraft-font` (with fonts written table by table: a Type 1 with its own copy of the cipher, a TrueType, a bare name-keyed and CID-keyed CFF, a collection, variable fonts with `fvar` and `gvar`), PDF fonts (87), the glyph-name tables, and font embedding read back through the library's own reader and drawn.
- **Content, colour, images, rendering**: the content interpreter (164, arithmetic of 9.4.4 by hand), colour spaces, functions and ICC (68 and 23 for CMYK: a damaged output intent is reported once and the conversion goes on, the application's profile loses to the file's), images (51), the renderer (paths, geometry, text and robustness 67, shadings 27, patterns 14, transparency 35 with every blend mode's formula worked out, annotations 18, embedded fonts 3, corpus files drawn at the size their boxes give).
- **Text**: the text layer (83, with the structure tree, search, links and destinations), logical order (the algorithm exhaustively over every level sequence to length 6 against `reorder_visual`, and layout of Hebrew on a font with named glyphs), text of no size (6), ActualText, a fraction bar and scripts, a vertical stack of one letter, an `Info` that is not a dictionary (4), annotations (56 and the renderer's 8), text editing (editability, removal, replacement, forms, rotated, skewed and mirrored matrices, off-page glyphs, right-to-left and mixed text, the session), each derived from Courier's 600 units at 10 points on a 612 by 792 point page.
- **Adapters**: `kernkraft-pdf-engine` (the session registry, one extractor for each revision, errors with their path, saves against a changed file, and the job table: a cancelled save, append, compact, merge, extract, split, render, text, search, text edit and font index each stop with `Cancelled` and leave nothing); `kernkraft-jni` (every export through its request string, a JVM oracle driving the host `.so` as Kern Android does); `kernkraft-tauri` (the commands as the webview writes them, window close).
- `copy_keeps_an_empty_user_xattr` in `kernkraft-fs`: an empty attribute is copied as an empty attribute.
- `scripts/test_compare_bbox.py` (50 tests of the text comparison, each worked out from the page sizes and tolerances).

### Results
- 2654 tests across 110 suites, all passed (269.4s, CachyOS x86-64, rustc 1.99.1).

## [0.1.3.1] - 2026-11-09 23:40 (IST)

A program started from an AppImage no longer inherits the AppImage's environment. Still alpha; no API changes.

### Fixed
- **A terminal, or any other program, started from an AppImage could fail to start**: the AppImage launcher points the environment at the bundle (`PYTHONHOME`, `PYTHONPATH`, `LD_LIBRARY_PATH`, `PERLLIB`, `QT_PLUGIN_PATH`, the GTK, GIO and GStreamer paths, and a bundle entry at the front of `PATH` and `XDG_DATA_DIRS`), and every program started from there inherited it. A program that embeds Python, kitty for one, stopped at start-up with `Failed to import encodings module` because `PYTHONHOME` named a directory with no standard library, while the application reported the launch as done. Programs started by `launch`, `launch_in_terminal`, `launch_groups`, `open_default` and `open_terminal`, and `xdg-mime`, now get the environment without the bundle.

### Changed
- **How the bundle is left**: when `APPDIR` is set, a variable whose entries all lie inside the bundle is removed, and in a list variable (separated by `:`) the entries inside the bundle are dropped and the rest kept in order, so a path of the user's own survives. `APPDIR`, `APPIMAGE`, `ARGV0` and `OWD` are removed. Nothing else is touched, and outside a bundle `APPDIR` is unset and the environment passes through as it is.

### Tests
- `a_started_program_does_not_inherit_the_bundle`: a program started under a bundle environment records the environment it got, with the bundle variables gone, `PATH` and `XDG_DATA_DIRS` shortened to the entries outside the bundle, and an unrelated variable kept.
- Unit tests of the rule: nothing changes outside a bundle; a variable that points only into the bundle is removed, including a path with a doubled `/`; list variables keep their other entries in order; a list left with only empty entries is removed; a directory that only shares the bundle's name as a prefix is not inside it; unrelated variables are left alone; the runtime variables are removed; a non-UTF-8 entry outside the bundle is kept byte for byte.

### Results
- 859 tests across 70 suites, all passed (23.9s, CachyOS x86-64, rustc 1.99.1).

## [0.1.3.0] - 2026-11-09 23:16 (IST)

The terminal a user chooses is now the terminal that opens, and kernkraft reads the terminal lists itself instead of running `xdg-terminal-exec`. Still alpha; the terminal functions of `kernkraft-xdg-apps` change signature, so a direct Rust caller must pass the new `TerminalPrefs`.

### Added
- **The Default Terminal Execution Specification, read natively**: `kernkraft-xdg-apps` reads `xdg-terminals.list` and `${desktop}-xdg-terminals.list` from the config directories, then `xdg-terminal-exec/` under the system data directories, and picks the first listed entry that is an installed terminal (`TerminalEmulator` category, not `Hidden`, `TryExec` present). An entry ID may carry an `:action`, which runs that action's `Exec`; `-` excludes an entry from fallback and `+` protects it from a later exclusion; lines that are not entry IDs are discarded. With no usable list, any installed terminal entry is used, tried in a fixed order (the user's data directory first, then the system ones, by desktop file ID), and `OnlyShowIn` and `NotShowIn` apply to that fallback only.
- **The terminal's own arguments**: the entry's `Exec`, then its `X-TerminalArgDir` (joined to the value when the key ends in `=`), then the execution argument and the command. The execution argument is `X-TerminalArgExec`, else the older `X-ExecArg` or `ExecArg`, else `-e`; an explicitly empty value omits it.
- **`TerminalPrefs`**: the user's terminal setting and the XDG directories to read the lists and entries from.

### Changed
- **The user's terminal setting is now the first choice.** The order is the setting, the terminal lists, `$TERMINAL`, then any other installed terminal. Before, `xdg-terminal-exec` came first and a terminal chosen in Kern's settings was used only when it was absent, so a system with `xdg-terminal-exec` opened its desktop's default terminal whatever the user had chosen.
- **A setting that names no executable is an error**, not a reason to open another terminal. `NoTerminal` names it. A `$TERMINAL` that names no executable is still skipped.
- **`xdg-terminal-exec` is no longer run.** Its job is done by the reader above, so a system without it opens a terminal too.
- **Breaking**: `terminal_command`, `terminal_command_for`, `open_terminal`, `launch_in_terminal`, `launch_groups` and `open_default` take a `TerminalPrefs` where they took the setting as an `Option<&str>`, and the terminal functions of `kernkraft-tauri` read the lists from the directories the plugin was given. Callers that go through the commands are unaffected.
- `NoTerminal` lists every source it tried, in the new order, with the reason each failed.

### Fixed
- **Selecting a terminal in Kern's settings could open another one**: on a system with `xdg-terminal-exec` installed, which picks the desktop's default terminal (Konsole on KDE), the setting was never reached.
- **No terminal opened on a system without `xdg-terminal-exec`** unless `$TERMINAL` or the setting was set, even with a terminal installed.

### Tests
- `setting_then_lists_then_env_then_installed`: each source beats the ones after it.
- `a_setting_naming_no_executable_is_an_error_not_a_fallback`, `an_empty_setting_counts_as_not_set`, `a_terminal_variable_naming_no_executable_is_skipped_and_named`.
- `list_files_are_read_in_decreasing_priority`, `a_list_for_another_desktop_is_not_read`, `list_syntax_comments_blanks_whitespace_and_directives`: list locations, desktop-specific lists in `$XDG_CURRENT_DESKTOP` order, comments, blank lines and directives.
- `the_first_listed_usable_entry_wins_and_unusable_ones_are_passed_over`, `tryexec_that_is_installed_counts`, `the_notes_say_why_each_listed_entry_was_passed_over`, `a_listed_action_runs_the_actions_exec`.
- `the_execution_argument_comes_from_the_entry`, `the_directory_argument_follows_the_entrys_key`, `the_entrys_own_arguments_come_before_the_command`.
- `installed_terminals_are_tried_by_id_and_the_user_directory_first`, `an_entry_hides_the_same_id_in_a_lower_priority_directory`, `ids_of_entries_in_subdirectories_use_a_dash`, `fallback_respects_exclusions_and_protection`, `show_in_conditions_apply_to_fallback_only`.
- `without_a_usable_home_only_the_setting_and_the_variable_choose`, `a_list_that_cannot_be_read_is_an_error`.
- `terminal_apps_use_the_terminal_setting` also shows the setting beating an installed terminal entry.

### Results
- 850 tests across 69 suites, all passed (24.0s, CachyOS x86-64, rustc 1.99.1).

## [0.1.2.1] - 2026-11-09 18:59 (IST)

A `hyprtoolkit.conf` that writes its colours the way Hyprland's own files do is now read. Still alpha; no API changes.

### Fixed
- **`hyprtoolkit.conf` colours in `0xAARRGGBB` and decimal form were ignored**: `Rgba::parse` knew only `rgb(rrggbb)`, `rgba(rrggbbaa)` and `#hex`, so a file written with `0xFF282a36` had every colour line refused and the theme fell through to the portal, GTK and Kern's own tokens. It now also reads `0xAARRGGBB` (alpha first, all eight digits), `rgb(r,g,b)` and `rgba(r,g,b,a)` (decimal channels 0 to 255, alpha a fraction from 0.0 to 1.0). A number out of range, a sign, an exponent, `nan` or `inf` is refused, never clamped. A bare `#rrggbb` in a `.conf` is still a hyprlang comment.

### Tests
- `parses_argb_hyprtoolkit`: a file written in `0xAARRGGBB` fills every colour and both font families.
- `argb_colour_has_alpha_first`: `0x40ff8000` is red 255, green 128, blue 0, alpha 0x40, in either case of `x`.
- `decimal_colour_forms`: `rgb(r,g,b)` and `rgba(r,g,b,a)` with spaces, with alpha 0, .5, 0.933 and 1 rounded to a byte.
- `malformed_colours_are_refused`: 21 malformed inputs (short or long hex, out-of-range, negative, fractional or missing channels, bad alpha) return nothing.
- `every_notation_in_one_file`: five colour notations in one file parse with nothing ignored.

### Results
- 829 tests across 58 suites, all passed (25.0s, CachyOS x86-64, rustc 1.99.1).

## [0.1.2.0] - 2026-11-09 17:58 (IST)

The Documents walk can reach hidden items when asked. Still alpha; the Rust method's signature changes, so a direct Rust caller of `Kernkraft::documents` must pass the new argument.

### Added
- **`documents` takes `hidden`**: the Documents walk of `kernkraft-tauri` lists dotfiles, dot-folders and everything inside them only when `hidden` is set, as the name search already does. The `documents` command accepts it as an optional field, and a caller that sends none gets the walk it always got, without hidden items.

### Changed
- **`Kernkraft::documents(key, roots, hidden, origin)`** takes the new `hidden` argument before `origin`. Breaking for a direct Rust caller; callers that go through the command are unaffected.

### Tests
- `documents_reach_dotfiles_and_dot_folders_only_when_asked`: a dotfile and a file inside a dot-folder are listed only with the flag, and a hidden file that is not text never is.
- `ipc_documents` covers the command with no `hidden` field and with `hidden` true.

### Results
- 824 tests across 58 suites, all passed (24.0s, CachyOS x86-64, rustc 1.99.1).

## [0.1.1.0] - 2026-11-04 23:46 (IST)

The desktop release: seven new crates, among them the Tauri plugin Kern Desktop 0.1.0.0 is built on, and file operations in `kernkraft-fs` that can be undone. Still alpha, and the API changes below break 0.1.0.2 callers.

### Added
- **`kernkraft-text`**: opens UTF-8 text for editing and saves it atomically with its byte order mark and line endings as they were. A file that could not be saved back byte for byte (not UTF-8, UTF-16, mixed line endings, too large, not writable) opens as a read-only copy that says why, and a save never overwrites such a file.
- **`kernkraft-xdg-apps`**: MIME type lookup from the Shared MIME-info Database (globs, aliases, subclasses, text detection), desktop entries, default applications per the MIME Applications Associations spec, launching through `Exec` lines (files or a URL), and terminal discovery.
- **`kernkraft-thumb`**: thumbnails for PNG, JPEG, WebP, GIF and SVG in the freedesktop thumbnail cache that other file managers share, with failure entries so a broken image is not retried on every view.
- **`kernkraft-bus`**: session and system D-Bus connections that resolve the bus address themselves and only ever open a Unix socket.
- **`kernkraft-theme`**: the system theme from Hyprland's `hyprtoolkit.conf`, the portal's appearance settings, GTK and fontconfig, resolved against Kern's design tokens into CSS custom properties, with derived colours checked for WCAG contrast, and a watcher that reports changes live.
- **`kernkraft-volumes`**: mounted volumes from `/proc/self/mountinfo` with udev labels; mount, unmount and eject through UDisks2; a watcher for mounts and devices that come and go. Each volume carries an identity the actions check, so an action never reaches a different disk that took the same place.
- **`kernkraft-tauri`**: the Tauri 2 plugin (`plugin:kernkraft-tauri|<command>`). Paths cross as `WirePath` (non-UTF-8 names survive) and errors as `WireError` (kind, message, details). File manager commands: listing, create, rename, and copy, move, trash and delete as background operations with progress, cancel and conflict prompts; the trash view; undo and redo; folder watching through inotify; properties. Also text open, check and save, a thumbnail queue, the theme, applications and defaults, the terminal, volumes, user folders, a streamed name search and the Documents walk.
- **`kernkraft-fs` undo journal**: every copy, move, trash, rename and create is recorded with stamps of what it left, and undo or redo refuses, naming the file, when something has changed there since.
- **`kernkraft-fs` `same_filesystem`**: whether a move would be a rename, by device number and mount ID (a bind mount shares the device number but not the mount), so a drop moves on one filesystem and copies across.
- **`kernkraft-fs` `measure`**: totals of size and item counts for a properties view, cancellable.
- **`kernkraft-fs` errors**: `MountPoint`, `MountIdUnavailable`, `Changed`, `Irreversible`, `Partial`, `SourceNotRemoved`, `NothingToUndo`, `NothingToRedo`.

### Changed
- **Every save says what it expects at the path** (`Expect`): the file as it was read (`Unchanged`, by stamp), nothing (`Nothing`, so a new file never replaces one that appeared meanwhile), or `Anything`. `write_atomic`, `write_bytes_atomic` and `kernkraft_pdf::save` take it in place of the optional stamp. Breaking.
- A mount point is never moved: a move or merge of one fails with `FsError::MountPoint` before anything is copied or deleted, at any depth of a merge.
- Restoring from the trash recreates a missing original folder; undoing a trash does not, and fails instead.
- In `kernkraft-tauri`, rename and create are refused with `Busy` while a file operation or an undo runs, and an operation or undo is refused while a rename or create runs; search and the Documents walk never conflict with either.

### Results
- 823 tests across 58 suites, all passed (26.1s, CachyOS x86-64, rustc 1.99.1).

## [0.1.0.2] - 2026-10-18 21:55 (IST)

The fixes for the seven known issues 0.1.0.1 pinned. All eleven tests that shipped red now pass.

### Added
- **`FsError::OwnershipNotKept`**: an atomic save that would change a file's owner or group (the file belongs to another user, or to a group this process is not in) is refused, the file untouched.
- **`FsError::XattrsNotKept`**: names the extended attributes a destination would not take.
- **`OpReport::warnings`**: items a copy or move completed without carrying everything over, today only extended attributes the destination refused.

### Changed
- **Loading a PDF checks its page tree.** A cyclic tree, a page reached twice or a tree nested too deeply is now a `PdfError::Parse` at load, not at the first operation. A page-tree node without `/Type` is given the type ISO 32000 requires, and the file's cross-reference and object streams are dropped on load, since every save writes its own.
- `page_count` counts pages with the same walk as every operation. On a document built by hand whose page tree is malformed it is 0, and every operation reports `PdfError::Parse`.

### Fixed
- A page node without `/Type` is numbered the same way by `page_count`, every page operation and the renderer.
- `extract`, every split and `remove` drop the form fields whose widgets were all on dropped pages, together with their appearances. A field with widgets on kept pages keeps those.
- A load and save that edits nothing writes the same bytes again.
- A rendered page is the requested width and the width times the aspect ratio rounded to the nearest pixel, as Kern Android renders it. The page fills that exact size.
- `parse_ranges` takes page numbers in ASCII digits only, so `+1` is a bad range.
- `.trashinfo` files with space around `=` are read.
- Atomic save keeps the owner, group, extended attributes and POSIX ACLs. Copy and move keep the extended attributes of files and directories, reporting any the destination refuses.

### Results
- 260 tests across 16 suites, all passed (0.23s, CachyOS x86-64, rustc 1.99.1).

## [0.1.0.1] - 2026-10-18 21:33 (IST)

The test release for 0.1.0.0: a round-trip corpus, property tests of every page operation, and edge cases for both crates. No library code changed. Eleven of the new tests fail on purpose: each pins a defect that 0.1.0.2 fixes, and they stay red until then.

### Tests
- **PDF corpus**: ten files, built in code so every expectation follows from how each was made: a classic cross-reference table, a cross-reference stream with object streams, Flate-compressed content, an incremental update, a nested page tree, a page whose contents are an array of streams, an indirect `/Kids` array, an image shared by two pages, an `/Info` dictionary, and the sample from `testdata/`.
- **`kernkraft-pdf` round trip** (`roundtrip`): every corpus file loads with its page count and markers; a save and reload keeps every content object, the trailer's `/Root` and `/Info`, and each page's inherited attributes; an atomic save to disk reads back the same.
- **`kernkraft-pdf` properties** (`properties`): over the whole corpus, an identity reorder changes nothing, a reorder and its inverse restore the document, four quarter turns restore every rotation, a split per page or by ranges merges back to the original, extracting every page keeps it whole, and removing any one page leaves the others. Every result holds no reference to a missing object and survives a save and reload.
- **`kernkraft-pdf` edge cases**: page trees that are cyclic, list a page twice, nest 300 deep or have pages without `/Type`; `/Rotate` given by reference, as a real or negative; malformed page ranges; form fields and an open action that point at dropped pages; untitled, foreign and nested-name-tree bookmarks; merging one document twice and inputs with unparsable versions; empty input and bytes after `%%EOF`; every corpus page rendered at the size its boxes and rotation give.
- **`kernkraft-fs` edge cases**: atomic save into a read-only directory, with setgid bits, of 0 bytes and of 3 MiB, under a non-UTF-8 name, and keeping the group, extended attributes and POSIX ACL; trash round trips of non-UTF-8 names and names holding spaces, `%`, newlines and `#`, restoring into a removed folder, restoring onto a path already taken, an info file whose item is gone, and `.trashinfo` parsing (space around `=`, CRLF, repeated keys, a following group); copying a FIFO, onto a dangling symlink, a read-only tree and hard links; cancelling during the second source; progress that never runs backwards or past its total; moving a symlink; extended attributes through copy; duplicate `user-dirs.dirs` keys; scanning duplicate roots, a file root and a FIFO; staging a directory.

### Known issues
Each item below is pinned by a failing test and fixed in 0.1.0.2.
- A page node without `/Type` is a page to the renderer and to every page operation, but `page_count` leaves it out, so page numbers can name different pages.
- `extract`, and every split built on it, keeps the `/AcroForm` fields of dropped pages, and with them those pages' field appearances.
- A load and save that edits nothing rewrites the file: the cross-reference stream moves to a new object number on every save.
- A rendered page is one pixel taller than the width times the aspect ratio, rounded, whenever the exact height has a fraction (MuPDF rounds outward); Kern Android rounds to the nearest pixel, and the height limit check rounds to nearest too.
- `parse_ranges` accepts `+1` as page 1.
- `.trashinfo` files with space around `=` are reported as malformed.
- Atomic save keeps permission bits but drops the group, extended attributes and POSIX ACLs; copy drops extended attributes.

### Results
- 256 tests across 16 suites: 245 passed, 11 failed, every failure a known issue above (0.14s, CachyOS x86-64, rustc 1.99.1).

## [0.1.0.0] - 2026-10-18 13:39 (IST)

The first alpha: the workspace, a complete `kernkraft-fs` and the page operations of `kernkraft-pdf`.

### Added
- **Workspace and gates**: a Cargo workspace whose CI runs rustfmt, clippy with warnings denied, the test suite, a check that no core crate depends on `tauri` or `jni`, and `cargo deny`, which bans every HTTP, socket and async runtime crate. The version label lives in `VERSION` and reaches Cargo as semver build metadata (`0.1.0+0`).
- **`kernkraft-fs` XDG directories**: base directories per the XDG Base Directory Specification (an empty or relative variable is ignored) and user directories from `user-dirs.dirs`, with English defaults for any it leaves out. An unreadable `user-dirs.dirs` is an error, never a silent fallback.
- **`kernkraft-fs` atomic save**: temp file in the target's directory, fsync, permission bits carried over, rename, directory fsync; symlinks are written through and kept. A `FileStamp` (length, mtime, device, inode, ctime) taken at open makes a save refuse when another program wrote, replaced or deleted the file in the meantime. Any failure leaves the target and its directory untouched.
- **`kernkraft-fs` trash**: FreeDesktop Trash 1.0, with the home trash, a mount's shared `.Trash/$uid` and `.Trash-$uid`; list, restore and purge. Items are only ever renamed into a trash, never copied across devices. Listing reports each info file it cannot use. Restore never replaces a file at the original path.
- **`kernkraft-fs` file operations**: create, rename (never replacing anything), permanent delete (never following a symlink), and bulk copy and move with Skip, Replace and Keep Both conflict handling, byte progress and cancel. A copied file is never seen half-written, a folder cannot be placed inside itself, and a move across devices deletes a source only after its copy is complete.
- **`kernkraft-fs` scanning and staging**: sorted directory listing, a depth-limited scan that follows directory symlinks and ends on link loops, and a private `0700` staging cache for temporary copies of documents, swept on open.
- **`kernkraft-pdf` foundation**: load from a path or bytes (encrypted files, files with no pages and malformed files are typed errors; a parser panic is caught), atomic save with external-change detection, strict page-range parsing (out of range is an error, never clamped), and page-tree flattening that keeps every inherited `Resources`, `MediaBox`, `CropBox` and `Rotate`.
- **`kernkraft-pdf` page operations**: extract, split by ranges, per page and by bookmarks (named destinations included), merge, rotate, reorder and remove. Pages keep their inherited attributes through every operation, and pages left out of a result take none of their content with them. Merge, extract and split do not carry bookmarks into their result; remove keeps the outline.
- **`kernkraft-pdf` rendering** (`render` feature): MuPDF renders a page to a requested width as opaque RGBA8; `Renderer` keeps a document open for many pages.

### Results
- 190 tests across 14 suites, all passed (0.1 s warm, CachyOS x86-64, rustc 1.99.1).
