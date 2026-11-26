//! Resource limits for inputs this program did not write.
//!
//! A PDF, an office package or a photograph is a program the reader runs in its
//! own address space: a few bytes can ask for a gigabyte of pixels, a page tree
//! that never ends, a ZIP entry that expands without bound, or XML nested deep
//! enough to exhaust the stack. The readers used to trust those requests. This
//! crate states the budgets once so the PDF, ZIP, XML and image readers cannot
//! drift apart, and so a caller can lower one for a fuzz run or a test.
//!
//! The limits are deliberately generous for real documents — a scan corpus is
//! hundreds of megabytes and a scanned page can be a hundred megapixels — while
//! still bounding the work an untrusted file can ask for. Every rejection is a
//! [`LimitError`] naming the budget and the amount, so a caller can say why a
//! file was refused rather than reporting a generic parse failure.

use std::io::{Cursor, Read, Seek};
use std::time::{Duration, Instant};

use quick_xml::events::{BytesRef, Event};
use quick_xml::Reader;

/// Largest input byte slice any reader will accept.
pub const MAX_INPUT_BYTES: u64 = 256 * 1024 * 1024;
/// Most pages a paginated document may claim.
pub const MAX_PAGES: usize = 10_000;
/// Most indirect objects a PDF may hold.
pub const MAX_PDF_OBJECTS: usize = 250_000;
/// Most lexical tokens a PDF may present; bounds the parser's token vector.
pub const MAX_PDF_TOKENS: usize = 4_000_000;
/// Deepest PDF array/dictionary/page tree the parser will descend.
pub const MAX_PDF_NESTING: usize = 64;
/// Largest byte count a single decoded PDF stream may produce.
pub const MAX_DECODED_STREAM_BYTES: u64 = 128 * 1024 * 1024;
/// Most entries a ZIP package may hold.
pub const MAX_ZIP_ENTRIES: usize = 10_000;
/// Largest uncompressed size a single ZIP entry may declare.
pub const MAX_ZIP_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// Largest total uncompressed size a ZIP package may declare.
pub const MAX_ZIP_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// Largest expansion ratio a ZIP entry above one mebibyte may declare.
pub const MAX_ZIP_RATIO: u64 = 200;
/// Largest XML part, compressed or not, a package may carry.
pub const MAX_XML_BYTES: u64 = 64 * 1024 * 1024;
/// Deepest XML element nesting accepted.
pub const MAX_XML_DEPTH: usize = 256;
/// Largest image grid in pixels.
pub const MAX_IMAGE_PIXELS: u64 = 100_000_000;
/// Largest single image dimension.
pub const MAX_IMAGE_DIMENSION: u32 = 100_000;
/// Most frames a TIFF, GIF or WebP may hold when read as pages.
pub const MAX_IMAGE_FRAMES: usize = 1_000;
/// Maximum bytes one image decode may allocate.
pub const MAX_IMAGE_ALLOC_BYTES: u64 = 512 * 1024 * 1024;
/// Largest total size one document may expand to, across every PDF stream and
/// image frame its reader decodes.
///
/// The per-item ceilings bound one stream or frame; a file of many items each
/// just under its own ceiling would otherwise expand without bound. This is the
/// running total those ceilings are charged to — eight streams at
/// [`MAX_DECODED_STREAM_BYTES`], so a document may hold several streams at the
/// single-stream ceiling but not an unbounded number of them. Packages are not
/// charged to it: [`ZipBudget`] already bounds a whole package's expansion, at
/// `max_zip_total_bytes`.
pub const MAX_DOCUMENT_BYTES: u64 = 8 * MAX_DECODED_STREAM_BYTES;
/// Most blocks a flow reader (text, Markdown, CSV, RTF, HTML, DOCX and legacy
/// `.doc`) may produce.
///
/// A table counts one block per cell: what a grid costs is what it holds, not
/// the one item it becomes.
pub const MAX_FLOW_BLOCKS: usize = 1_000_000;
/// Largest file one LibreOffice bridge conversion may hand back.
pub const MAX_BRIDGE_OUTPUT_BYTES: u64 = 512 * 1024 * 1024;
/// Wall-clock budget for one LibreOffice bridge conversion.
pub const BRIDGE_TIMEOUT: Duration = Duration::from_secs(120);
/// Wall-clock budget for one parser, from entry to return.
pub const PARSE_TIMEOUT: Duration = Duration::from_secs(30);
/// Wall-clock budget for one document's OCR pages.
///
/// Recognition is seconds to a minute a page where parsing is milliseconds, so
/// it gets its own, larger budget rather than the parser's. The deadline is
/// checked between pages, which is the only place a blocking recognition call
/// can be interrupted without killing the engine.
pub const OCR_TIMEOUT: Duration = Duration::from_secs(600);

