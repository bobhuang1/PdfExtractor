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

use std::io::{Read, Seek};
use std::time::{Duration, Instant};

use quick_xml::events::Event;
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
/// Wall-clock budget for one parser, from entry to return.
pub const PARSE_TIMEOUT: Duration = Duration::from_secs(30);

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
    pub timeout: Duration,
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
            timeout: PARSE_TIMEOUT,
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

    /// A deadline measured from now, for one parser run.
    pub fn deadline(&self) -> Deadline {
        Deadline {
            start: Instant::now(),
            limit: self.timeout,
        }
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
                    entry.name().to_string(),
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
            let mut bytes = Vec::new();
            file.by_ref()
                .take(ceiling.saturating_add(1))
                .read_to_end(&mut bytes)
                .map_err(|error| LimitError::Zip {
                    detail: error.to_string(),
                })?;
            if bytes.len() as u64 > ceiling {
                if ceiling == self.max_zip_entry_bytes {
                    return Err(LimitError::ZipEntryTooLarge {
                        name,
                        actual: bytes.len() as u64,
                        limit: self.max_zip_entry_bytes,
                    });
                }
                return Err(LimitError::ZipTotalTooLarge {
                    actual: actual_total.saturating_add(bytes.len() as u64),
                    limit: self.max_zip_total_bytes,
                });
            }
            actual_total = actual_total.saturating_add(bytes.len() as u64);
            if is_xml_part(&name) {
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

/// Read one ZIP entry with the package-entry ceiling applied.
///
/// A central directory is a claim, not a proof: a crafted entry can declare a
/// small uncompressed size and expand past it. This caps the read itself, so a
/// lying entry cannot allocate without bound even after preflight.
pub fn read_zip_entry<R: Read>(file: &mut R) -> std::io::Result<Vec<u8>> {
    read_zip_entry_with_limit(file, MAX_ZIP_ENTRY_BYTES)
}

/// [`read_zip_entry`] with an explicit ceiling, for a caller with a tighter
/// budget.
pub fn read_zip_entry_with_limit<R: Read>(file: &mut R, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("the ZIP entry expands beyond the {limit}-byte limit"),
        ));
    }
    Ok(bytes)
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
