//! Render a [`DoclingDocument`] as retrieval chunks: JSON Lines records shaped
//! like `docling_core.transforms.chunker.doc_chunk.DocChunk`.
//!
//! A chunk is what a RAG pipeline indexes: a span of text small enough to embed,
//! carrying enough of the document around it that a hit can be cited. Docling
//! has two chunkers, and both are here:
//!
//! * [`ChunkKind::Hierarchical`](crate::chunks::ChunkKind::Hierarchical) follows the body tree. One visible item is one
//!   chunk — a paragraph, a heading, a formula — with consecutive list items
//!   written as one chunk (their lines are one list, and splitting a list
//!   between chunks loses the fact that they belong together). Each chunk
//!   carries the heading path it sits under, which is what tells an embedding
//!   model that a paragraph is about "Regional performance" and not about the
//!   document as a whole. An item is never cut: a table is one chunk, its pipe
//!   grid whole, and a picture with no caption is no chunk at all — there are
//!   no words to retrieve it by.
//! * [`ChunkKind::Hybrid`](crate::chunks::ChunkKind::Hybrid) packs the same items into chunks of at most
//!   `max_tokens`, sentence by sentence, so a chunk is never a sentence broken
//!   in the middle. A sentence longer than the whole budget is its own chunk
//!   rather than being truncated. `overlap_tokens` carries the tail sentences
//!   of one chunk into the next, so a passage that straddles a boundary is
//!   retrievable from either side. The heading path is written into the chunk
//!   text as Markdown headings, the way Docling's `contextualize` writes it.
//!
//! The record is Docling's: a `text`, and a `meta` carrying the `doc_items` the
//! span came from (each with its `self_ref`, `label` and provenance — page and
//! box — so a hit can be cited back to the page), the `headings` path, the
//! `captions` a picture or table carries, and the document's `origin`.
//!
//! Two things are deliberately ours rather than Docling's:
//!
//! * **Tables** are written as the same pipe grid the Markdown and text writers
//!   share. Docling serialises a table its own way in a chunk; a pipe grid is
//!   what a reader (and an LLM) can follow, and the two writers already agree on
//!   which cell holds what.
//! * **Token counting** defaults to a documented approximation: [`count_tokens`](crate::chunks::count_tokens)
//!   splits on whitespace and counts each word as one token per four characters.
//!   That is stable and identical on every machine, but it is not a model's own
//!   count. When [`ChunkOptions::tokenizer`](crate::chunks::ChunkOptions::tokenizer)
//!   carries a [`Tokenizer`](crate::chunks::Tokenizer) loaded from
//!   a Hugging Face `tokenizer.json`, a SentencePiece `.model`, or a tiktoken
//!   vocabulary file, hybrid budgets are counted in that model's tokens instead.
//!   The approximation remains the default, so a budget is reproducible unless
//!   the caller deliberately supplies the model whose budget it is. A
//!   `.tiktoken` rank file carries no split pattern, so its filename names the
//!   encoding family;
//!   [`Tokenizer::from_file_with_tiktoken_encoding`](crate::chunks::Tokenizer::from_file_with_tiktoken_encoding)
//!   names it
//!   explicitly for a vocabulary whose filename says nothing.
//!
//! A body item on the notes layer (a deck's speaker notes, a comment) is left
//! out unless [`ChunkOptions::include_notes`](crate::chunks::ChunkOptions::include_notes)
//! asks for it, exactly as the
//! Markdown and text writers leave it out; page furniture is always left out,
//! because a running head repeated on every page is not something to retrieve.

use crate::walk::{walk_body_with, Visitor};
use base64::Engine as _;
use pdfextractor_core::{
    DoclingDocument, Group, Origin, PictureItem, Provenance, TableItem, TextItem,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

/// Docling's own schema name for the metadata of a chunk.
pub const META_SCHEMA: &str = "docling_core.transforms.chunker.DocMeta";
/// The version Docling writes beside that name.
pub const META_VERSION: &str = "1.0.0";

/// One chunk: the text a reader indexes, and what it came from.
#[derive(Debug, Clone, Serialize)]
pub struct DocChunk {
    pub text: String,
    pub meta: DocMeta,
}

/// A chunk's provenance, shaped like Docling's `DocMeta`.
#[derive(Debug, Clone, Serialize)]
pub struct DocMeta {
    pub schema_name: String,
    pub version: String,
    /// The document items this chunk holds, in reading order. A chunk always
    /// names at least one: a span with no item behind it could not be cited.
    pub doc_items: Vec<DocItem>,
    /// The headings the items sit under, outermost first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headings: Option<Vec<String>>,
    /// The caption text of a picture or table, when the document states one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// One document item inside a chunk, shaped like Docling's `DocItem`.
#[derive(Debug, Clone, Serialize)]
pub struct DocItem {
    pub self_ref: String,
    /// The item's parent, when it has one — `#/body` or a group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Cref>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Cref>,
    pub content_layer: String,
    /// Docling's name for the item's kind. A paragraph is `text` there, which
    /// is the one document label this maps: the rest are already Docling's.
    pub label: String,
    #[serde(default)]
    pub prov: Vec<Provenance>,
}

/// A pointer to another item, in Docling's `cref` spelling.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cref {
    pub cref: String,
}

/// How a document is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChunkKind {
    /// One chunk per body item (list runs together), headings carried.
    #[default]
    Hierarchical,
    /// Sentences packed to a token budget, with overlap.
    Hybrid,
}

/// The reader's choices for [`chunk_document`].
#[derive(Debug, Clone)]
pub struct ChunkOptions {
    pub kind: ChunkKind,
    /// The most tokens a hybrid chunk may hold. Ignored by
    /// [`ChunkKind::Hierarchical`], which never cuts an item.
    pub max_tokens: usize,
    /// How many trailing tokens of a hybrid chunk are repeated at the start of
    /// the next, so a passage across a boundary is in both.
    pub overlap_tokens: usize,
    /// A real tokenizer whose model counts a hybrid chunk's tokens. `None`
    /// measures budgets with [`count_tokens`]'s documented approximation.
    pub tokenizer: Option<Arc<Tokenizer>>,
    /// Include a deck's speaker notes and comments, as `--include-notes` does
    /// for the prose readings.
    pub include_notes: bool,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            kind: ChunkKind::Hierarchical,
            max_tokens: 512,
            overlap_tokens: 0,
            tokenizer: None,
            include_notes: false,
        }
    }
}

/// Split `doc` into chunks.
pub fn chunk_document(doc: &DoclingDocument, options: ChunkOptions) -> Vec<DocChunk> {
    let mut collector = Collector::new(doc);
    walk_body_with(doc, options.include_notes, &mut collector);
    let pieces = collector.pieces;
    match options.kind {
        ChunkKind::Hierarchical => hierarchical(&pieces, doc),
        ChunkKind::Hybrid => hybrid(&pieces, doc, &options),
    }
}