/// A budget for one untrusted input.
///
/// `Default` is the production policy. A caller that wants a tighter budget —
/// a fuzzer, a test, a service with a smaller memory allowance — constructs one
/// by changing the fields it cares about.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_input_bytes: u64,
    pub max_pages: usize,
    pub max_pdf_objects: usize,
    pub max_pdf_tokens: usize,
    pub max_pdf_nesting: usize,
    pub max_decoded_stream_bytes: u64,
    pub max_zip_entries: usize,
    pub max_zip_entry_bytes: u64,
    pub max_zip_total_bytes: u64,
    pub max_zip_ratio: u64,
    pub max_xml_bytes: u64,
    pub max_xml_depth: usize,
    pub max_image_pixels: u64,
    pub max_image_dimension: u32,
    pub max_image_frames: usize,
    pub max_image_alloc_bytes: u64,
    pub max_flow_blocks: usize,
    pub max_document_bytes: u64,
    pub max_bridge_output_bytes: u64,
    pub bridge_timeout: Duration,
    pub timeout: Duration,
    pub ocr_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: MAX_INPUT_BYTES,
            max_pages: MAX_PAGES,
            max_pdf_objects: MAX_PDF_OBJECTS,
            max_pdf_tokens: MAX_PDF_TOKENS,
            max_pdf_nesting: MAX_PDF_NESTING,
            max_decoded_stream_bytes: MAX_DECODED_STREAM_BYTES,
            max_zip_entries: MAX_ZIP_ENTRIES,
            max_zip_entry_bytes: MAX_ZIP_ENTRY_BYTES,
            max_zip_total_bytes: MAX_ZIP_TOTAL_BYTES,
            max_zip_ratio: MAX_ZIP_RATIO,
            max_xml_bytes: MAX_XML_BYTES,
            max_xml_depth: MAX_XML_DEPTH,
            max_image_pixels: MAX_IMAGE_PIXELS,
            max_image_dimension: MAX_IMAGE_DIMENSION,
            max_image_frames: MAX_IMAGE_FRAMES,
            max_image_alloc_bytes: MAX_IMAGE_ALLOC_BYTES,
            max_flow_blocks: MAX_FLOW_BLOCKS,
            max_document_bytes: MAX_DOCUMENT_BYTES,
            max_bridge_output_bytes: MAX_BRIDGE_OUTPUT_BYTES,
            bridge_timeout: BRIDGE_TIMEOUT,
            timeout: PARSE_TIMEOUT,
            ocr_timeout: OCR_TIMEOUT,
        }
    }
}

impl Limits {
    /// Reject an input before a reader touches it.
    pub fn check_input(&self, len: usize) -> Result<()> {
        if len as u64 > self.max_input_bytes {
            return Err(LimitError::InputTooLarge {
                actual: len as u64,
                limit: self.max_input_bytes,
            });
        }
        Ok(())
    }

    /// Reject a document claiming more pages than the budget allows.
    pub fn check_pages(&self, count: usize) -> Result<()> {
        if count > self.max_pages {
            return Err(LimitError::TooManyPages {
                actual: count,
                limit: self.max_pages,
            });
        }
        Ok(())
    }

    /// Reject a PDF with more indirect objects than the budget allows.
    pub fn check_pdf_objects(&self, count: usize) -> Result<()> {
        if count > self.max_pdf_objects {
            return Err(LimitError::TooManyPdfObjects {
                actual: count,
                limit: self.max_pdf_objects,
            });
        }
        Ok(())
    }

    /// Reject a PDF that presents more lexical tokens than the budget allows.
    pub fn check_pdf_tokens(&self, count: usize) -> Result<()> {
        if count > self.max_pdf_tokens {
            return Err(LimitError::TooManyPdfTokens {
                actual: count,
                limit: self.max_pdf_tokens,
            });
        }
        Ok(())
    }

    /// Reject a stream whose decoded size exceeds the budget.
    pub fn check_decoded_stream(&self, len: usize) -> Result<()> {
        if len as u64 > self.max_decoded_stream_bytes {
            return Err(LimitError::DecodedStreamTooLarge {
                actual: len as u64,
                limit: self.max_decoded_stream_bytes,
            });
        }
        Ok(())
    }

