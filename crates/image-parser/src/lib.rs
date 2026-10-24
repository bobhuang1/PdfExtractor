//! Plain images as documents: one image is one page.
//!
//! An image has no text layer, so everything the document says comes from OCR.
//! What this crate owns is the page: which format the bytes are, how large the
//! picture is in points, and at what resolution it says it was made. That is
//! the same reading Docling's own image backend takes — the page is the pixel
//! grid divided by the resolution the file declares, and a file that declares
//! none is 72 dpi, one pixel per point.
//!
//! A camera also says which way round it was held: the EXIF orientation tag.
//! The stored grid is not the page when that tag is not 1 — a phone held
//! sideways writes a landscape grid of a portrait page, or an upside-down one
//! — so the page is measured from the upright grid, and [`Orientation`] is
//! reported for the reader of the pixels to apply.
//!
//! PNG, JPEG, TIFF, BMP, WebP, GIF and JPEG 2000 are read from their headers
//! alone; the pixels are only touched when a tag says they are not upright or
//! when the encoding has to be decoded for a reader that cannot open it. [`turn_upright`]
//! returns a picture re-encoded with the tag's turn applied, for a consumer
//! that writes the picture out — a picture a Word document or a deck carries,
//! for instance — and [`upright_png`] returns the same pixels as a lossless
//! PNG, which is the form an OCR engine has to see. A TIFF, a GIF or a WebP can
//! hold several frames. [`ImageDocument`] still describes the still: for a GIF
//! the logical screen its descriptor states — the canvas every frame is placed
//! on — with the first frame drawn on it, which is what Pillow, and so Docling's
//! own backend, reads from the file, and for a WebP the canvas its `ANIM` chunk
//! states. A GIF or a WebP arriving as the *input* is the whole animation,
//! though, and each of its frames is a page a reader turns, so [`gif_frames`]
//! and [`webp_frames`] return every one of them, composited on that screen. A
//! TIFF is the same story in a different shape: its chain of image file
//! directories is a batch of sheets, and [`tiff_frames`] returns every page at
//! its own grid.
//!
//! WebP and JPEG 2000 are carried by the document model but cannot be named in
//! Docling's `ImageRef`, so a picture in either encoding is transcoded to PNG
//! by [`transcode_to_png`] before it reaches the model — a picture a reader can
//! open, instead of a box with no image. That is the picture *inside* a
//! document; either file is also one page on its own, because a page's name for
//! its source is not the reference Docling validates. A JPEG 2000 page is either
//! spelling: the JP2 container (`.jp2`), whose header boxes state the grid and,
//! when the writer states one, the capture resolution — a scan declaring 150 dpi
//! is placed at that resolution rather than at one pixel per point — or the bare
//! codestream (`.j2c`, `.j2k`, `.jpc`, `.jpf`), whose `SIZ` marker states the grid
//! and which has no container to declare a density, so it is one pixel per point.
//! Which spelling a file carries is the file's own answer: the first bytes are a
//! signature box or a codestream's start marker, and the name it is saved under
//! does not decide.

mod parser;

pub use parser::{
    gif_frames, is_document_media_type, mimetype, tiff_frames, transcode_to_png, turn_upright,
    upright_png, webp_frames, ImageDocument, Orientation, PngPicture, UprightPicture, DEFAULT_DPI,
    DOCUMENT_MEDIA_TYPES,
};
