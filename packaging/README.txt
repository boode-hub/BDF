BDF — a fast, portable PDF reader
==================================

Run bdf.exe (Windows) or ./bdf (macOS, Linux). Nothing is installed: settings,
bookmarks and the log live in the BDF-data folder next to the program, so the
whole folder can sit on a USB stick or a network share.

The PDF engine (PDFium) is built into the program, so bdf.exe works on its
own: copy just that file anywhere and it runs. To use a different PDFium,
put pdfium.dll (libpdfium.so / libpdfium.dylib) next to the program.

Open files with File > Open, by dropping them on the window, or with
"Open with" in your file manager. Help > Keyboard shortcuts lists them all.

If the window does not appear on an old or virtual machine, start BDF with
  bdf --renderer wgpu
or choose Graphics in Preferences.

Licences: PDFium (BSD-3-Clause) and its components, Phosphor icons (MIT);
see the licenses folder.
