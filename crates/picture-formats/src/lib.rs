//! The picture formats this carries, in one place.
//!
//! A picture format has two names: the media type the document names it by
//! (`image/png`) and the extension its bytes are written under (`png`). Two
//! sides of the extractor need both — the flow-format reader decides which
//! `<source type="…">` declaration it can take, and the CLI decides what to call
//! the file it extracts — and if each kept its own list a format added to one
//! would silently stop halfway through the other: a picture the reader accepted
//! would be written as `.bin`, or one the CLI knew how to write would be passed
//! over in a page. Both therefore read [`PICTURE_FORMATS`], and neither has a
//! list of its own.
//!
//! "Carried" is the whole of the claim, not "decodable": a format here is one
//! whose bytes travel through the document model and out to a file unchanged —
//! a PNG, a JPEG, an SVG, a Windows metafile. Whether the pixels can be read is
//! a different question, answered by the image and metafile parsers.

/// One picture format the extractor carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureFormat {
    /// The media type the document names the picture by, as Docling writes it.
    pub mime: &'static str,
    /// The extension an extracted picture is written under.
    pub extension: &'static str,
    /// Other extensions that name the same format when a file is read: an
    /// outside file may be a `.jpeg` even though this writes `.jpg`.
    pub read_as: &'static [&'static str],
}

/// Every picture format that travels through the extractor.
///
/// One entry is one format on both sides: the reader takes a `<source>` that
/// declares [`PictureFormat::mime`], and the CLI writes such a picture under
/// [`PictureFormat::extension`] and reads that extension back as the same
/// media type. Adding a format here adds it to both; there is nowhere else to
/// add it.
pub const PICTURE_FORMATS: &[PictureFormat] = &[
    PictureFormat {
        mime: "image/png",
        extension: "png",
        read_as: &[],
    },
    PictureFormat {
        mime: "image/jpeg",
        extension: "jpg",
        read_as: &["jpeg"],
    },
    PictureFormat {
        mime: "image/jp2",
        // A JPEG 2000 travels in two spellings: the JP2 container, and the bare
        // codestream a scanner or an encoder writes under `.j2c`, `.j2k`,
        // `.jpc` or `.jpf`. The extension is only a name here: the image reader
        // reads the bytes and states the page from whichever header is there —
        // a container's `ihdr` and `resc`, or a codestream's `SIZ` — so a
        // `.jpf` holding a codestream and a `.jpc` holding a container are both
        // read as what they are.
        extension: "jp2",
        read_as: &["j2c", "j2k", "jpc", "jpf"],
    },
    PictureFormat {
        mime: "image/gif",
        extension: "gif",
        read_as: &[],
    },
    PictureFormat {
        mime: "image/bmp",
        extension: "bmp",
        read_as: &[],
    },
    PictureFormat {
        mime: "image/tiff",
        extension: "tif",
        read_as: &["tiff"],
    },
    PictureFormat {
        mime: "image/webp",
        extension: "webp",
        read_as: &[],
    },
    PictureFormat {
        mime: "image/svg+xml",
        extension: "svg",
        read_as: &[],
    },
    PictureFormat {
        mime: "image/emf",
        extension: "emf",
        read_as: &[],
    },
    PictureFormat {
        mime: "image/wmf",
        extension: "wmf",
        read_as: &[],
    },
];

/// The format a media type names, if it is one this carries.
///
/// The type is matched exactly: stripping `; charset=…` and folding case is the
/// caller's job, because a caller that has to decide what the declaration *is*
/// has to look at it anyway.
pub fn format_for_mime(mime: &str) -> Option<&'static PictureFormat> {
    PICTURE_FORMATS.iter().find(|format| format.mime == mime)
}

/// Whether a media type is a picture this carries.
pub fn is_carried(mime: &str) -> bool {
    format_for_mime(mime).is_some()
}

/// The extension a media type is written under, if it is one this carries.
pub fn extension_for_mime(mime: &str) -> Option<&'static str> {
    format_for_mime(mime).map(|format| format.extension)
}

/// The format a file extension names, if it is one this carries.
///
/// Case does not matter: a file saved as `SCAN.JPG` is the same format as one
/// saved as `scan.jpg`, and the extension is the only thing a caller has.
pub fn format_for_extension(extension: &str) -> Option<&'static PictureFormat> {
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    PICTURE_FORMATS.iter().find(|format| {
        format.extension == extension || format.read_as.contains(&extension.as_str())
    })
}

/// The media type a file extension names, if it is one this carries.
pub fn mime_for_extension(extension: &str) -> Option<&'static str> {
    format_for_extension(extension).map(|format| format.mime)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every format is the same format whichever way it is named: a media type
    /// leads to its extension and back, an extension leads to its media type
    /// and back, and every alias leads to the format it belongs to.
    #[test]
    fn every_format_round_trips_through_its_names() {
        for format in PICTURE_FORMATS {
            assert_eq!(
                extension_for_mime(format.mime),
                Some(format.extension),
                "{}",
                format.mime
            );
            assert_eq!(
                mime_for_extension(format.extension),
                Some(format.mime),
                "{}",
                format.extension
            );
            for alias in format.read_as {
                assert_eq!(
                    format_for_extension(alias).map(|found| found.mime),
                    Some(format.mime),
                    "{alias} names {}",
                    format.mime
                );
            }
        }
    }

    /// One format, one name: two entries with the same media type or the same
    /// extension would make "the" format ambiguous.
    #[test]
    fn no_two_formats_share_a_name() {
        for (index, format) in PICTURE_FORMATS.iter().enumerate() {
            for other in &PICTURE_FORMATS[index + 1..] {
                assert_ne!(format.mime, other.mime, "two formats named by one type");
                assert_ne!(
                    format.extension, other.extension,
                    "two formats written under one extension"
                );
                assert!(
                    !other.read_as.contains(&format.extension),
                    "{} is another format's extension",
                    format.extension
                );
                assert!(
                    !format.read_as.contains(&other.extension),
                    "{} reads another format's extension",
                    other.extension
                );
            }
        }
    }

    /// A format outside the list is carried by neither side, however image-like
    /// it is.
    #[test]
    fn a_format_outside_the_list_is_not_carried() {
        assert!(!is_carried("image/avif"));
        assert!(!is_carried("image/heic"));
        assert!(!is_carried("text/html"));
        assert!(!is_carried("image/"));
        assert_eq!(mime_for_extension("avif"), None);
        assert_eq!(mime_for_extension("bin"), None);
        assert_eq!(extension_for_mime("image/avif"), None);
    }

    /// A file's extension is not case-sensitive, and a caller that hands over
    /// the dot writes the same file as one that does not.
    #[test]
    fn an_extension_is_read_in_any_case() {
        assert_eq!(mime_for_extension("JPG"), Some("image/jpeg"));
        assert_eq!(mime_for_extension(".JpG"), Some("image/jpeg"));
        assert_eq!(mime_for_extension("TIFF"), Some("image/tiff"));
    }
}
