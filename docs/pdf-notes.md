# PDF 1.7 Spec Notes (Phase 1)

Key components to understand:
- PDF is a structured document with header, body (objects), cross-reference table (xref), trailer
- Objects: dictionaries, arrays, streams, numbers, strings, names, booleans, null
- Indirect objects: `1 0 obj ... endobj`
- Streams: contain compressed data (often Flate), need decoding
- Page tree: Catalog -> Pages -> Page(s)
- Content streams: contain drawing operators (BT/ET for text)
- Fonts: Type1, TrueType etc - for text extraction

For minimal parsing (hello world):
- Read header: %PDF-1.4
- Parse objects (dictionaries with << >>, arrays with [])
- Find xref and trailer to locate objects
- Follow page tree to find content streams
- Parse content stream for text operators (Tj, Td, etc.)

Implementation approach (domain logic ourselves, per hybrid plan):
- Write our own PDF tokenizer/lexer
- Parse objects from bytes
- Handle basic stream decoding if needed (or use infrastructure if available - but decoding logic is core; need to clarify per plan)
- Build object graph

Next: prototype minimal parser for hello.pdf