/// The chunks of `doc` as JSON Lines: one record per line, each a whole JSON
/// object, so a pipeline can read the file a line at a time.
pub fn to_jsonl(doc: &DoclingDocument, options: &ChunkOptions) -> anyhow::Result<String> {
    let mut out = String::new();
    for chunk in chunk_document(doc, options.clone()) {
        out.push_str(&serde_json::to_string(&chunk)?);
        out.push('\n');
    }
    Ok(out)
}

/// Write the chunks of `doc` to `path` as JSON Lines.
///
/// The bytes land in a sibling temporary file that is renamed into place, the
/// same contract every other writer keeps: a stopped run leaves whole files or
/// none, never a half-written `.jsonl` that still looks like a chunk file.
pub fn export_to_file(
    doc: &DoclingDocument,
    path: &Path,
    options: &ChunkOptions,
) -> anyhow::Result<()> {
    let jsonl = to_jsonl(doc, options)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.jsonl".to_string());
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));

    std::fs::write(&temp, jsonl)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

/// The approximate token count a budget is measured in when no tokenizer file
/// is supplied.
///
/// Whitespace separates words, and a word costs one token per four characters
/// (at least one), which is a stable stand-in for a byte-pair tokenizer: it
/// over-counts `information` and under-counts `a`, and is the same number on
/// every machine. It remains public so a caller can compare a real tokenizer's
/// count with the reproducible default.
pub fn count_tokens(text: &str) -> usize {
    text.split_whitespace()
        .map(|word| word.chars().count().div_ceil(4).max(1))
        .sum()
}

/// The three tokenizer files the chunker accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizerFormat {
    HuggingFace,
    SentencePiece,
    Tiktoken,
}

impl TokenizerFormat {
    /// The format's name, as a reader chooses between the three.
    pub fn label(self) -> &'static str {
        match self {
            TokenizerFormat::HuggingFace => "Hugging Face",
            TokenizerFormat::SentencePiece => "SentencePiece",
            TokenizerFormat::Tiktoken => "tiktoken",
        }
    }
}

/// The cl100k_base split pattern, used by OpenAI's tiktoken vocabularies when
/// the file itself cannot name a pattern. A `.tiktoken` file carries only ranks;
/// the pattern belongs to the encoding family, not the rank file.
const CL100K_PATTERN: &str = r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+";

/// The p50k_base/r50k_base split pattern, the older GPT-3-era family.
const P50K_PATTERN: &str = r"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+";

/// The o200k_base split pattern, selected by the vocabulary's own name because
/// that is the only place a `.tiktoken` file says which family it belongs to.
const O200K_PATTERN: &str = concat!(
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+",
    r"(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    r"|[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*",
    r"(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    r"|\p{N}{1,3}",
    r"| ?[^\s\p{L}\p{N}]+[\r\n/]*",
    r"|\s*[\r\n]+",
    r"|\s+",
);

/// The OpenAI tiktoken encoding family a `.tiktoken` rank file belongs to.
///
/// A rank file carries tokens and ranks but no split pattern, so the family is
/// normally read from the filename. Naming it explicitly lets a vocabulary
/// whose filename says nothing be loaded as the family it really is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiktokenEncoding {
    /// `cl100k_base`, the GPT-4/GPT-3.5-turbo family.
    Cl100k,
    /// `o200k_base`, the GPT-4o family.
    O200k,
    /// `p50k_base`, the older GPT-3 code family.
    P50k,
    /// `r50k_base`, the GPT-2/GPT-3 family.
    R50k,
}

impl TiktokenEncoding {
    /// The name tiktoken gives this family.
    pub fn name(self) -> &'static str {
        match self {
            TiktokenEncoding::Cl100k => "cl100k_base",
            TiktokenEncoding::O200k => "o200k_base",
            TiktokenEncoding::P50k => "p50k_base",
            TiktokenEncoding::R50k => "r50k_base",
        }
    }

    /// The split pattern the family's encodings use.
    fn pattern(self) -> &'static str {
        match self {
            TiktokenEncoding::Cl100k => CL100K_PATTERN,
            TiktokenEncoding::O200k => O200K_PATTERN,
            TiktokenEncoding::P50k | TiktokenEncoding::R50k => P50K_PATTERN,
        }
    }

    /// The family a name states, with the short aliases a caller may type.
    fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "cl100k_base" | "cl100k" => Some(TiktokenEncoding::Cl100k),
            "o200k_base" | "o200k" => Some(TiktokenEncoding::O200k),
            "p50k_base" | "p50k" => Some(TiktokenEncoding::P50k),
            "r50k_base" | "r50k" | "gpt2" => Some(TiktokenEncoding::R50k),
            _ => None,
        }
    }
}

impl std::str::FromStr for TiktokenEncoding {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::from_name(name).ok_or_else(|| {
            format!(
                "unknown tiktoken encoding family {name:?}; expected cl100k_base, o200k_base, \
                 p50k_base or r50k_base"
            )
        })
    }
}

impl std::fmt::Display for TiktokenEncoding {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.name())
    }
}

/// A real tokenizer loaded from a Hugging Face `tokenizer.json`, a SentencePiece
/// `.model`, or a tiktoken `.tiktoken` vocabulary.
///
/// The file is the model's vocabulary and pipeline — not a model name resolved
/// over a network. A tiktoken rank file has no split pattern of its own, so its
/// name chooses the OpenAI family pattern, or an explicit [`TiktokenEncoding`]
/// overrides that choice; the default is cl100k_base.
pub struct Tokenizer {
    inner: TokenizerKind,
    /// What the window names beside the format: the vocabulary size, or the
    /// tiktoken family a rank file belongs to.
    description: String,
}

enum TokenizerKind {
    HuggingFace(Box<tokenizers::Tokenizer>),
    // The SentencePiece processor is much larger than the other variants, and
    // a tokenizer is built once per run: boxing it keeps every `Tokenizer`
    // (and every chunk option holding one) small.
    SentencePiece(Box<sentencepiece_rs::SentencePieceProcessor>),
    Tiktoken(tiktoken_rs::CoreBPE),
}

impl std::fmt::Debug for Tokenizer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Tokenizer")
    }
}

