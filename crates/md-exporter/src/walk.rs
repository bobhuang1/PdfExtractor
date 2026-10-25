//! The tree walk every non-JSON writer shares.
//!
//! One traversal decides what a rendered document *is*: which references
//! exist, what reading order means through nested groups, and that only
//! `body` items are content — a running head or a page number repeated on
//! every page is furniture, and it stays out of a Markdown file, an HTML
//! file and a text file just the same. A deck's speaker notes and comments
//! are on the notes layer and are left out of those three by default for the
//! same reason: they are the document talking about itself, not what it says.
//! [`walk_body_with`] includes them when a reader asks for a deck's remarks
//! (the CLI's `--include-notes`), still in the order the body tree gives them —
//! and each writer marks a notes-layer item as a remark (Markdown a labelled
//! blockquote, HTML an `<aside>`), because a reader must be able to tell the
//! slide from the deck talking about it.
//! (The YAML writer writes the whole model, so it always lists them — after
//! the walked items, because the walk is what its order is.) Each format then
//! implements [`Visitor`] and receives the same items, in the same order,
//! without re-deriving any of those rules.
//!
//! [`Order`] is a visitor that records nothing but the sequence, for the YAML
//! writer: the whole document model is written there, and the walk decides
//! where its `texts`, `tables` and `pictures` are listed.

use pdfextractor_core::{DoclingDocument, Group, PictureItem, TableItem, TextItem};

/// A writer: a destination for the body tree's items, visited in reading order.
///
/// `index` is the item's position in the document's array of its kind — the
/// number its `self_ref` names. Renderers that only need the item ignore it.
///
/// [`Visitor::group_start`] and [`Visitor::group_end`] bracket a group's
/// children. Only a format that can say “these items belong together” needs
/// them — Markdown has no groups and marks their boundaries, HTML writes a
/// `<section>`, plain text does nothing — so they default to no-ops and a
/// writer that only renders items is unchanged.
pub trait Visitor {
    fn text(&mut self, index: usize, item: &TextItem);
    fn table(&mut self, index: usize, table: &TableItem);
    fn picture(&mut self, index: usize, picture: &PictureItem);
    /// A group's children follow, in the group's own order.
    fn group_start(&mut self, _group: &Group) {}
    /// The group's children have all been visited.
    fn group_end(&mut self) {}
}

/// Walk the body tree in reading order, handing every content item to
/// `visitor`. This is a reading of what the document shows: the notes layer
/// stays out.
pub fn walk_body(doc: &DoclingDocument, visitor: &mut impl Visitor) {
    walk_body_with(doc, false, visitor);
}

/// The same walk, with a deck's own remarks included when `notes` is true.
///
/// A notes-layer item is reached where the body tree puts it — a slide's
/// speaker notes inside the slide's group, a comment in its own
/// `comment_section` group — so the document still reads in one order. Only
/// the notes layer joins; page furniture and invisible content stay out.
pub fn walk_body_with(doc: &DoclingDocument, notes: bool, visitor: &mut impl Visitor) {
    for child in &doc.body.children {
        walk_ref(doc, &child.ref_path, notes, visitor);
    }
}

/// Render whatever `ref_path` points at, in body order: a `body`-layer item
/// (or a notes-layer one, when `notes` asks) reaches the visitor, furniture is
/// skipped, a group's children follow in their own order, and a reference that
/// does not resolve is ignored rather than failing a render (the JSON writer
/// carries the same tolerance).
fn walk_ref(doc: &DoclingDocument, ref_path: &str, notes: bool, visitor: &mut impl Visitor) {
    let Some(rest) = ref_path.strip_prefix("#/") else {
        return;
    };
    let Some((kind, index)) = rest.split_once('/') else {
        return;
    };
    let Ok(index) = index.parse::<usize>() else {
        return;
    };
    match kind {
        "texts" => {
            if let Some(item) = doc.texts.get(index) {
                if is_read(&item.content_layer, notes) {
                    visitor.text(index, item);
                }
            }
        }
        "tables" => {
            if let Some(item) = doc.tables.get(index) {
                if is_read(&item.content_layer, notes) {
                    visitor.table(index, item);
                }
            }
        }
        "pictures" => {
            if let Some(item) = doc.pictures.get(index) {
                if is_read(&item.content_layer, notes) {
                    visitor.picture(index, item);
                }
            }
        }
        "groups" => {
            if let Some(group) = doc.groups.get(index) {
                visitor.group_start(group);
                for child in &group.children {
                    walk_ref(doc, &child.ref_path, notes, visitor);
                }
                visitor.group_end();
            }
        }
        _ => {}
    }
}

/// Whether an item on `layer` is part of this reading: body content always,
/// the notes layer when a reader asked for a deck's remarks.
fn is_read(layer: &str, notes: bool) -> bool {
    layer == "body" || (notes && layer == "notes")
}

/// The body tree's visitation order: which items of each kind the walk
/// reaches, in the order it reaches them.
#[derive(Default, Debug)]
pub struct Order {
    pub texts: Vec<usize>,
    pub tables: Vec<usize>,
    pub pictures: Vec<usize>,
}

impl Visitor for Order {
    fn text(&mut self, index: usize, _item: &TextItem) {
        if !self.texts.contains(&index) {
            self.texts.push(index);
        }
    }
    fn table(&mut self, index: usize, _table: &TableItem) {
        if !self.tables.contains(&index) {
            self.tables.push(index);
        }
    }
    fn picture(&mut self, index: usize, _picture: &PictureItem) {
        if !self.pictures.contains(&index) {
            self.pictures.push(index);
        }
    }
}