    /// Reject an image grid the decoder must not allocate.
    pub fn check_image(&self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(LimitError::ImageEmpty);
        }
        if width > self.max_image_dimension || height > self.max_image_dimension {
            return Err(LimitError::ImageTooWide {
                width,
                height,
                limit: self.max_image_dimension,
            });
        }
        let pixels = u64::from(width) * u64::from(height);
        if pixels > self.max_image_pixels {
            return Err(LimitError::ImageTooManyPixels {
                width,
                height,
                pixels,
                limit: self.max_image_pixels,
            });
        }
        Ok(())
    }

    /// Reject an image decode whose working set exceeds the budget.
    pub fn check_image_alloc(&self, bytes: u64) -> Result<()> {
        if bytes > self.max_image_alloc_bytes {
            return Err(LimitError::ImageAllocTooLarge {
                actual: bytes,
                limit: self.max_image_alloc_bytes,
            });
        }
        Ok(())
    }

    /// Reject an animation or TIFF with more sheets than the budget allows.
    pub fn check_image_frames(&self, count: usize) -> Result<()> {
        if count > self.max_image_frames {
            return Err(LimitError::TooManyImageFrames {
                actual: count,
                limit: self.max_image_frames,
            });
        }
        Ok(())
    }

    /// Reject a flow document that expanded into more blocks than the budget
    /// allows. A table counts one block per cell.
    pub fn check_flow_blocks(&self, count: usize) -> Result<()> {
        if count > self.max_flow_blocks {
            return Err(LimitError::TooManyFlowBlocks {
                actual: count,
                limit: self.max_flow_blocks,
            });
        }
        Ok(())
    }

    /// Reject a file one LibreOffice conversion wrote that is larger than the
    /// budget allows: the bridge reads a small file and can be handed a huge one
    /// back, which no input-size check on the original would catch.
    pub fn check_bridge_output(&self, len: u64) -> Result<()> {
        if len > self.max_bridge_output_bytes {
            return Err(LimitError::BridgeOutputTooLarge {
                actual: len,
                limit: self.max_bridge_output_bytes,
            });
        }
        Ok(())
    }

    /// A whole-document total to charge expansions to, starting at zero.
    pub fn document_budget(&self) -> DocumentBudget<'_> {
        DocumentBudget::new(self)
    }

    /// A deadline measured from now, for one parser run.
    pub fn deadline(&self) -> Deadline {
        Deadline {
            start: Instant::now(),
            limit: self.timeout,
        }
    }

    /// A deadline measured from now, for one document's OCR pages.
    pub fn ocr_deadline(&self) -> Deadline {
        Deadline {
            start: Instant::now(),
            limit: self.ocr_timeout,
        }
    }

    /// A deadline measured from now, for one LibreOffice conversion.
    pub fn bridge_deadline(&self) -> Deadline {
        Deadline {
            start: Instant::now(),
            limit: self.bridge_timeout,
        }
    }

    /// The budgets a reader carries through its walk.
    pub fn guard(&self) -> Guard<'_> {
        Guard::new(self)
    }

    /// Check a ZIP package before a reader parses it.
    ///
    /// This checks the declared uncompressed sizes, the total they add up to,
    /// and the ratio of each entry to its compressed form. It then reads each
    /// entry under the same ceiling to catch a central directory that lies
    /// about its sizes, so the total *actual* expansion is bounded too. XML
    /// parts are walked while they are read, so a deeply nested part is refused
    /// before a recursive reader sees it.
    pub fn check_zip<R: Read + Seek>(&self, archive: &mut zip::ZipArchive<R>) -> Result<()> {
        let count = archive.len();
        if count > self.max_zip_entries {
            return Err(LimitError::TooManyZipEntries {
                actual: count,
                limit: self.max_zip_entries,
            });
        }
        let deadline = self.deadline();
        let mut total = 0u64;
        let mut actual_total = 0u64;
        for index in 0..count {
            deadline.check("reading a ZIP directory")?;
            let (name, size, compressed, is_dir) = {
                let entry = archive
                    .by_index_raw(index)
                    .map_err(|error| LimitError::Zip {
                        detail: error.to_string(),
                    })?;
                (
                    entry
                        .name()
                        .map_err(|error| LimitError::Zip {
                            detail: error.to_string(),
                        })?
                        .into_owned(),
                    entry.size(),
                    entry.compressed_size(),
                    entry.is_dir(),
                )
            };
            if is_dir {
                continue;
            }
            if size > self.max_zip_entry_bytes {
                return Err(LimitError::ZipEntryTooLarge {
                    name,
                    actual: size,
                    limit: self.max_zip_entry_bytes,
                });
            }
            // An entry whose compressed form is tiny and whose declared form is
            // large is the shape of a bomb. The ratio is only meaningful above
            // a mebibyte, where a legitimately repetitive XML or CSV can reach
            // a few hundred to one without being a bomb.
            if size > 1024 * 1024 && compressed > 0 && size / compressed > self.max_zip_ratio {
                return Err(LimitError::ZipRatioTooHigh {
                    name,
                    ratio: size / compressed,
                    limit: self.max_zip_ratio,
                });
            }
            total = total.saturating_add(size);
            if total > self.max_zip_total_bytes {
                return Err(LimitError::ZipTotalTooLarge {
                    actual: total,
                    limit: self.max_zip_total_bytes,
                });
            }
            // The central directory is a claim. Read the entry under the total
            // budget so an entry that lies about its size cannot expand past
            // the package ceiling, and so the sum of what really came out is
            // bounded as well.
            let remaining = self.max_zip_total_bytes.saturating_sub(actual_total);
            // One read may not allocate more than either budget allows: the
            // package's remaining total or a single entry's ceiling.
            let ceiling = remaining.min(self.max_zip_entry_bytes);
            let mut file = archive.by_index(index).map_err(|error| LimitError::Zip {
                detail: error.to_string(),
            })?;
            // Only an XML part's bytes are needed after the count, for the depth
            // and `DOCTYPE` walk. Every other entry — a picture, a font, an
            // embedded object — is measured as it streams through one reused
            // buffer, so the gate does not hold a copy of a package's largest
            // picture just to know how big it was.
            let is_xml = is_xml_part(&name);
            let mut bytes = Vec::new();
            let actual = if is_xml {
                file.by_ref()
                    .take(ceiling.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|error| LimitError::Zip {
                        detail: error.to_string(),
                    })?;
                bytes.len() as u64
            } else {
                count_stream(&mut file, ceiling)?
            };
            if actual > ceiling {
                if ceiling == self.max_zip_entry_bytes {
                    return Err(LimitError::ZipEntryTooLarge {
                        name,
                        actual,
                        limit: self.max_zip_entry_bytes,
                    });
                }
                return Err(LimitError::ZipTotalTooLarge {
                    actual: actual_total.saturating_add(actual),
                    limit: self.max_zip_total_bytes,
                });
            }
            actual_total = actual_total.saturating_add(actual);
            if is_xml {
                if bytes.len() as u64 > self.max_xml_bytes {
                    return Err(LimitError::XmlTooLarge {
                        actual: bytes.len() as u64,
                        limit: self.max_xml_bytes,
                    });
                }
                // SVG media is carried as bytes: the renderer decides whether
                // the markup can be drawn, and a malformed drawing is left as
                // the bytes it is. A well-formed one still gets the same depth
                // and `DOCTYPE` gate as every other XML part.
                let is_svg = name.to_ascii_lowercase().ends_with(".svg");
                match self.check_xml(&bytes) {
                    Err(LimitError::XmlMalformed { .. }) if is_svg => {}
                    result => result?,
                }
            }
        }
        Ok(())
    }

    /// Check an XML part for size, nesting depth and entity declarations.
    ///
    /// The reader is iterative, so this walk cannot itself overflow the stack.
    /// A `DOCTYPE` is refused outright: OOXML and ODF carry no entity
    /// declarations, and allowing one is how an entity-expansion bomb starts.
    pub fn check_xml(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > self.max_xml_bytes {
            return Err(LimitError::XmlTooLarge {
                actual: bytes.len() as u64,
                limit: self.max_xml_bytes,
            });
        }
        let deadline = self.deadline();
        let mut reader = Reader::from_reader(std::io::Cursor::new(bytes));
        let mut buffer = Vec::new();
        let mut depth = 0usize;
        let mut events = 0u64;
        loop {
            events = events.wrapping_add(1);
            if events.is_multiple_of(4096) {
                deadline.check("reading XML")?;
            }
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(_)) => {
                    depth += 1;
                    if depth > self.max_xml_depth {
                        return Err(LimitError::XmlTooDeep {
                            actual: depth,
                            limit: self.max_xml_depth,
                        });
                    }
                }
                Ok(Event::Empty(_)) => {
                    if depth + 1 > self.max_xml_depth {
                        return Err(LimitError::XmlTooDeep {
                            actual: depth + 1,
                            limit: self.max_xml_depth,
                        });
                    }
                }
                Ok(Event::End(_)) => depth = depth.saturating_sub(1),
                Ok(Event::DocType(_)) => return Err(LimitError::XmlDoctype),
                Ok(Event::Eof) => break,
                Err(error) => {
                    return Err(LimitError::XmlMalformed {
                        detail: error.to_string(),
                    })
                }
                _ => {}
            }
            buffer.clear();
        }
        Ok(())
    }
}