impl Tokenizer {
    /// Load a tokenizer file from `path`.
    ///
    /// The extension names the format, so a malformed `tokenizer.json` fails as
    /// JSON rather than being mistaken for a binary model. A name with no
    /// recognized extension is recognized by its contents.
    ///
    /// A missing, unreadable or malformed file is an error here, before any
    /// document is converted, rather than a budget quietly falling back to the
    /// approximation.
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::from_file_with_tiktoken_encoding(path, None)
    }

    /// Load a tokenizer file, stating the encoding family when it is a
    /// `.tiktoken` vocabulary whose filename does not.
    ///
    /// `encoding` overrides the family the filename would select and is refused
    /// for any other tokenizer format; `None` keeps the filename's choice
    /// (cl100k_base when the name says nothing).
    pub fn from_file_with_tiktoken_encoding(
        path: impl AsRef<Path>,
        encoding: Option<TiktokenEncoding>,
    ) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let format = detect_tokenizer_format(path)?;
        if encoding.is_some() && format != TokenizerFormat::Tiktoken {
            anyhow::bail!(
                "{}: an encoding family only applies to a .tiktoken vocabulary",
                path.display()
            );
        }
        let tiktoken_family =
            (format == TokenizerFormat::Tiktoken).then(|| tiktoken_encoding(path, encoding));
        let mut tiktoken_vocab_size = None;
        let inner = match format {
            TokenizerFormat::HuggingFace => TokenizerKind::HuggingFace(Box::new(
                tokenizers::Tokenizer::from_file(path)
                    .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?,
            )),
            TokenizerFormat::SentencePiece => {
                let bytes = std::fs::read(path)
                    .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
                TokenizerKind::SentencePiece(Box::new(
                    sentencepiece_rs::SentencePieceProcessor::from_serialized_model(&bytes)
                        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?,
                ))
            }
            TokenizerFormat::Tiktoken => {
                let bytes = std::fs::read(path)
                    .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
                let (tokenizer, vocab_size) = load_tiktoken(path, &bytes, tiktoken_family)?;
                tiktoken_vocab_size = Some(vocab_size);
                TokenizerKind::Tiktoken(tokenizer)
            }
        };
        let description = match &inner {
            TokenizerKind::HuggingFace(tokenizer) => format!(
                "{} · {} tokens",
                format.label(),
                tokenizer.get_vocab_size(true)
            ),
            TokenizerKind::SentencePiece(tokenizer) => {
                format!(
                    "{} · {} tokens",
                    format.label(),
                    tokenizer.model().vocab_size()
                )
            }
            TokenizerKind::Tiktoken(_) => format!(
                "{} · {} · {} tokens",
                format.label(),
                tiktoken_family
                    .expect("a tiktoken format has an encoding family")
                    .name(),
                tiktoken_vocab_size.expect("a tiktoken format has a vocabulary size")
            ),
        };
        Ok(Self { inner, description })
    }

    /// What the badge beside the chosen file names: its vocabulary size. A
    /// tiktoken rank file also names the encoding family it was loaded as.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The model's own count for `text`, without special tokens — the count
    /// Docling's tokenizer uses for a chunk budget.
    ///
    /// A span the model's own pipeline refuses falls back to [`count_tokens`],
    /// so one pathological string cannot abort a whole export.
    pub fn count_tokens(&self, text: &str) -> usize {
        match &self.inner {
            TokenizerKind::HuggingFace(tokenizer) => tokenizer
                .encode(text, false)
                .map(|encoding| encoding.get_ids().len())
                .unwrap_or_else(|_| count_tokens(text)),
            TokenizerKind::SentencePiece(tokenizer) => tokenizer
                .encode_to_ids(text)
                .map(|ids| ids.len())
                .unwrap_or_else(|_| count_tokens(text)),
            TokenizerKind::Tiktoken(tokenizer) => tokenizer.encode_ordinary(text).len(),
        }
    }
}

/// The format named by the extension, or by the bytes when the extension is
/// unfamiliar.
///
/// This is the same question [`Tokenizer::from_file`] asks before it loads the
/// file, exposed for a caller that only needs to name the format — the window
/// shows it beside the chosen tokenizer file.
pub fn detect_tokenizer_format(path: impl AsRef<Path>) -> anyhow::Result<TokenizerFormat> {
    let path = path.as_ref();
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => Ok(TokenizerFormat::HuggingFace),
        Some("model" | "spm") => Ok(TokenizerFormat::SentencePiece),
        Some("tiktoken") => Ok(TokenizerFormat::Tiktoken),
        _ => {
            let bytes = std::fs::read(path)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
            if looks_like_tiktoken(&bytes) {
                Ok(TokenizerFormat::Tiktoken)
            } else if looks_like_json(&bytes) {
                Ok(TokenizerFormat::HuggingFace)
            } else if looks_like_sentencepiece(&bytes) {
                Ok(TokenizerFormat::SentencePiece)
            } else {
                anyhow::bail!(
                    "{}: not a tokenizer.json, SentencePiece .model or tiktoken vocabulary",
                    path.display()
                )
            }
        }
    }
}

fn looks_like_json(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b'{')
}

fn looks_like_sentencepiece(bytes: &[u8]) -> bool {
    // A ModelProto starts with `pieces` (field 1, length-delimited), so a
    // non-empty model's first key is 0x0a. The parser remains the authority:
    // this only decides which error a malformed file gets.
    bytes.first() == Some(&0x0a)
}

fn looks_like_tiktoken(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    text.lines()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| {
            let mut fields = line.split_whitespace();
            matches!(
                (fields.next(), fields.next(), fields.next()),
                (Some(token), Some(rank), None)
                    if base64::engine::general_purpose::STANDARD
                        .decode(token)
                        .is_ok()
                        && rank.parse::<u32>().is_ok()
            )
        })
}

/// Read a `.tiktoken` rank file: one base64 token and its integer rank per
/// line. The file has no pattern or special tokens, so both are supplied here.
fn load_tiktoken(
    path: &Path,
    bytes: &[u8],
    encoding: Option<TiktokenEncoding>,
) -> anyhow::Result<(tiktoken_rs::CoreBPE, usize)> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| anyhow::anyhow!("{}: not UTF-8 text: {error}", path.display()))?;
    let mut ranks = HashMap::new();
    let mut seen_ranks = HashSet::new();
    for (line_no, raw) in text.lines().enumerate() {
        let number = line_no + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(token), Some(rank), None) = (fields.next(), fields.next(), fields.next()) else {
            anyhow::bail!(
                "{}:{number}: expected one base64 token and one rank",
                path.display()
            );
        };
        let token = base64::engine::general_purpose::STANDARD
            .decode(token)
            .map_err(|error| {
                anyhow::anyhow!("{}:{number}: invalid base64 token: {error}", path.display())
            })?;
        let rank: u32 = rank.parse().map_err(|error| {
            anyhow::anyhow!("{}:{number}: invalid rank: {error}", path.display())
        })?;
        if !seen_ranks.insert(rank) {
            anyhow::bail!("{}:{number}: rank {rank} appears twice", path.display());
        }
        ranks.insert(token, rank);
    }
    if ranks.is_empty() {
        anyhow::bail!("{}: the tiktoken vocabulary is empty", path.display());
    }
    // BPE falls back to single bytes, so every byte must have a rank even when
    // the vocabulary also holds longer merges. A file missing one would panic
    // on ordinary text; refuse it here instead.
    if let Some(missing) = (0u16..=255).find(|byte| {
        let token = [*byte as u8];
        !ranks.contains_key(token.as_slice())
    }) {
        anyhow::bail!(
            "{}: the tiktoken vocabulary has no rank for single byte 0x{missing:02X}",
            path.display()
        );
    }
    let pattern = tiktoken_pattern(path, encoding);
    let vocab_size = ranks.len();
    let tokenizer =
        tiktoken_rs::CoreBPE::new(ranks.into_iter().collect(), Default::default(), pattern)
            .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
    Ok((tokenizer, vocab_size))
}

