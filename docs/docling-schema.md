# Docling JSON Schema Reference

Based on research from Docling project. This document captures the key structure for our Rust implementation.

## Key References
- [Docling Document Concepts](https://docling-project.github.io/docling/concepts/docling_document/)
- [Docling Core Schema](https://raw.githubusercontent.com/docling-project/docling-core/main/docs/DoclingDocument.json)

## Core Structure
- `schema_name`: "DoclingDocument" 
- `version`: Schema version (string)
- `name`: Document name
- `origin`: Optional source metadata (mimetype, binary_hash, filename, uri)
- `pages`: HashMap of page number (as string in JSON) to Page
- `texts`: Vec of TextItem
- `tables`: Vec of TableItem
- `pictures`: Vec of PictureItem
- `body`: Root group with children (reading order via refs)
- `groups`, `furniture` etc as needed

## Coordinates
- Bounding boxes use `l, t, r, b` (left, top, right, bottom)
- `coord_origin`: "topleft" (default) or "bottomleft"
- Page coordinates, not normalized 0-1

## References
Items reference each other via `{"$ref": "#/texts/0"}` style paths and have `self_ref`.

## Implementation Status
Core types implemented in `crates/core/src/types.rs`. May need adjustment as we discover actual schema details from real Docling JSON outputs.
