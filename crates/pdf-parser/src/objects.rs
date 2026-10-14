//! PDF object model (PDF 1.7, clause 7.3).

use std::collections::HashMap;

pub type ObjId = (usize, i32);

#[derive(Debug, Clone, PartialEq)]
pub enum PdfValue {
    Ref(ObjId),
    Dictionary(HashMap<String, PdfValue>),
    Array(Vec<PdfValue>),
    String(String),
    Name(String),
    Number(f64),
    Integer(i64),
    Boolean(bool),
    Null,
    /// A stream: its dictionary plus the raw (still encoded) bytes.
    Stream(HashMap<String, PdfValue>, Vec<u8>),
}

impl PdfValue {
    pub fn as_dict(&self) -> Option<&HashMap<String, PdfValue>> {
        match self {
            PdfValue::Dictionary(d) => Some(d),
            PdfValue::Stream(d, _) => Some(d),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<PdfValue>> {
        match self {
            PdfValue::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            PdfValue::Number(n) => Some(*n),
            PdfValue::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&str> {
        match self {
            PdfValue::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_ref(&self) -> Option<ObjId> {
        match self {
            PdfValue::Ref(id) => Some(*id),
            _ => None,
        }
    }

    /// Raw stream bytes, if this value is a stream.
    pub fn stream_bytes(&self) -> Option<&[u8]> {
        match self {
            PdfValue::Stream(_, bytes) => Some(bytes),
            _ => None,
        }
    }

    /// Look up a key in a dictionary or stream dictionary.
    pub fn get(&self, key: &str) -> Option<&PdfValue> {
        self.as_dict().and_then(|d| d.get(key))
    }
}
