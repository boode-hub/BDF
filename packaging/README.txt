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

To update, choose Help > Check for updates (BDF only goes online when you
ask). A new version is installed only if it is signed with BDF's key, matches
its checksum and passes a self-test; the previous program is kept as
bdf.old.exe (bdf.old) until the new one has started. To go back by hand,
delete bdf.exe and rename bdf.old.exe to bdf.exe.

If the window does not appear on an old or virtual machine, start BDF with
  bdf --renderer wgpu
or choose Graphics in Preferences.

Licences: PDFium (BSD-3-Clause) and its components, Phosphor icons (MIT);
see the licenses folder.
