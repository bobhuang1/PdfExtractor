//! The shared resolver every XML reader appends character data through.
//!
//! quick-xml reports a document's character data and the references written
//! inside it as separate events — `Event::Text` for the run of characters and
//! `Event::GeneralRef` for each `&…;` — so a reader that wants the text the
//! document says has to append both. That puts this function on the path of
//! every docx, xlsx, pptx, odp/ods/odt and chart reading, and the branch it
//! takes decides whether a document reads as the author wrote it or as markup.
//! Each branch is held here: the five entities XML predefines, a numeric
//! character reference (the one form that is not a name at all), and a name no
//! standard declares, which is kept in its spelling because no DTD is read.

use pdfextractor_limits::push_reference;
use quick_xml::events::BytesRef;

/// Resolve one reference exactly as a reader appends it: the name the event
/// carries, which is the reference without the `&` and `;` around it.
fn resolved(reference: &str) -> String {
    let mut text = String::new();
    push_reference(&mut text, &BytesRef::new(reference));
    text
}

#[test]
fn the_five_predefined_entities_resolve_to_their_characters() {
    for (reference, character) in [
        ("lt", '<'),
        ("gt", '>'),
        ("amp", '&'),
        ("quot", '"'),
        ("apos", '\''),
    ] {
        assert_eq!(
            resolved(reference),
            character.to_string(),
            "&{reference}; did not resolve to {character:?}"
        );
    }
}

#[test]
fn numeric_character_references_resolve_in_both_bases() {
    assert_eq!(resolved("#65"), "A", "&#65; is the decimal form");
    assert_eq!(resolved("#x41"), "A", "&#x41; is the hexadecimal form");
    // Above the basic plane, so the resolver cannot be reading a `char` from
    // the digits alone: this is a scalar value, not a UTF-16 pair.
    assert_eq!(resolved("#x1F600"), "\u{1F600}");
}

#[test]
fn a_reference_no_standard_declares_keeps_its_spelling() {
    // `&nbsp;` and friends are declared per document in a DTD, and this
    // workspace refuses a DOCTYPE outright, so there is nothing to resolve
    // against. Keeping the spelling is what an unread declaration can honestly
    // become; dropping it would delete text and inventing a space would add
    // some.
    assert_eq!(resolved("nbsp"), "&nbsp;");
    assert_eq!(resolved("custom"), "&custom;");
}
