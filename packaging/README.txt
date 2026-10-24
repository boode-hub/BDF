BDF — a fast, portable PDF reader
==================================

Run bdf.exe (Windows) or ./bdf (macOS, Linux). Nothing is installed: settings,
bookmarks and the log live in the BDF-data folder next to the program, so the
whole folder can sit on a USB stick or a network share.

Keep the PDFium library (pdfium.dll / libpdfium.so / libpdfium.dylib) next to
the program; BDF needs it to read PDFs.

Open files with File > Open, by dropping them on the window, or with
"Open with" in your file manager. Press F1-style help: Help > Keyboard shortcuts.

If the window does not appear on an old or virtual machine, start BDF with
  bdf --renderer wgpu
or choose Graphics in Preferences.

Licences: PDFium (BSD, PDFium-LICENSE.txt), Phosphor icons (MIT).
