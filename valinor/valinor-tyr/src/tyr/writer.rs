//! A streaming JSON writer that formats values as mainline's writers do.
//!
//! Mainline builds most responses with rapidjson's `Writer`
//! (through `rapidjson::writer_wrapper_t`),
//! and some (the OSRM serializer, `/locate`) as `baldr::json` maps.
//! Both are covered here:
//! numbers go through the formatters in [`super::numbers`],
//! and strings can be escaped either way (see [`JsonWriter::string`] and
//! [`JsonWriter::baldr_string`]).
//! Keys are written in the order the caller writes them;
//! `baldr::json` maps are hash maps whose order clients cannot rely on,
//! so the serializers built on them write keys in source order instead.

use super::numbers::{
    UNLIMITED_DECIMAL_PLACES, write_fixed, write_general, write_rapidjson_double,
};
use core::fmt::Write;

/// A streaming JSON writer.
#[derive(Debug)]
pub(crate) struct JsonWriter {
    out: String,
    /// For each open object or array, whether it has no members yet.
    first: Vec<bool>,
    /// Whether the last thing written was a key, so the next value needs no comma.
    after_key: bool,
    /// rapidjson's `maxDecimalPlaces` for [`JsonWriter::double`].
    max_decimal_places: u32,
}

impl JsonWriter {
    /// A writer with rapidjson's defaults (no limit on decimal places).
    pub(crate) fn new() -> Self {
        Self::with_capacity(0)
    }