impl Order {
    /// The order `doc`'s body tree is walked in.
    pub fn of(doc: &DoclingDocument) -> Order {
        let mut order = Order::default();
        walk_body(doc, &mut order);
        order
    }

    /// The full model with `texts`, `tables` and `pictures` listed in the
    /// body's reading order.
    ///
    /// This is a *reordering*, never a removal: every item the arrays held
    /// still holds — the walk's items first, in walk order, then the rest
    /// (page furniture the body never reaches) in the order they were — and
    /// because the model addresses items by their position (`#/texts/3`, and
    /// `body.children` pointing there), every `self_ref` and every child `$ref`
    /// is rewritten to where its item now sits. The document that comes out is
    /// a legal `DoclingDocument` whose YAML reads top to bottom in the order a
    /// reader meets the content.
    pub fn reorder(&self, doc: &DoclingDocument) -> DoclingDocument {
        let mut out = doc.clone();
        let texts = rearrange(&mut out.texts, &self.texts);
        let tables = rearrange(&mut out.tables, &self.tables);
        let pictures = rearrange(&mut out.pictures, &self.pictures);

        // The item at each position now *is* what its `self_ref` should say.
        for (index, item) in out.texts.iter_mut().enumerate() {
            item.self_ref = format!("#/texts/{index}");
        }
        for (index, item) in out.tables.iter_mut().enumerate() {
            item.self_ref = format!("#/tables/{index}");
        }
        for (index, item) in out.pictures.iter_mut().enumerate() {
            item.self_ref = format!("#/pictures/{index}");
        }

        // Every pointer into those arrays, wherever it is held: the root
        // groups' children, each group's children, and the children (and,
        // for safety, the parents) of every other item — a key-value item
        // spans the texts a caption names, for instance. A pointer that names
        // a group or a page is returned unchanged.
        let remap = |path: &str| remap_path(path, &texts, &tables, &pictures);
        let refs = |list: &mut Vec<pdfextractor_core::Ref>| {
            for r in list.iter_mut() {
                r.ref_path = remap(&r.ref_path);
            }
        };
        refs(&mut out.body.children);
        if let Some(furniture) = &mut out.furniture {
            refs(&mut furniture.children);
        }
        for group in &mut out.groups {
            refs(&mut group.children);
        }
        for kv in &mut out.key_value_items {
            refs(&mut kv.children);
            if let Some(parent) = &mut kv.parent {
                parent.ref_path = remap(&parent.ref_path);
            }
        }
        // The three arrays hold three types, so each takes its own pass.
        for item in &mut out.texts {
            refs(&mut item.children);
            if let Some(parent) = &mut item.parent {
                parent.ref_path = remap(&parent.ref_path);
            }
        }
        for item in &mut out.tables {
            refs(&mut item.children);
            if let Some(parent) = &mut item.parent {
                parent.ref_path = remap(&parent.ref_path);
            }
            // A rich cell references the group its pictures sit in. Pictures
            // move with the walk, so the cell's pointer has to move with them
            // or it would name the item that took the old slot.
            for cell in &mut item.data.table_cells {
                if let Some(reference) = &mut cell.reference {
                    reference.ref_path = remap(&reference.ref_path);
                }
            }
        }
        for item in &mut out.pictures {
            refs(&mut item.children);
            if let Some(parent) = &mut item.parent {
                parent.ref_path = remap(&parent.ref_path);
            }
        }
        out
    }
}

/// Move the walked positions to the front of `items`, keeping the rest in
/// their old relative order, and answer the map from each item's old position
/// to its new one.
fn rearrange<T>(items: &mut Vec<T>, order: &[usize]) -> Vec<usize> {
    let mut slots: Vec<Option<T>> = std::mem::take(items).into_iter().map(Some).collect();
    let mut map = (0..slots.len()).collect::<Vec<usize>>();
    let mut taken = vec![false; slots.len()];
    let mut out: Vec<T> = Vec::with_capacity(slots.len());

    // The walked items first, in walk order — a reference the walk met twice
    // answers once, and a reference beyond the array is ignored like any
    // unresolvable one.
    for &old in order {
        if old < slots.len() && !taken[old] {
            taken[old] = true;
            map[old] = out.len();
            if let Some(item) = slots[old].take() {
                out.push(item);
            }
        }
    }
    // Then everything the walk never reached — page furniture — in the order
    // it was.
    for old in 0..slots.len() {
        if !taken[old] {
            map[old] = out.len();
            if let Some(item) = slots[old].take() {
                out.push(item);
            }
        }
    }
    *items = out;
    map
}

/// Point `path` at an item's new position when it names one that moved.
fn remap_path(path: &str, texts: &[usize], tables: &[usize], pictures: &[usize]) -> String {
    let Some(rest) = path.strip_prefix("#/") else {
        return path.to_string();
    };
    let Some((kind, index)) = rest.split_once('/') else {
        return path.to_string();
    };
    let Ok(old) = index.parse::<usize>() else {
        return path.to_string();
    };
    let map = match kind {
        "texts" => texts,
        "tables" => tables,
        "pictures" => pictures,
        _ => return path.to_string(),
    };
    match map.get(old) {
        Some(new) => format!("#/{kind}/{new}"),
        None => path.to_string(),
    }
}