/// Append what one `&…;` reference in character data stands for.
///
/// The XML reader reports a document's character data and the references
/// written inside it as separate events, so a reader that wants the text the
/// document says appends both. The five entities XML predefines, and numeric
/// character references, resolve to their characters; a reference a document
/// declares in its own DTD (`&nbsp;`) keeps its spelling, because no DTD is
/// read here.
pub fn push_reference(into: &mut String, reference: &BytesRef<'_>) {
    if let Ok(Some(character)) = reference.resolve_char_ref() {
        into.push(character);
        return;
    }
    match reference.as_ref() {
        "lt" => into.push('<'),
        "gt" => into.push('>'),
        "amp" => into.push('&'),
        "quot" => into.push('"'),
        "apos" => into.push('\''),
        other => {
            into.push('&');
            into.push_str(other);
            into.push(';');
        }
    }
}

/// Count the bytes `reader` yields, stopping one past `ceiling` so a lie about
/// a declared size is still visible.
///
/// Nothing is kept: the gate needs the amount for every part it will not parse,
/// and holding those bytes would mean holding the whole of a package's media.
fn count_stream<R: Read>(reader: &mut R, ceiling: u64) -> Result<u64> {
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    while total <= ceiling {
        let want = ceiling
            .saturating_sub(total)
            .saturating_add(1)
            .min(buffer.len() as u64) as usize;
        let read = reader
            .read(&mut buffer[..want])
            .map_err(|error| LimitError::Zip {
                detail: error.to_string(),
            })?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
    }
    Ok(total)
}

/// Whether a ZIP part is XML a package reader may parse.
///
/// OOXML relationship parts (`.rels`), legacy VML drawings (`.vml`) and SVG
/// media are XML even though their names do not end in `.xml`; they need the
/// same size, nesting and `DOCTYPE` gate as the document parts.
fn is_xml_part(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with(".xml")
        || name.ends_with(".rels")
        || name.ends_with(".vml")
        || name.ends_with(".svg")
}

/// A wall-clock budget for one parser run.
#[derive(Debug, Clone)]
pub struct Deadline {
    start: Instant,
    limit: Duration,
}

impl Deadline {
    /// Return the stage's name when the budget is spent.
    pub fn check(&self, stage: &str) -> Result<()> {
        let elapsed = self.start.elapsed();
        if elapsed > self.limit {
            return Err(LimitError::Timeout {
                stage: stage.to_string(),
                elapsed,
                limit: self.limit,
            });
        }
        Ok(())
    }

