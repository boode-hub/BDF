import sys
from pypdf import PdfReader, PdfWriter
import io
content = b"BT /F1 24 Tf 72 700 Td (Secret PAGE-1) Tj ET"
objs = [
 b"<< /Type /Catalog /Pages 2 0 R >>",
 b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
 b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
 b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
 b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
]
out = bytearray(b"%PDF-1.7\n")
offs = []
for i, o in enumerate(objs, 1):
    offs.append(len(out)); out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
x = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs)+1)
for o in offs: out += b"%010d 00000 n \n" % o
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs)+1, x)
w = PdfWriter(clone_from=PdfReader(io.BytesIO(bytes(out))))
w.encrypt(user_password="bdf-secret", owner_password="bdf-owner", algorithm="RC4-128")
with open(sys.argv[1], "wb") as f: w.write(f)
