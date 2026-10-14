# Tesseract Architecture Notes (Phase 1)

## Overview
Tesseract is an OCR engine. For our hybrid approach, we use it via bindings (leptess) initially.

## Key Concepts
- Page Layout Analysis: detects text blocks, images, tables
- Baseline detection and line finding
- Character segmentation
- Recognition using LSTM (modern) or legacy classifiers
- Language models and dictionaries

## Integration Plan
- Use `leptess` crate - provides safe Rust bindings to Tesseract C++ API
- Need Tesseract installed on system (or consider static build)
- Pass image data to Tesseract, get text + bounding boxes
- Map results to Docling JSON (Provenance with bboxes, TextItems)

## Mapping to our pipeline
In OCR engine crate (crates/ocr-engine), wrap leptess for:
1. Image preprocessing if needed (grayscale, threshold) using imageproc
2. OCR via tesseract
3. Convert hOCR/box data to our internal structures

## References
- Tesseract docs: https://tesseract-ocr.github.io/
- leptess crate: https://docs.rs/leptess
- Tesseract source: https://github.com/tesseract-ocr/tesseract
