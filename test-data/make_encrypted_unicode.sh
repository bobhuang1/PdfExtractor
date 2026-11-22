#!/bin/sh
# Regenerate the Unicode-password fixtures with qpdf (checked with 11.9.0).
#
# Both files hold `hello.pdf`'s page under a non-ASCII user password, so a
# reader has to write the password the way the file's revision does:
#
#   encrypted_pdfdoc.pdf      revision 4 (AES-128), password `Grüße`, stored in
#                             PDFDocEncoding: qpdf converts the UTF-8 argument.
#   encrypted_r6_saslprep.pdf revision 6 (AES-256), password `päss` stored as
#                             composed UTF-8. qpdf 11.9 does not SASLprep an R6
#                             password, so the file is written from the composed
#                             text; a reader that does (Adobe's, and this one)
#                             opens it with the decomposed `pa\u0308ss` too.
set -eu
cd "$(dirname "$0")"
QPDF=${QPDF:-qpdf}
"$QPDF" --warning-exit-0 --static-id --encrypt 'Grüße' owner 128 --use-aes=y \
  -- hello.pdf encrypted_pdfdoc.pdf
"$QPDF" --warning-exit-0 --static-id --encrypt 'päss' owner 256 \
  -- hello.pdf encrypted_r6_saslprep.pdf
