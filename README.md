# BDF

A fast, portable PDF reader for Windows, macOS and Linux, written in Rust,
dressed in Sharingan crimson and black. The logo is Kakashi's Mangekyou
Sharingan (or Itachi's, in Preferences), drawn from the traced Classic eyes of
[Sharingan-Monitor](https://github.com/BOoDe007/Sharingan-Monitor); it spins
while pages load.

* **Portable.** Unzip and run. No installer, no admin rights, no registry.
  Settings, bookmarks and the log live in `BDF-data/` next to the program.
* **Light.** One small executable plus PDFium. Idle CPU is zero: BDF only
  draws when something changes.
* **Accurate.** Pages are rendered by PDFium, the engine inside Chrome.

## Features

| Area | What you get |
|---|---|
| Reading | Tabs; single, continuous, facing and book layouts; fit width / fit page / 5–6400% zoom (Ctrl+wheel zooms at the cursor); rotate; full screen; dark pages for night reading |
| Split view | Two panes side by side or stacked, each with its own tabs; the same document or two different ones; optional synchronised scrolling |
| Navigation | Thumbnails, the document outline, your own bookmarks (Ctrl+B), page labels, go to page (Ctrl+G), back/forward (Alt+←/→), clickable links |
| Search | Find as you type, match case, whole words, all hits highlighted, results list with context; ignores line breaks and hyphenation, matches ligatures and curly quotes |
| Text | Select (drag, double-click word, triple-click line, across pages) and copy |
| Markup | Highlight, underline, strikeout, sticky notes, pen, rectangle, ellipse; edit or delete; undo/redo; saved into the PDF so other readers see them |
| Smart highlighting | Selection follows what you see: hidden text (covered, white on white, invisible, switched-off layers, off the page) is ignored, columns are kept apart, and highlights snap to whole words — see `crates/bdf-core/src/visual.rs` |
| Forms | Fill in text fields, checkboxes, radio buttons, drop-downs and lists; fillable fields are tinted; values save into the PDF |
| Tools | Magnifier loupe, hand tool, middle-mouse panning |
| Files | Open recent, drag and drop, password-protected PDFs, document properties, print |

Saving appends your changes to the end of the file (an *incremental save*):
the original bytes are never rewritten, so digital signatures stay valid. The
new file is written beside the original, verified by reopening it, and only
then moved into place.

## Layout of the code

```
crates/bdf-core   document engine, no UI; fully unit tested
  pdfium.rs       PDFium through its C API (the only unsafe code)
  worker.rs       engine thread: priorities, cancellation, search jobs
  layout.rs       page positions for every layout mode
  geom.rs         PDF space <-> screen space, rotation
  text.rs         search over extracted text
  visual.rs       selection by visible lines and columns (smart highlighting)
  cache.rs        byte-bounded LRU for page images
  settings.rs     portable settings, per-document memory
  testpdf.rs      tiny PDF writer for tests and benchmarks
crates/bdf        the application (egui)
  view.rs         one document view: scrolling, zoom, tiles, tools
  app.rs          tabs, panes, engine responses, commands, shortcuts
  chrome.rs       menus, toolbar, tab strip, status bar, find bar
  sidebar.rs      thumbnails, outline, bookmarks, annotations, results
  dialogs.rs      modal dialogs
  print.rs        Windows GDI printing; CUPS on macOS/Linux
  bench.rs        `bdf --bench` scripted benchmark
```

How it stays fast:

* PDFium runs on its own thread; the window never waits for it.
* What is on screen is drawn first. Reading a page's text, links and the
  hidden-text check waits until the visible pages have their images, then
  the next screen and the sidebar thumbnails follow.
* Pages are drawn as 1024-pixel tiles, and only the visible ones (plus one
  screen ahead) are rendered. Tiles that scroll away before they are drawn are
  dropped from the queue, so fast scrolling never builds a backlog.
* A small thumbnail shows instantly while the sharp tiles render; while you
  zoom, the previous tiles are stretched until the new ones are ready.
* Page images live in a GPU cache with a memory budget (Preferences).
* Search runs one page at a time between renders.

## Building

Needs Rust 1.88 or newer and the PDFium library.

```sh
scripts/fetch_pdfium.sh win-x64      # or linux-x64, mac-arm64, mac-x64
cargo build --release
cp .pdfium/*pdfium* target/release/
target/release/bdf some.pdf
```

`scripts/package.sh win-x64` produces the portable zip in `dist/`.

## Testing

```sh
BDF_PDFIUM=$PWD/.pdfium cargo test --workspace     # unit + real-PDFium tests
target/release/bdf --bench --report bench.json      # scripted benchmark of the real app
cargo run --release -p bdf-core --example timing -- file.pdf   # engine cost per step for page 1
python3 scripts/check_bench.py bench.json           # fails if over budget
scripts/ui_smoke.sh target/release/bdf              # Linux: real clicks via xdotool, checks the saved PDF
```

CI (`.github/workflows/ci.yml`) runs formatting, clippy, all tests and the
benchmark on Windows, macOS and Linux, enforces the performance budget in
`scripts/check_bench.py`, and uploads the portable zips. Tagging `v*`
publishes a release.

The UI test drives the real window with the mouse (highlight, pen, rectangle,
Edit > Undo, File > Save, ticking a form checkbox) and then checks what the
saved file contains.

The engine tests cover rendering, tiles stitching into whole pages, rotation
geometry, text and links, annotations surviving an in-place save, passwords,
and 150 truncated or corrupted files opened without a crash.

## Command line

```
bdf [file.pdf ...] [--page N] [--renderer gl|wgpu]
bdf --bench [file] [--report path]
bdf --generate N out.pdf
bdf --generate-form out.pdf         # a one-page test form
bdf --list-annotations file.pdf      # what PDFium sees, one line per annotation
```

If the window does not open on a virtual machine or over remote desktop
(no OpenGL), BDF restarts itself with DirectX/Vulkan/Metal automatically.

## Licences

PDFium: BSD-3-Clause. Phosphor icons: MIT. egui: MIT/Apache-2.0.
