#!/bin/sh
# Regenerate encrypted_xref_no_type.pdf with qpdf (checked with 11.9.0).
#
# qpdf writes this as a PDF 1.5-style file whose cross-reference stream is the
# trailer: `startxref` points at object 7, and the document's `/Encrypt` entry
# sits in that stream's own dictionary. The `/Type /XRef` marker is then
# blanked in place, with the same number of bytes, so every offset in the file
# and the `startxref` value stay valid. What is left is a whole file whose only
# defect is the missing marker: a reader has to see the encryption anyway and
# refuse to hand back ciphertext, and the file's own password still has to open
# it.
set -eu
cd "$(dirname "$0")"
QPDF=${QPDF:-qpdf}
"$QPDF" --static-id --object-streams=generate --encrypt user owner 256 \
  -- compressed.pdf encrypted_xref_no_type.pdf
# Blank `/Type /XRef` (11 bytes) in place, never shifting what follows it.
perl -0777 -pi -e 'my $n = s{/Type /XRef}{" " x 11}e;
  die "no /Type /XRef to remove\n" unless $n;' encrypted_xref_no_type.pdf
