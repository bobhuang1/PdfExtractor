use pdfextractor_pdf_parser::lexer::{Lexer, Token};
use std::fs;

#[test]
fn lexes_header_objects_and_stream_keywords() {
    let data = fs::read("../../test-data/hello.pdf").expect("read test-data/hello.pdf");
    let mut lexer = Lexer::new(&data);
    let tokens = lexer.tokenize();

    assert!(matches!(tokens.first(), Some(Token::Header(h)) if h == "%PDF-1.4"));
    assert!(tokens.contains(&Token::Obj), "expected obj keywords");
    assert!(tokens.contains(&Token::Stream), "expected stream keyword");
    assert!(
        tokens.contains(&Token::EndStream),
        "expected endstream keyword"
    );
    assert!(matches!(tokens.last(), Some(Token::Eof)));
}

#[test]
fn literal_strings_decode_escapes() {
    let mut lexer = Lexer::new(b"(a\\)b (c))");
    let tokens = lexer.tokenize();
    assert_eq!(tokens[0], Token::String("a)b (c)".to_string()));
}

#[test]
fn indirect_references_lex_as_number_number_r() {
    let mut lexer = Lexer::new(b"2 0 R");
    let tokens = lexer.tokenize();
    // No %%EOF in this input, so no Eof token is produced.
    assert_eq!(tokens, vec![Token::Integer(2), Token::Integer(0), Token::R]);
}

#[test]
fn eof_marker_is_lexed() {
    let mut lexer = Lexer::new(b"1 0 obj\nendobj\n%%EOF");
    let tokens = lexer.tokenize();
    assert_eq!(tokens.last(), Some(&Token::Eof));
}