    /// The time left before the budget is spent.
    pub fn remaining(&self) -> Duration {
        self.limit.saturating_sub(self.start.elapsed())
    }
}

/// The budgets one reader applies while it walks its input.
///
/// A guard pairs the time budget — checked as the walk proceeds, because a
/// deadline cannot be enforced after the fact — with the size budgets the
/// reader checks once it has something to measure. [`Guard::unbudgeted`] is the
/// no-op form, so a reader whose convenience entry point has always been
/// infallible keeps that signature for bytes the caller has already bounded.
#[derive(Debug, Clone)]
pub struct Guard<'a> {
    limits: Option<&'a Limits>,
    deadline: Option<Deadline>,
}

impl<'a> Guard<'a> {
    /// Carry `limits` through one reading.
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            limits: Some(limits),
            deadline: Some(limits.deadline()),
        }
    }

    /// A guard that refuses nothing.
    pub fn unbudgeted() -> Self {
        Self {
            limits: None,
            deadline: None,
        }
    }

    /// Whether this guard enforces anything.
    pub fn is_budgeted(&self) -> bool {
        self.limits.is_some()
    }

    /// Reject the input before the reader touches it.
    pub fn check_input(&self, len: usize) -> Result<()> {
        match self.limits {
            Some(limits) => limits.check_input(len),
            None => Ok(()),
        }
    }

    /// Reject a document that grew past the block budget.
    pub fn check_flow_blocks(&self, count: usize) -> Result<()> {
        match self.limits {
            Some(limits) => limits.check_flow_blocks(count),
            None => Ok(()),
        }
    }

    /// Check the time budget, naming the stage the walk is in.
    pub fn check(&self, stage: &str) -> Result<()> {
        match &self.deadline {
            Some(deadline) => deadline.check(stage),
            None => Ok(()),
        }
    }
}

/// The bytes one package's reads have decoded so far, and the budget they are
/// charged to.
///
/// A central directory is a claim, not a proof: a crafted entry can declare a
/// small uncompressed size and expand past it, and a package of many such
/// entries can expand past the package ceiling even when every entry is under
/// its own. Charging each read to one running total is what keeps the *actual*
/// expansion inside the budget the caller asked for, rather than trusting the
/// sizes the archive states.
#[derive(Debug)]
pub struct ZipBudget<'a> {
    limits: &'a Limits,
    decoded: u64,
}

impl<'a> ZipBudget<'a> {
    /// Charge reads to `limits`.
    pub fn new(limits: &'a Limits) -> Self {
        Self { limits, decoded: 0 }
    }

    /// The bytes decoded so far.
    pub fn decoded_bytes(&self) -> u64 {
        self.decoded
    }

    /// The most the next entry may produce: its own ceiling, or what is left
    /// of the package's.
    pub fn entry_ceiling(&self) -> u64 {
        self.limits
            .max_zip_entry_bytes
            .min(self.limits.max_zip_total_bytes.saturating_sub(self.decoded))
    }

    /// Read one entry, charged to this package's running total.
    ///
    /// A refusal names the budget that ran out: the entry's own ceiling, or the
    /// package total every entry shares.
    pub fn read_entry<R: Read>(&mut self, name: &str, file: &mut R) -> Result<Vec<u8>> {
        let ceiling = self.entry_ceiling();
        let mut bytes = Vec::new();
        file.take(ceiling.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| LimitError::Zip {
                detail: error.to_string(),
            })?;
        if bytes.len() as u64 > ceiling {
            if ceiling == self.limits.max_zip_entry_bytes {
                return Err(LimitError::ZipEntryTooLarge {
                    name: name.to_string(),
                    actual: bytes.len() as u64,
                    limit: self.limits.max_zip_entry_bytes,
                });
            }
            return Err(LimitError::ZipTotalTooLarge {
                actual: self.decoded.saturating_add(bytes.len() as u64),
                limit: self.limits.max_zip_total_bytes,
            });
        }
        self.decoded = self.decoded.saturating_add(bytes.len() as u64);
        Ok(bytes)
    }
}

/// The bytes one document has expanded to so far, and the budget they are
/// charged to.
///
/// The per-item ceilings bound a single stream or frame. A file of many items
/// each just inside its own ceiling would still expand past what the caller
/// allowed, so every stream and frame a reader decodes is charged here as well:
/// this is the whole-document total, the same idea as [`ZipBudget`]'s package
/// total but across formats rather than one archive.
#[derive(Debug)]
pub struct DocumentBudget<'a> {
    limits: &'a Limits,
    expanded: u64,
}

impl<'a> DocumentBudget<'a> {
    /// Charge expansions to `limits`.
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            expanded: 0,
        }
    }

    /// The bytes expanded so far.
    pub fn expanded_bytes(&self) -> u64 {
        self.expanded
    }

    /// Charge `bytes` to the document's total, refusing the item that passes it.
    pub fn charge(&mut self, bytes: u64) -> Result<()> {
        self.expanded = self.expanded.saturating_add(bytes);
        if self.expanded > self.limits.max_document_bytes {
            return Err(LimitError::DocumentTooLarge {
                actual: self.expanded,
                limit: self.limits.max_document_bytes,
            });
        }
        Ok(())
    }
}