fn tiktoken_encoding(path: &Path, encoding: Option<TiktokenEncoding>) -> TiktokenEncoding {
    if let Some(encoding) = encoding {
        return encoding;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.contains("o200k") {
        TiktokenEncoding::O200k
    } else if name.contains("p50k") {
        TiktokenEncoding::P50k
    } else if name.contains("r50k") || name.contains("gpt2") {
        TiktokenEncoding::R50k
    } else {
        TiktokenEncoding::Cl100k
    }
}

fn tiktoken_pattern(path: &Path, encoding: Option<TiktokenEncoding>) -> &'static str {
    tiktoken_encoding(path, encoding).pattern()
}

/// One body item, rendered as its chunk text and carrying what a chunk needs
/// to cite it.
#[derive(Debug, Clone)]
struct Piece {
    text: String,
    item: DocItem,
    /// The heading path the item sits under, outermost first.
    headings: Vec<String>,
    /// The caption text a picture states.
    captions: Vec<String>,
    /// Whether the item is a list item, whose run with its neighbours is one
    /// chunk: a list split between chunks is no longer a list.
    list_item: bool,
    /// Whether the item opens a section. A section heading starts a new hybrid
    /// chunk, so the passage under it is packed with its own heading rather
    /// than with the section before it.
    heading: bool,
    /// Whether the text may be split into sentences by the hybrid chunker. A
    /// table's grid and a code block's lines are structure, not prose.
    splittable: bool,
}

/// The walk that turns the body tree into pieces, keeping the heading path.
struct Collector<'a> {
    doc: &'a DoclingDocument,
    pieces: Vec<Piece>,
    /// The open headings: their depth and their text.
    headings: Vec<(i32, String)>,
    /// One saved heading path per open group, restored when the group closes,
    /// so a heading inside a slide does not become the document's.
    saved: Vec<Vec<(i32, String)>>,
}

impl<'a> Collector<'a> {
    fn new(doc: &'a DoclingDocument) -> Self {
        Self {
            doc,
            pieces: Vec::new(),
            headings: Vec::new(),
            saved: Vec::new(),
        }
    }

    /// The heading path as a chunk states it.
    fn path(&self) -> Vec<String> {
        self.headings.iter().map(|(_, text)| text.clone()).collect()
    }

    /// Open a heading at `depth`, dropping the headings it closes.
    fn push_heading(&mut self, depth: i32, text: &str) {
        while self.headings.last().is_some_and(|(open, _)| *open >= depth) {
            self.headings.pop();
        }
        self.headings.push((depth, text.to_string()));
    }

    fn piece(
        &mut self,
        item: &BaseFields,
        text: String,
        list_item: bool,
        splittable: bool,
        heading: bool,
    ) {
        self.pieces.push(Piece {
            text,
            item: doc_item(item),
            headings: self.path(),
            captions: Vec::new(),
            list_item,
            heading,
            splittable,
        });
    }

    /// The text of the text items a list of references names, in their order.
    fn referenced_texts(&self, refs: &[pdfextractor_core::Ref]) -> Vec<String> {
        refs.iter()
            .filter_map(|r| {
                let index = r.ref_path.strip_prefix("#/texts/")?.parse::<usize>().ok()?;
                let text = self.doc.texts.get(index)?.text.trim();
                (!text.is_empty()).then(|| text.to_string())
            })
            .collect()
    }
}

/// The fields every item kind shares, so one function makes a [`DocItem`].
struct BaseFields<'a> {
    self_ref: &'a str,
    parent: Option<&'a pdfextractor_core::Ref>,
    children: &'a [pdfextractor_core::Ref],
    content_layer: &'a str,
    label: &'a str,
    prov: &'a [Provenance],
}

fn doc_item(fields: &BaseFields<'_>) -> DocItem {
    DocItem {
        self_ref: fields.self_ref.to_string(),
        parent: fields.parent.map(|parent| Cref {
            cref: parent.ref_path.clone(),
        }),
        children: fields
            .children
            .iter()
            .map(|child| Cref {
                cref: child.ref_path.clone(),
            })
            .collect(),
        content_layer: fields.content_layer.to_string(),
        // Docling calls a paragraph a `text`; every other label is already its
        // own, so this is the only mapping.
        label: if fields.label == "paragraph" {
            "text".to_string()
        } else {
            fields.label.to_string()
        },
        prov: fields.prov.to_vec(),
    }
}

impl Visitor for Collector<'_> {
    fn text(&mut self, _index: usize, item: &TextItem) {
        let base = BaseFields {
            self_ref: &item.self_ref,
            parent: item.parent.as_ref(),
            children: &item.children,
            content_layer: &item.content_layer,
            label: &item.label,
            prov: &item.prov,
        };
        let text = item.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        match item.label.as_str() {
            "title" => {
                self.push_heading(1, &text);
                self.piece(&base, text, false, true, true);
            }
            "section_header" => {
                let depth = item.level.unwrap_or(1).clamp(1, 5) + 1;
                self.push_heading(depth, &text);
                self.piece(&base, text, false, true, true);
            }
            "list_item" => {
                let text = list_line(item, &text);
                self.piece(&base, text, true, true, false);
            }
            "formula" => {
                let text = format!("$${}$$", text);
                self.piece(&base, text, false, false, false);
            }
            "code" => {
                let text = crate::fence(&text);
                self.piece(&base, text, false, false, false);
            }
            _ => self.piece(&base, text, false, true, false),
        }
    }

    fn table(&mut self, _index: usize, table: &TableItem) {
        let Some(text) = crate::pipe_table_with(table, |_| String::new()) else {
            return;
        };
        let base = BaseFields {
            self_ref: &table.self_ref,
            parent: table.parent.as_ref(),
            children: &table.children,
            content_layer: &table.content_layer,
            label: &table.label,
            prov: &table.prov,
        };
        self.piece(&base, text, false, false, false);
    }

    fn picture(&mut self, _index: usize, picture: &PictureItem) {
        // A picture is retrieved by its caption: with no words there is no
        // chunk to make, and Docling makes none either.
        let captions = self.referenced_texts(&picture.captions);
        if captions.is_empty() {
            return;
        }
        let base = BaseFields {
            self_ref: &picture.self_ref,
            parent: picture.parent.as_ref(),
            children: &picture.children,
            content_layer: &picture.content_layer,
            label: &picture.label,
            prov: &picture.prov,
        };
        let text = captions.join("\n");
        self.piece(&base, text, false, false, false);
        if let Some(piece) = self.pieces.last_mut() {
            piece.captions = captions;
        }
    }

    fn group_start(&mut self, _group: &Group) {
        self.saved.push(self.headings.clone());
    }

    fn group_end(&mut self) {
        if let Some(headings) = self.saved.pop() {
            self.headings = headings;
        }
    }
}