    /// A writer that reserves `capacity` bytes up front.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            out: String::with_capacity(capacity),
            first: Vec::new(),
            after_key: false,
            max_decimal_places: UNLIMITED_DECIMAL_PLACES,
        }
    }

    /// The JSON written so far.
    pub(crate) fn finish(self) -> String {
        self.out
    }

    /// `writer_wrapper_t::set_precision`: the decimal places later doubles are truncated to.
    pub(crate) fn set_precision(&mut self, max_decimal_places: u32) {
        self.max_decimal_places = max_decimal_places;
    }

    fn prefix(&mut self) {
        if self.after_key {
            self.after_key = false;
            return;
        }
        if let Some(first) = self.first.last_mut() {
            if *first {
                *first = false;
            } else {
                self.out.push(',');
            }
        }
    }

    /// Starts an object.
    pub(crate) fn start_object(&mut self) -> &mut Self {
        self.prefix();
        self.out.push('{');
        self.first.push(true);
        self
    }

    /// Ends the innermost object.
    pub(crate) fn end_object(&mut self) -> &mut Self {
        self.first.pop();
        self.out.push('}');
        self
    }

    /// Starts an array.
    pub(crate) fn start_array(&mut self) -> &mut Self {
        self.prefix();
        self.out.push('[');
        self.first.push(true);
        self
    }

    /// Ends the innermost array.
    pub(crate) fn end_array(&mut self) -> &mut Self {
        self.first.pop();
        self.out.push(']');
        self
    }

    /// Writes an object key; the next call writes its value.
    pub(crate) fn key(&mut self, key: &str) -> &mut Self {
        self.prefix();
        escape_rapidjson(&mut self.out, key);
        self.out.push(':');
        self.after_key = true;
        self
    }

    /// Writes a string as rapidjson does: only `"`, `\`, and control characters are escaped.
    pub(crate) fn string(&mut self, value: &str) -> &mut Self {
        self.prefix();
        escape_rapidjson(&mut self.out, value);
        self
    }

    /// Writes a string as `baldr::json` does, which also escapes `/` and DEL.
    pub(crate) fn baldr_string(&mut self, value: &str) -> &mut Self {
        self.prefix();
        escape_baldr(&mut self.out, value);
        self
    }

    /// Writes an unsigned integer.
    pub(crate) fn u64(&mut self, value: u64) -> &mut Self {
        self.prefix();
        let _ = write!(self.out, "{value}");
        self
    }

    /// Writes a signed integer.
    pub(crate) fn i64(&mut self, value: i64) -> &mut Self {
        self.prefix();
        let _ = write!(self.out, "{value}");
        self
    }

    /// Writes a boolean.
    pub(crate) fn bool(&mut self, value: bool) -> &mut Self {
        self.prefix();
        self.out.push_str(if value { "true" } else { "false" });
        self
    }

    /// Writes `null`.
    pub(crate) fn null(&mut self) -> &mut Self {
        self.prefix();
        self.out.push_str("null");
        self
    }

    /// Writes a double as rapidjson does, truncated to the current precision
    /// (see [`super::numbers::write_rapidjson_double`]).
    ///
    /// Pass floats as `f64::from(value)`: mainline promotes them before formatting.
    pub(crate) fn double(&mut self, value: f64) -> &mut Self {
        self.prefix();
        write_rapidjson_double(&mut self.out, value, self.max_decimal_places);
        self
    }

    /// Writes a `baldr::json::fixed_t` (see [`super::numbers::write_fixed`]).
    pub(crate) fn fixed(&mut self, value: f64, precision: usize) -> &mut Self {
        self.prefix();
        write_fixed(&mut self.out, value, precision);
        self
    }

    /// Writes a `baldr::json::float_t` (see [`super::numbers::write_general`]).
    pub(crate) fn general(&mut self, value: f64) -> &mut Self {
        self.prefix();
        write_general(&mut self.out, value);
        self
    }

    /// Writes already-serialized JSON as a value (`baldr::json::RawJSON`).
    pub(crate) fn raw(&mut self, json: &str) -> &mut Self {
        self.prefix();
        self.out.push_str(json);
        self
    }
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// rapidjson's `Writer::WriteString` for UTF-8 output without validation.
fn escape_rapidjson(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => push_unicode_escape(out, c),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `baldr::json`'s string visitor: like rapidjson, but also `\/`,
/// and every byte `iscntrl` accepts in the C locale (which adds DEL).
fn escape_baldr(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '/' => out.push_str("\\/"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 || c == '\u{7f}' => push_unicode_escape(out, c),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn push_unicode_escape(out: &mut String, c: char) {
    let code = u32::from(c);
    out.push_str("\\u00");
    out.push(char::from(HEX[((code >> 4) & 0xF) as usize]));
    out.push(char::from(HEX[(code & 0xF) as usize]));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting_and_commas() {
        let mut w = JsonWriter::new();
        w.start_object();
        w.key("a").u64(1);
        w.key("b")
            .start_array()
            .i64(-1)
            .bool(true)
            .null()
            .end_array();
        w.key("c").start_object().end_object();
        w.key("d")
            .start_array()
            .start_object()
            .key("x")
            .string("y")
            .end_object();
        w.start_object().end_object().end_array();
        w.end_object();
        assert_eq!(
            w.finish(),
            r#"{"a":1,"b":[-1,true,null],"c":{},"d":[{"x":"y"},{}]}"#
        );
    }

    #[test]
    fn precision_applies_to_later_doubles() {
        let mut w = JsonWriter::new();
        w.start_array().double(1.234_567);
        w.set_precision(3);
        w.double(1.234_567).fixed(1.234_567, 2).general(1.234_567);
        w.end_array();
        assert_eq!(w.finish(), "[1.234567,1.234,1.23,1.23457]");
    }

    #[test]
    fn escaping() {
        let mut w = JsonWriter::new();
        w.start_array()
            .string("a\"b\\c/d\u{1}\n\u{7f}é")
            .baldr_string("a\"b\\c/d\u{1}\n\u{7f}é")
            .end_array();
        assert_eq!(
            w.finish(),
            "[\"a\\\"b\\\\c/d\\u0001\\n\u{7f}é\",\"a\\\"b\\\\c\\/d\\u0001\\n\\u007Fé\"]"
        );
    }
}