/// A ZIP package whose entry reads are charged to one budget.
///
/// The archive and the running total travel together, and the archive itself is
/// private, so a reader cannot read a part outside its budget even by accident:
/// every read goes through [`Package::read_name`]. Listing is
/// [`Package::entries`], and the one pass that is not charged is the gate's, in
/// [`Package::gate`].
pub struct Package<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
    budget: ZipBudget<'a>,
    /// Whether every stored name decodes to its own bytes, decided on the
    /// first lookup that the raw index misses.
    ///
    /// [`zip`]'s lookup falls back to decoding every entry's name (CP437) when
    /// the raw bytes are not found: a scan per absent part. A package whose
    /// names all decode to themselves cannot match through that fallback — the
    /// decoded text is the bytes the raw index already searched — so one walk
    /// settles it for good instead of one walk per miss.
    names_decode_as_stored: Option<bool>,
}

impl<'a> Package<'a> {
    /// Open `data` as a ZIP whose reads are charged to `limits`.
    pub fn new(
        data: &'a [u8],
        limits: &'a Limits,
    ) -> std::result::Result<Self, zip::result::ZipError> {
        Ok(Self {
            archive: zip::ZipArchive::new(Cursor::new(data))?,
            budget: ZipBudget::new(limits),
            names_decode_as_stored: None,
        })
    }

    /// Run the gates a package must pass before a reader trusts it.
    ///
    /// The gate decompresses every entry itself, under the same ceilings, so a
    /// central directory that lies about its sizes cannot expand past the
    /// package budget. Those reads are the gate's own, not content the reader
    /// keeps, so they are not charged to the reader's running total; every part
    /// the reader takes goes through [`Package::read_name`] and is.
    pub fn gate(&mut self) -> Result<()> {
        self.budget.limits.check_zip(&mut self.archive)
    }

    /// Read the entry called `name`, charged to the package's budget.
    ///
    /// `None` when the entry is absent or the budget refuses it — the same
    /// reading a reader has always had for a part it cannot use.
    pub fn read_name(&mut self, name: &str) -> Option<Vec<u8>> {
        let index = self.index_of(name)?;
        let mut file = self.archive.by_index(index).ok()?;
        self.budget.read_entry(name, &mut file).ok()
    }

    /// The index of the entry called `name`.
    ///
    /// The raw byte lookup is a hash. The fallback behind it — decoding every
    /// name in the package — is what [`zip::ZipArchive::by_name`] runs on a
    /// miss, and a reader asks about parts a package may not have (footnotes,
    /// styles, a relationship file), so that fallback would be a scan of the
    /// whole directory per absent part. It is walked once here, and only for a
    /// package that stores a name decoding differently from its bytes, which
    /// is the one case it could match; otherwise a miss is final.
    fn index_of(&mut self, name: &str) -> Option<usize> {
        if let Some(index) = self.archive.index_for_name_raw(name.as_bytes()) {
            return Some(index);
        }
        match self.names_decode_as_stored {
            Some(true) => None,
            Some(false) => self.archive.index_for_name(name),
            None => {
                let (decoded_as_stored, found) = self.walk_names(name);
                self.names_decode_as_stored = Some(decoded_as_stored);
                found
            }
        }
    }

    /// One walk of the directory: whether every stored name decodes to its own
    /// bytes, and the first entry whose decoded name is `name`.
    fn walk_names(&mut self, name: &str) -> (bool, Option<usize>) {
        let count = self.archive.len();
        let mut decoded_as_stored = true;
        let mut found = None;
        for index in 0..count {
            let Ok(file) = self.archive.by_index_raw(index) else {
                continue;
            };
            let Ok(decoded) = file.name() else {
                continue;
            };
            if decoded.as_bytes() != file.name_raw() {
                decoded_as_stored = false;
            }
            if found.is_none() && decoded == name {
                found = Some(index);
            }
        }
        (decoded_as_stored, found)
    }

    /// Every entry's name and whether it is a directory, in stored order.
    ///
    /// An entry whose name cannot be decoded is left out: no reader can ask for
    /// it by name, so it could never become content. The gate refuses such a
    /// package outright; a reader that lists before it reads still gets the rest.
    pub fn entries(&mut self) -> Vec<(String, bool)> {
        let count = self.archive.len();
        (0..count)
            .filter_map(|index| {
                let file = self.archive.by_index_raw(index).ok()?;
                Some((file.name().ok()?.into_owned(), file.is_dir()))
            })
            .collect()
    }

    /// The bytes this package's reads have decoded so far.
    pub fn decoded_bytes(&self) -> u64 {
        self.budget.decoded_bytes()
    }
}