/// A list item as its line: the document's own marker, indented by its nesting
/// depth the way Docling indents a nested list (four spaces per level), with a
/// bullet glyph written as Markdown's `-`.
fn list_line(item: &TextItem, text: &str) -> String {
    let depth = item.level.unwrap_or(1).clamp(1, 8) as usize;
    let indent = "    ".repeat(depth - 1);
    match item.marker.as_deref().map(str::trim) {
        Some(marker) if !marker.is_empty() && !crate::is_bullet_glyph(marker) => {
            format!("{indent}{marker} {text}")
        }
        _ => format!("{indent}- {text}"),
    }
}

/// The meta every chunk of `doc` starts from.
fn meta(
    doc: &DoclingDocument,
    items: Vec<DocItem>,
    headings: Vec<String>,
    captions: Vec<String>,
) -> DocMeta {
    DocMeta {
        schema_name: META_SCHEMA.to_string(),
        version: META_VERSION.to_string(),
        doc_items: items,
        headings: (!headings.is_empty()).then_some(headings),
        captions: (!captions.is_empty()).then_some(captions),
        origin: doc.origin.clone(),
    }
}

fn item_of(piece: &Piece) -> DocItem {
    piece.item.clone()
}

/// One chunk per item, with a run of list items as one chunk.
fn hierarchical(pieces: &[Piece], doc: &DoclingDocument) -> Vec<DocChunk> {
    let mut chunks: Vec<DocChunk> = Vec::new();
    // Whether the chunk being built is a list run, and so takes the next list
    // item instead of starting a chunk of its own.
    let mut open_list = false;
    for piece in pieces {
        if open_list && piece.list_item {
            if let Some(chunk) = chunks.last_mut() {
                chunk.text.push('\n');
                chunk.text.push_str(&piece.text);
                chunk.meta.doc_items.push(item_of(piece));
                continue;
            }
        }
        open_list = piece.list_item;
        chunks.push(DocChunk {
            text: piece.text.clone(),
            meta: meta(
                doc,
                vec![item_of(piece)],
                piece.headings.clone(),
                piece.captions.clone(),
            ),
        });
    }
    chunks
}

/// Sentences packed to the token budget, with the tail of one chunk repeated at
/// the head of the next.
fn hybrid(pieces: &[Piece], doc: &DoclingDocument, options: &ChunkOptions) -> Vec<DocChunk> {
    let budget = options.max_tokens.max(1);
    let overlap = options.overlap_tokens.min(budget.saturating_sub(1));
    let count = |text: &str| match options.tokenizer.as_deref() {
        Some(tokenizer) => tokenizer.count_tokens(text),
        None => count_tokens(text),
    };

    // Every sentence, with the piece it came from — a table or a code block is
    // one unit, because its lines are structure rather than sentences.
    struct Unit {
        text: String,
        piece: usize,
        heading: bool,
    }
    let mut units: Vec<Unit> = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let texts: Vec<String> = if piece.splittable {
            sentences(&piece.text)
        } else {
            vec![piece.text.clone()]
        };
        for text in texts {
            units.push(Unit {
                text,
                piece: index,
                heading: piece.heading,
            });
        }
    }

    let mut chunks: Vec<DocChunk> = Vec::new();
    let mut start = 0;
    while start < units.len() {
        let mut end = start;
        let mut tokens = 0;
        // The first unit is always taken, however large: a sentence is never
        // cut, and a chunk smaller than the budget with nothing in it is worse
        // than one sentence over it.
        while end < units.len() {
            // A section heading opens its own chunk: the passage under it is
            // packed with its own heading instead of tailing the section before.
            if end > start && units[end].heading {
                break;
            }
            let cost = count(&units[end].text);
            if end > start && tokens + cost > budget {
                break;
            }
            tokens += cost;
            end += 1;
            if tokens >= budget {
                break;
            }
        }

        let mut items: Vec<DocItem> = Vec::new();
        let mut text = String::new();
        let mut headings: Vec<String> = Vec::new();
        let mut captions: Vec<String> = Vec::new();
        for unit in &units[start..end] {
            let piece = &pieces[unit.piece];
            if items.last().map(|item| item.self_ref.as_str()) != Some(piece.item.self_ref.as_str())
            {
                items.push(item_of(piece));
            }
            if headings.is_empty() {
                headings = piece.headings.clone();
                captions = piece.captions.clone();
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&unit.text);
        }
        chunks.push(DocChunk {
            text: contextualize(&text, &headings),
            meta: meta(doc, items, headings, captions),
        });

        // The next chunk starts with the closing sentences whose cost fits the
        // overlap, and always makes progress: a chunk of one sentence repeats
        // nothing, or the walk would never end.
        if end >= units.len() {
            break;
        }
        let mut next = end;
        let mut carried = 0;
        while next > start + 1 {
            let cost = count(&units[next - 1].text);
            if carried + cost > overlap {
                break;
            }
            carried += cost;
            next -= 1;
        }
        start = next;
    }
    chunks
}

/// A chunk's text with its heading path in front, as Markdown headings — the
/// shape Docling's `contextualize` writes: `# title`, `## section`, then the
/// passage.
fn contextualize(text: &str, headings: &[String]) -> String {
    if headings.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    for (depth, heading) in headings.iter().enumerate() {
        if depth > 0 {
            out.push('\n');
        }
        out.push_str(&"#".repeat(depth + 1));
        out.push(' ');
        out.push_str(heading);
    }
    out.push_str("\n\n");
    out.push_str(text);
    out
}