/// Why an input was refused.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum LimitError {
    #[error("the input is {actual} bytes; the limit is {limit} bytes")]
    InputTooLarge { actual: u64, limit: u64 },
    #[error("the document claims {actual} pages; the limit is {limit}")]
    TooManyPages { actual: usize, limit: usize },
    #[error("the PDF has {actual} objects; the limit is {limit}")]
    TooManyPdfObjects { actual: usize, limit: usize },
    #[error("the PDF presents more than {limit} tokens")]
    TooManyPdfTokens { actual: usize, limit: usize },
    #[error("the PDF nests {actual} levels deep; the limit is {limit}")]
    PdfTooDeep { actual: usize, limit: usize },
    #[error("a decoded PDF stream is {actual} bytes; the limit is {limit} bytes")]
    DecodedStreamTooLarge { actual: u64, limit: u64 },
    #[error("the ZIP has {actual} entries; the limit is {limit}")]
    TooManyZipEntries { actual: usize, limit: usize },
    #[error("ZIP entry {name:?} expands to {actual} bytes; the limit is {limit} bytes")]
    ZipEntryTooLarge {
        name: String,
        actual: u64,
        limit: u64,
    },
    #[error("ZIP entry {name:?} expands {ratio}:1; the limit is {limit}:1")]
    ZipRatioTooHigh {
        name: String,
        ratio: u64,
        limit: u64,
    },
    #[error("the ZIP expands to {actual} bytes; the limit is {limit} bytes")]
    ZipTotalTooLarge { actual: u64, limit: u64 },
    #[error("the ZIP package could not be read: {detail}")]
    Zip { detail: String },
    #[error("an XML part is {actual} bytes; the limit is {limit} bytes")]
    XmlTooLarge { actual: u64, limit: u64 },
    #[error("XML nests {actual} levels deep; the limit is {limit}")]
    XmlTooDeep { actual: usize, limit: usize },
    #[error("XML declarations (DOCTYPE) are not accepted: they can expand entities without bound")]
    XmlDoctype,
    #[error("the XML could not be read: {detail}")]
    XmlMalformed { detail: String },
    #[error("the image could not be read: {detail}")]
    ImageDecode { detail: String },
    #[error("the image has no pixels")]
    ImageEmpty,
    #[error("the image is {width}x{height}; each side is limited to {limit} pixels")]
    ImageTooWide { width: u32, height: u32, limit: u32 },
    #[error("the image is {width}x{height} ({pixels} pixels); the limit is {limit} pixels")]
    ImageTooManyPixels {
        width: u32,
        height: u32,
        pixels: u64,
        limit: u64,
    },
    #[error("the image decode needs {actual} bytes; the limit is {limit} bytes")]
    ImageAllocTooLarge { actual: u64, limit: u64 },
    #[error("the image has {actual} frames; the limit is {limit}")]
    TooManyImageFrames { actual: usize, limit: usize },
    #[error("the document produced {actual} blocks; the limit is {limit}")]
    TooManyFlowBlocks { actual: usize, limit: usize },
    #[error("the document expanded to {actual} bytes; the whole-document limit is {limit} bytes")]
    DocumentTooLarge { actual: u64, limit: u64 },
    #[error("LibreOffice wrote {actual} bytes for one document; the limit is {limit} bytes")]
    BridgeOutputTooLarge { actual: u64, limit: u64 },
    #[error("parsing timed out after {elapsed:?} during {stage}; the limit is {limit:?}")]
    Timeout {
        stage: String,
        elapsed: Duration,
        limit: Duration,
    },
}

/// The result of a guarded operation.
pub type Result<T> = std::result::Result<T, LimitError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_budget_accepts_a_modest_input() {
        let limits = Limits::default();
        assert!(limits.check_input(1024).is_ok());
        assert!(limits.check_pages(10).is_ok());
        assert!(limits.check_image(2_000, 3_000).is_ok());
    }

    /// The whole-document total charges every item, so a file of many items
    /// each inside its own ceiling cannot add up past what the caller allowed.
    #[test]
    fn a_document_total_refuses_the_item_that_passes_it() {
        let limits = Limits {
            max_document_bytes: 100,
            ..Limits::default()
        };
        let mut budget = limits.document_budget();
        budget.charge(60).expect("under the total");
        budget.charge(40).expect("exactly the total");
        assert_eq!(budget.expanded_bytes(), 100);
        let error = budget.charge(1).unwrap_err();
        assert!(matches!(
            error,
            LimitError::DocumentTooLarge {
                actual: 101,
                limit: 100
            }
        ));
        assert!(error.to_string().contains("whole-document"));
    }

    #[test]
    fn a_page_tree_over_budget_is_refused_by_name() {
        let limits = Limits {
            max_pages: 2,
            ..Limits::default()
        };
        let error = limits.check_pages(3).unwrap_err();
        assert!(matches!(error, LimitError::TooManyPages { actual: 3, .. }));
        assert!(error.to_string().contains("3"));
    }

    #[test]
    fn nesting_and_doctypes_are_refused() {
        let limits = Limits {
            max_xml_depth: 3,
            ..Limits::default()
        };
        let error = limits.check_xml(b"<a><b><c><d/></c></b></a>").unwrap_err();
        assert!(matches!(error, LimitError::XmlTooDeep { actual: 4, .. }));

        let error = limits
            .check_xml(b"<!DOCTYPE a [<!ENTITY x \"boom\">]><a>&x;</a>")
            .unwrap_err();
        assert_eq!(error, LimitError::XmlDoctype);
    }

    #[test]
    fn an_image_allocation_over_budget_is_named() {
        let limits = Limits {
            max_image_alloc_bytes: 1024,
            ..Limits::default()
        };
        let error = limits.check_image_alloc(1025).unwrap_err();
        assert!(matches!(
            error,
            LimitError::ImageAllocTooLarge {
                actual: 1025,
                limit: 1024
            }
        ));
    }

    #[test]
    fn a_flow_document_over_its_block_budget_is_refused_by_name() {
        let limits = Limits {
            max_flow_blocks: 2,
            ..Limits::default()
        };
        assert!(limits.check_flow_blocks(2).is_ok());
        let error = limits.check_flow_blocks(3).unwrap_err();
        assert!(matches!(
            error,
            LimitError::TooManyFlowBlocks {
                actual: 3,
                limit: 2
            }
        ));
        assert!(error.to_string().contains("3 blocks"), "{error}");
    }

    #[test]
    fn a_bridged_file_over_its_output_budget_is_refused_by_name() {
        let limits = Limits {
            max_bridge_output_bytes: 1024,
            ..Limits::default()
        };
        assert!(limits.check_bridge_output(1024).is_ok());
        let error = limits.check_bridge_output(1025).unwrap_err();
        assert!(matches!(
            error,
            LimitError::BridgeOutputTooLarge {
                actual: 1025,
                limit: 1024
            }
        ));
        assert!(error.to_string().contains("1025"), "{error}");
    }

    #[test]
    fn an_ocr_deadline_is_measured_against_its_own_budget() {
        let limits = Limits {
            ocr_timeout: Duration::ZERO,
            ..Limits::default()
        };
        let error = limits
            .ocr_deadline()
            .check("recognising a page")
            .unwrap_err();
        assert!(matches!(error, LimitError::Timeout { .. }));
        assert!(error.to_string().contains("recognising a page"), "{error}");
    }

    #[test]
    fn a_streamed_count_stops_one_byte_past_the_ceiling() {
        let mut data = Cursor::new(vec![b'x'; 4096]);
        assert_eq!(count_stream(&mut data, 100).unwrap(), 101);
        let mut data = Cursor::new(vec![b'x'; 10]);
        assert_eq!(count_stream(&mut data, 100).unwrap(), 10);
    }

    #[test]
    fn a_custom_entry_budget_governs_a_zip_read() {
        let limits = Limits {
            max_zip_entry_bytes: 8,
            ..Limits::default()
        };
        let mut budget = ZipBudget::new(&limits);
        let error = budget
            .read_entry("a.xml", &mut Cursor::new(vec![b'x'; 16]))
            .expect_err("the entry is twice the ceiling");
        assert!(
            matches!(error, LimitError::ZipEntryTooLarge { limit: 8, .. }),
            "{error}"
        );
        assert!(error.to_string().contains("a.xml"), "{error}");
        assert_eq!(budget.decoded_bytes(), 0, "a refused read is not counted");
    }

    #[test]
    fn a_package_total_governs_the_reads_that_follow() {
        let limits = Limits {
            max_zip_entry_bytes: 64,
            max_zip_total_bytes: 100,
            ..Limits::default()
        };
        let mut budget = ZipBudget::new(&limits);
        let first = budget
            .read_entry("a.xml", &mut Cursor::new(vec![b'x'; 60]))
            .expect("under both budgets");
        assert_eq!(first.len(), 60);
        assert_eq!(budget.decoded_bytes(), 60);
        assert_eq!(budget.entry_ceiling(), 40, "what is left of the package");

        let error = budget
            .read_entry("b.xml", &mut Cursor::new(vec![b'x'; 60]))
            .expect_err("the second entry would pass the package total");
        assert!(
            matches!(error, LimitError::ZipTotalTooLarge { limit: 100, .. }),
            "{error}"
        );
        assert!(error.to_string().contains("limit is 100 bytes"), "{error}");
        assert_eq!(budget.decoded_bytes(), 60);
    }

    #[test]
    fn a_guard_enforces_what_its_budget_says_and_an_unbudgeted_one_refuses_nothing() {
        let limits = Limits {
            max_input_bytes: 4,
            max_flow_blocks: 1,
            timeout: Duration::ZERO,
            ..Limits::default()
        };
        let guard = limits.guard();
        assert!(matches!(
            guard.check_input(5),
            Err(LimitError::InputTooLarge { actual: 5, .. })
        ));
        assert!(matches!(
            guard.check_flow_blocks(2),
            Err(LimitError::TooManyFlowBlocks { actual: 2, .. })
        ));
        assert!(matches!(
            guard.check("reading a block"),
            Err(LimitError::Timeout { .. })
        ));

        let guard = Guard::unbudgeted();
        assert!(!guard.is_budgeted());
        assert!(guard.check_input(usize::MAX).is_ok());
        assert!(guard.check_flow_blocks(usize::MAX).is_ok());
        assert!(guard.check("reading a block").is_ok());
    }

    #[test]
    fn a_timed_out_deadline_names_its_stage() {
        let limits = Limits {
            timeout: Duration::ZERO,
            ..Limits::default()
        };
        let error = limits.deadline().check("reading a page").unwrap_err();
        assert!(matches!(error, LimitError::Timeout { .. }));
        assert!(error.to_string().contains("reading a page"));
    }
}