/// A text split into sentences: a full stop, an exclamation mark or a question
/// mark that ends a word ends a sentence, and so does a line break. The rule is
/// deliberately blunt — an abbreviation such as `et al.` ends one too — because
/// a wrong split costs a chunk boundary while a missing split costs a chunk
/// three times the budget.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        let ends =
            matches!(c, '.' | '!' | '?') && chars.peek().is_none_or(|next| next.is_whitespace());
        if ends || c == '\n' {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            current.clear();
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfextractor_core::{DocumentBuilder, Ref, TextLabel};

    fn doc_with_sections() -> DoclingDocument {
        let mut b = DocumentBuilder::new("demo.pdf");
        b.add_text_item(
            "Annual Report",
            1,
            [0.0, 0.0, 10.0, 10.0],
            TextLabel {
                label: "title".to_string(),
                ..Default::default()
            },
        );
        b.add_text_item(
            "Revenue",
            1,
            [0.0, 0.0, 10.0, 10.0],
            TextLabel {
                label: "section_header".to_string(),
                level: Some(1),
                ..Default::default()
            },
        );
        b.add_text("Revenue grew by 9%.", 1);
        b.add_text("Every region contributed.", 1);
        b.add_text_item(
            "Outlook",
            1,
            [0.0, 0.0, 10.0, 10.0],
            TextLabel {
                label: "section_header".to_string(),
                level: Some(1),
                ..Default::default()
            },
        );
        b.add_text("The next quarter is uncertain.", 1);
        b.build()
    }

    #[test]
    fn the_hierarchical_chunker_makes_one_chunk_per_item_with_its_heading_path() {
        let doc = doc_with_sections();
        let chunks = chunk_document(&doc, ChunkOptions::default());
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "Annual Report",
                "Revenue",
                "Revenue grew by 9%.",
                "Every region contributed.",
                "Outlook",
                "The next quarter is uncertain.",
            ]
        );
        assert_eq!(
            chunks[2].meta.headings.as_deref(),
            Some(["Annual Report".to_string(), "Revenue".to_string()].as_slice())
        );
        // The path is the section's own, not the previous one's.
        assert_eq!(
            chunks[5].meta.headings.as_deref(),
            Some(["Annual Report".to_string(), "Outlook".to_string()].as_slice())
        );
    }

    #[test]
    fn every_chunk_names_the_item_it_came_from_with_its_page_and_box() {
        let doc = doc_with_sections();
        let chunks = chunk_document(&doc, ChunkOptions::default());
        let item = &chunks[2].meta.doc_items[0];
        assert_eq!(item.self_ref, "#/texts/2");
        assert_eq!(item.label, "text");
        assert_eq!(item.content_layer, "body");
        assert_eq!(item.prov.len(), 1);
        assert_eq!(item.prov[0].page_no, 1);
        assert_eq!(item.prov[0].bbox.l, 72.0);
        assert_eq!(chunks[2].meta.schema_name, META_SCHEMA);
        assert_eq!(chunks[2].meta.version, META_VERSION);
    }

    /// A run of list items is one chunk, and the line keeps the marker the
    /// document printed; a nested item is indented four spaces per level, as
    /// Docling indents it.
    #[test]
    fn a_list_run_is_one_chunk_with_its_markers_and_nesting() {
        let mut b = DocumentBuilder::new("demo.pdf");
        for (level, marker, text) in [
            (1, "-", "Revenue grew."),
            (1, "-", "North America led."),
            (2, "-", "Europe followed."),
        ] {
            b.add_text_item(
                text,
                1,
                [0.0; 4],
                TextLabel {
                    label: "list_item".to_string(),
                    level: Some(level),
                    marker: Some(marker.to_string()),
                    ..Default::default()
                },
            );
        }
        let mut doc = b.build();
        // The builder keeps a list item's marker, not its depth; a reader that
        // has one (an office list, a PDF bullet) states it on the item.
        doc.texts[2].level = Some(2);
        let chunks = chunk_document(&doc, ChunkOptions::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].text,
            "- Revenue grew.\n- North America led.\n    - Europe followed."
        );
        assert_eq!(chunks[0].meta.doc_items.len(), 3);
    }

    /// A paragraph between two list runs is not a list item, so the runs do not
    /// merge across it, and the paragraph is its own chunk.
    #[test]
    fn a_paragraph_breaks_a_list_run() {
        let mut b = DocumentBuilder::new("demo.pdf");
        for text in ["one", "two"] {
            b.add_text_item(
                text,
                1,
                [0.0; 4],
                TextLabel {
                    label: "list_item".to_string(),
                    level: Some(1),
                    marker: Some("-".into()),
                    ..Default::default()
                },
            );
            if text == "one" {
                b.add_text("A paragraph.", 1);
            }
        }
        let chunks = chunk_document(&b.build(), ChunkOptions::default());
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["- one", "A paragraph.", "- two"]);
    }

    #[test]
    fn a_table_is_one_chunk_of_its_pipe_grid() {
        let mut b = DocumentBuilder::new("demo.pdf");
        b.add_text("Units by region", 1);
        b.add_table_plain(&[
            vec!["Region".to_string(), "Units".to_string()],
            vec!["North".to_string(), "145".to_string()],
        ]);
        let doc = b.build();
        let chunks = chunk_document(&doc, ChunkOptions::default());
        let table = chunks
            .iter()
            .find(|c| c.meta.doc_items[0].label == "table")
            .expect("a table chunk");
        assert!(table.text.contains("| North"), "{}", table.text);
        assert!(table.text.contains("145"), "{}", table.text);
    }

    /// A picture is retrieved by its caption. With no caption there are no
    /// words to index, so it makes no chunk — Docling makes none either.
    #[test]
    fn a_picture_is_a_chunk_only_when_it_has_a_caption() {
        let mut b = DocumentBuilder::new("demo.pdf");
        b.add_picture_plain("picture", None);
        assert!(chunk_document(&b.build(), ChunkOptions::default()).is_empty());

        let mut b = DocumentBuilder::new("demo.pdf");
        b.add_text("A growth chart", 1);
        b.add_picture_plain("picture", None);
        let mut doc = b.build();
        doc.pictures[0].captions = vec![Ref {
            ref_path: "#/texts/0".to_string(),
        }];
        let chunks = chunk_document(&doc, ChunkOptions::default());
        assert_eq!(chunks.len(), 2);
        let picture = chunks
            .iter()
            .find(|c| c.meta.doc_items[0].label == "picture")
            .expect("a picture chunk");
        assert_eq!(picture.text, "A growth chart");
        assert_eq!(
            picture.meta.captions.as_deref(),
            Some(["A growth chart".to_string()].as_slice())
        );
    }

    #[test]
    fn a_formula_is_a_chunk_that_keeps_its_delimiters() {
        let mut b = DocumentBuilder::new("demo.pdf");
        b.add_text_item(
            "E = mc^2",
            1,
            [0.0; 4],
            TextLabel {
                label: "formula".to_string(),
                ..Default::default()
            },
        );
        let chunks = chunk_document(&b.build(), ChunkOptions::default());
        assert_eq!(chunks[0].text, "$$E = mc^2$$");
    }

    #[test]
    fn the_jsonl_is_one_whole_record_per_line() {
        let doc = doc_with_sections();
        let jsonl = to_jsonl(&doc, &ChunkOptions::default()).expect("jsonl");
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 6);
        for line in lines {
            let value: serde_json::Value = serde_json::from_str(line).expect("a whole record");
            assert!(value.get("text").is_some());
            assert!(value.get("meta").and_then(|m| m.get("doc_items")).is_some());
        }
    }

    #[test]
    fn the_token_counter_is_four_characters_a_word_and_whitespace_separates() {
        assert_eq!(count_tokens(""), 0);
        assert_eq!(count_tokens("a b c"), 3);
        assert_eq!(count_tokens("information"), 3);
        assert_eq!(count_tokens("one two"), 2);
        assert_eq!(count_tokens("  words   split  "), 4);
    }

    #[test]
    fn hybrid_chunks_respect_the_budget_and_never_cut_a_sentence() {
        let doc = doc_with_sections();
        let options = ChunkOptions {
            kind: ChunkKind::Hybrid,
            max_tokens: 8,
            overlap_tokens: 0,
            tokenizer: None,
            include_notes: false,
        };
        let chunks = chunk_document(&doc, options);
        let bodies: Vec<String> = chunks
            .iter()
            .map(|c| c.text.rsplit("\n\n").next().unwrap().to_string())
            .collect();
        // Every sentence survives whole, in reading order.
        for sentence in [
            "Revenue grew by 9%.",
            "Every region contributed.",
            "The next quarter is uncertain.",
        ] {
            assert!(
                bodies.iter().any(|body| body.contains(sentence)),
                "{sentence} was cut: {bodies:?}"
            );
        }
        assert!(chunks.len() > 1, "a small budget has to split");
        // No chunk exceeds the budget unless a single sentence does.
        for chunk in &chunks {
            let body = chunk.text.rsplit("\n\n").next().unwrap();
            let single = sentences(body).len() == 1;
            assert!(
                count_tokens(body) <= 8 || single,
                "chunk of {} tokens: {}",
                count_tokens(body),
                chunk.text
            );
        }
    }

    #[test]
    fn hybrid_writes_the_heading_path_into_the_chunk_text() {
        let doc = doc_with_sections();
        let chunks = chunk_document(
            &doc,
            ChunkOptions {
                kind: ChunkKind::Hybrid,
                max_tokens: 512,
                ..Default::default()
            },
        );
        let body = chunks
            .iter()
            .find(|c| c.text.contains("Revenue grew by 9%."))
            .expect("the sentence is in a chunk");
        assert!(
            body.text.starts_with("# Annual Report\n## Revenue\n\n"),
            "{}",
            body.text
        );
        assert_eq!(
            body.meta.headings.as_deref(),
            Some(["Annual Report".to_string(), "Revenue".to_string()].as_slice())
        );
    }

    #[test]
    fn hybrid_overlap_repeats_the_tail_of_one_chunk_in_the_next() {
        let mut b = DocumentBuilder::new("demo.pdf");
        b.add_text("One sentence here.", 1);
        b.add_text("Two sentence here.", 1);
        b.add_text("Three sentence here.", 1);
        let doc = b.build();
        let options = ChunkOptions {
            kind: ChunkKind::Hybrid,
            max_tokens: 4,
            overlap_tokens: 3,
            tokenizer: None,
            include_notes: false,
        };
        let chunks = chunk_document(&doc, options);
        assert!(chunks.len() >= 2);
        for pair in chunks.windows(2) {
            let previous = pair[0].text.rsplit("\n\n").next().unwrap();
            let next = pair[1].text.rsplit("\n\n").next().unwrap();
            let tail = previous.split(' ').next_back().unwrap_or_default();
            assert!(
                next.starts_with(tail) || next.contains(tail),
                "{next:?} does not carry {tail:?} from {previous:?}"
            );
        }
    }

    #[test]
    fn a_sentence_larger_than_the_whole_budget_is_its_own_chunk_untruncated() {
        let mut b = DocumentBuilder::new("demo.pdf");
        let long =
            "This single sentence is deliberately much longer than any budget chosen in this test.";
        b.add_text(long, 1);
        let chunks = chunk_document(
            &b.build(),
            ChunkOptions {
                kind: ChunkKind::Hybrid,
                max_tokens: 4,
                ..Default::default()
            },
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, long);
    }

    #[test]
    fn the_notes_layer_joins_only_when_asked_for() {
        let mut b = DocumentBuilder::new("deck.pptx");
        let slide = b.add_group("chapter", "slide-0", "body");
        b.add_text_item_in(
            "Q3 Review",
            1,
            [0.0; 4],
            TextLabel {
                label: "title".to_string(),
                ..Default::default()
            },
            &slide,
        );
        b.add_notes_item_in("Say the picture is the growth.", 1, [0.0; 4], &slide);
        let doc = b.build();

        let without = chunk_document(&doc, ChunkOptions::default());
        assert_eq!(without.len(), 1);
        assert_eq!(without[0].text, "Q3 Review");

        let with = chunk_document(
            &doc,
            ChunkOptions {
                include_notes: true,
                ..Default::default()
            },
        );
        assert_eq!(with.len(), 2);
        assert_eq!(with[1].text, "Say the picture is the growth.");
        assert_eq!(with[1].meta.doc_items[0].content_layer, "notes");
    }

    fn tokenizer_fixture(name: &str) -> Tokenizer {
        Tokenizer::from_file(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-data")
                .join(name),
        )
        .unwrap_or_else(|error| panic!("the {name} tokenizer loads: {error}"))
    }

    fn mini_tokenizer() -> Tokenizer {
        tokenizer_fixture("tokenizer-mini.json")
    }

    #[test]
    fn a_sentencepiece_model_counts_a_hybrid_budget() {
        let tokenizer = tokenizer_fixture("tokenizer-mini.model");
        assert_eq!(tokenizer.description(), "SentencePiece · 13 tokens");
        assert_eq!(tokenizer.count_tokens("abcdefgh"), 1);
        assert!(
            tokenizer.count_tokens("abcdefgh") < count_tokens("abcdefgh"),
            "the SentencePiece model must count this word differently from the approximation"
        );
    }

    #[test]
    fn a_tiktoken_vocabulary_counts_a_hybrid_budget() {
        let tokenizer = tokenizer_fixture("tokenizer-mini.tiktoken");
        assert_eq!(
            tokenizer.description(),
            "tiktoken · cl100k_base · 257 tokens"
        );
        assert_eq!(tokenizer.count_tokens("abcdefgh"), 1);
        assert!(
            tokenizer.count_tokens("abcdefgh") < count_tokens("abcdefgh"),
            "the tiktoken vocabulary must count this word differently from the approximation"
        );
    }

    /// The extension cannot answer for a `.bin` or extensionless copy, so only
    /// the bytes can name the format; an uppercase extension is normalized. Each
    /// renamed copy must also load and count like the file it came from, not
    /// merely be classified correctly.
    #[test]
    fn a_renamed_tokenizer_loads_and_counts_from_its_bytes() {
        let dir =
            std::env::temp_dir().join(format!("pdfe-tokenizer-format-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");

        for (fixture, expected) in [
            ("tokenizer-mini.json", TokenizerFormat::HuggingFace),
            ("tokenizer-mini.model", TokenizerFormat::SentencePiece),
            ("tokenizer-mini.tiktoken", TokenizerFormat::Tiktoken),
        ] {
            let source = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-data")
                .join(fixture);
            let stem = Path::new(fixture)
                .file_stem()
                .expect("a fixture stem")
                .to_string_lossy();
            let extension = Path::new(fixture)
                .extension()
                .expect("a fixture extension")
                .to_string_lossy();
            let original = tokenizer_fixture(fixture);
            for (how, name) in [
                ("an unfamiliar extension", format!("{fixture}.bin")),
                ("no extension", format!("{stem}-{extension}")),
                (
                    "an uppercase extension",
                    format!("{stem}.{}", extension.to_ascii_uppercase()),
                ),
            ] {
                let target = dir.join(name);
                std::fs::copy(&source, &target).unwrap_or_else(|error| {
                    panic!("copy {} to {}: {error}", source.display(), target.display())
                });
                assert_eq!(
                    detect_tokenizer_format(&target)
                        .unwrap_or_else(|error| panic!("detect {}: {error}", target.display())),
                    expected,
                    "{fixture} was not detected when renamed with {how}"
                );

                let renamed = Tokenizer::from_file(&target)
                    .unwrap_or_else(|error| panic!("load {}: {error}", target.display()));
                assert_eq!(
                    renamed.description(),
                    original.description(),
                    "{fixture} described a different vocabulary when renamed with {how}"
                );
                assert_eq!(
                    renamed.count_tokens("abcdefgh"),
                    original.count_tokens("abcdefgh"),
                    "{fixture} counted a different number of tokens when renamed with {how}"
                );
                let mixed = "Mixed CASE, punctuation — 42 tokens, and 'quotes'!";
                assert_eq!(
                    renamed.count_tokens(mixed),
                    original.count_tokens(mixed),
                    "{fixture} counted a longer mixed sentence differently when renamed with {how}"
                );
                assert!(
                    renamed.count_tokens(mixed) > renamed.count_tokens("abcdefgh"),
                    "{fixture} did not count the mixed sentence as more tokens than one word \
                     when renamed with {how}"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A non-tokenizer file renamed to an unknown extension is rejected by the
    /// loader with the formats it accepts, rather than being mistaken for one
    /// and failing later with an opaque parser message.
    #[test]
    fn an_unsupported_renamed_tokenizer_is_rejected_with_a_format_error() {
        let dir =
            std::env::temp_dir().join(format!("pdfe-tokenizer-unsupported-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("tokenizer.bin");
        std::fs::write(&path, b"this is not a tokenizer").expect("write the unsupported file");

        let error = Tokenizer::from_file(&path)
            .expect_err("an unsupported file must be rejected")
            .to_string();
        assert!(
            error.contains("not a tokenizer.json, SentencePiece .model or tiktoken vocabulary"),
            "the refusal does not name the accepted formats: {error}"
        );
        assert!(
            error.contains(&path.display().to_string()),
            "the refusal does not name the file: {error}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_explicit_encoding_family_overrides_the_filename() {
        assert_eq!(
            tiktoken_pattern(Path::new("vocab.tiktoken"), None),
            CL100K_PATTERN
        );
        assert_eq!(
            tiktoken_pattern(Path::new("o200k-vocab.tiktoken"), None),
            O200K_PATTERN
        );
        assert_eq!(
            tiktoken_pattern(
                Path::new("o200k-vocab.tiktoken"),
                Some(TiktokenEncoding::P50k)
            ),
            P50K_PATTERN,
            "an explicit family must beat a hint in the filename"
        );
        assert_eq!(
            tiktoken_pattern(Path::new("vocab.tiktoken"), Some(TiktokenEncoding::O200k)),
            O200K_PATTERN
        );

        for (name, encoding) in [
            ("cl100k_base", TiktokenEncoding::Cl100k),
            ("cl100k", TiktokenEncoding::Cl100k),
            ("o200k_base", TiktokenEncoding::O200k),
            ("p50k_base", TiktokenEncoding::P50k),
            ("r50k_base", TiktokenEncoding::R50k),
        ] {
            assert_eq!(name.parse::<TiktokenEncoding>(), Ok(encoding), "{name}");
            assert_eq!(encoding.to_string(), encoding.name());
        }
        assert!("not-a-family".parse::<TiktokenEncoding>().is_err());
    }

    #[test]
    fn a_tiktoken_vocabulary_loads_under_every_encoding_family() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/tokenizer-mini.tiktoken");
        for encoding in [
            TiktokenEncoding::Cl100k,
            TiktokenEncoding::O200k,
            TiktokenEncoding::P50k,
            TiktokenEncoding::R50k,
        ] {
            let tokenizer = Tokenizer::from_file_with_tiktoken_encoding(&path, Some(encoding))
                .unwrap_or_else(|error| panic!("{encoding} loads: {error}"));
            assert_eq!(
                tokenizer.description(),
                format!("tiktoken · {encoding} · 257 tokens")
            );
            assert_eq!(tokenizer.count_tokens("abcdefgh"), 1);
        }
    }

    #[test]
    fn an_encoding_family_is_refused_for_another_tokenizer_format() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/tokenizer-mini.json");
        let error =
            Tokenizer::from_file_with_tiktoken_encoding(&path, Some(TiktokenEncoding::Cl100k))
                .expect_err("an encoding family must not apply to a Hugging Face file")
                .to_string();
        assert!(error.contains(".tiktoken"), "{error}");
    }

    #[test]
    fn a_real_tokenizer_counts_a_hybrid_budget_in_its_own_tokens() {
        let tokenizer = Arc::new(mini_tokenizer());
        assert_eq!(tokenizer.description(), "Hugging Face · 10 tokens");
        assert_eq!(count_tokens("abcdefgh"), 2);
        assert!(
            tokenizer.count_tokens("abcdefgh") > count_tokens("abcdefgh"),
            "the real model must count this word differently from the approximation"
        );

        let mut b = DocumentBuilder::new("demo.pdf");
        for _ in 0..3 {
            b.add_text("abcdefgh abcdefgh.", 1);
        }
        let doc = b.build();
        let approximate = chunk_document(
            &doc,
            ChunkOptions {
                kind: ChunkKind::Hybrid,
                max_tokens: 12,
                ..Default::default()
            },
        );
        let real = chunk_document(
            &doc,
            ChunkOptions {
                kind: ChunkKind::Hybrid,
                max_tokens: 12,
                tokenizer: Some(Arc::clone(&tokenizer)),
                ..Default::default()
            },
        );
        assert!(
            real.len() > approximate.len(),
            "the real tokenizer made {} chunk(s), the approximation {}",
            real.len(),
            approximate.len()
        );
        for chunk in &real {
            let body = chunk.text.rsplit("\n\n").next().unwrap();
            assert!(
                tokenizer.count_tokens(body) <= 12 || sentences(body).len() == 1,
                "chunk of {} real tokens: {}",
                tokenizer.count_tokens(body),
                chunk.text
            );
        }
    }

    #[test]
    fn an_export_lands_where_it_was_asked_for_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("pdfe-chunks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("demo.jsonl");
        export_to_file(&doc_with_sections(), &path, &ChunkOptions::default()).expect("export");
        let text = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(text.lines().count(), 6);
        let names: Vec<String> = std::fs::read_dir(&dir)
            .expect("list")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["demo.jsonl".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
