//! A JSON document that behaves like the rapidjson `Document` mainline parses requests into.
//!
//! Mainline reads every request parameter through rapidjson and its own helpers
//! (`rapidjson::get`, `get_optional`, and `get_child_optional` in `baldr/rapidjson_utils.h`).
//! Those helpers decide which values are accepted and how they are converted,
//! so matching mainline's `options` exactly means matching them, not just JSON:
//!
//! - **Duplicate keys** are kept, and lookups find the *first* one
//!   (rapidjson's `FindMember`), where most JSON libraries keep the last.
//! - **Numbers** keep rapidjson's classification:
//!   an integer literal is stored as an integer (with `IsInt`, `IsUint`, `IsInt64`,
//!   and `IsUint64` derived from its value),
//!   and anything with a fraction or exponent, or too large for 64 bits, is a double.
//! - **Typed reads** ([`JsonValue::get`]) try the exact type first,
//!   then `boost::lexical_cast` from a string,
//!   then a C++ `static_cast` from a boolean or any number,
//!   as `get_optional` does for arithmetic types.
//!   Non-arithmetic types (strings, arrays, objects) are only accepted when they match exactly.
//!
//! # Deviations
//!
//! - rapidjson does not validate UTF-8 by default, so it accepts strings with invalid UTF-8.
//!   We replace invalid sequences with U+FFFD.
//! - rapidjson parses decimal numbers with its "normal precision" algorithm,
//!   which can be one unit in the last place away from the correctly rounded value
//!   for literals with more than about 15 significant digits.
//!   We always round correctly.
//! - rapidjson's parser is recursive without a depth limit.
//!   We reject documents nested more than [`MAX_DEPTH`] levels deep
//!   (as a parse failure) rather than risk exhausting the stack.
//! - Converting an out-of-range double to an integer is undefined behavior in C++.
//!   We reproduce what mainline's x86-64 builds do
//!   (see [`double_to_i32`] and [`double_to_u32`]).

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

/// The deepest nesting of arrays and objects we parse.
pub const MAX_DEPTH: usize = 512;

/// A JSON number, classified as rapidjson classifies it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JsonNumber {
    /// An integer literal without a minus sign that fits in 64 bits (rapidjson's `Uint`/`Uint64`).
    Unsigned(u64),
    /// An integer literal with a minus sign whose magnitude fits in 63 bits
    /// (rapidjson's `Int`/`Int64`).
    ///
    /// rapidjson stores `-0` as the integer 0, which we represent as `Unsigned(0)`,
    /// so this is always negative.
    Negative(i64),
    /// Any other number (rapidjson's `Double`).
    Double(f64),
}

impl JsonNumber {
    /// rapidjson's `GetDouble`, which converts integers to doubles.
    #[expect(
        clippy::cast_precision_loss,
        reason = "rapidjson converts 64-bit integers to doubles the same way"
    )]
    pub(crate) fn as_f64(self) -> f64 {
        match self {
            JsonNumber::Unsigned(u) => u as f64,
            JsonNumber::Negative(i) => i as f64,
            JsonNumber::Double(d) => d,
        }
    }
}

/// A JSON value, as rapidjson stores it.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number.
    Number(JsonNumber),
    /// A string.
    String(String),
    /// An array.
    Array(Vec<JsonValue>),
    /// An object, with its members in document order (duplicates included).
    Object(Vec<(String, JsonValue)>),
}

/// Why a document failed to parse.
///
/// Mainline reports every parse failure the same way (error 100),
/// so this only describes where it went wrong, for logs and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("parse JSON: invalid input at byte {offset}")]
pub struct JsonParseError {
    /// The byte offset of the failure.
    pub offset: usize,
}

impl JsonValue {
    /// An empty object, which is what mainline uses when a request has no JSON at all.
    pub const fn empty_object() -> JsonValue {
        JsonValue::Object(Vec::new())
    }

    /// Parses a document as rapidjson's `Document::Parse` does with default flags.
    ///
    /// Like `Parse(const char*)`, the input ends at the first NUL byte.
    ///
    /// # Errors
    ///
    /// Returns an error for anything rapidjson rejects:
    /// an empty document, trailing content after the root value,
    /// unescaped control characters, invalid escapes or surrogates,
    /// numbers too large for a double, and so on.
    pub fn parse(input: &[u8]) -> Result<JsonValue, JsonParseError> {
        let end = input.iter().position(|&b| b == 0).unwrap_or(input.len());
        let mut parser = Parser {
            input: &input[..end],
            pos: 0,
        };
        parser.skip_whitespace();
        let value = parser.value(0)?;
        parser.skip_whitespace();
        if parser.pos != parser.input.len() {
            return Err(parser.error());
        }
        Ok(value)
    }

    /// Whether this is an object (rapidjson's `IsObject`).
    pub fn is_object(&self) -> bool {
        matches!(self, JsonValue::Object(_))
    }

    /// The first member with this name, if this is an object (rapidjson's `FindMember`).
    pub fn member(&self, name: &str) -> Option<&JsonValue> {
        match self {
            JsonValue::Object(members) => members
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// Resolves a JSON pointer as rapidjson's `Pointer::Get` does.
    ///
    /// The pointer must be a valid, plain (not URI fragment) pointer such as `/costing_options/auto`;
    /// all of ours are literals or built from validated names and indexes.
    /// Each token looks up the first member of that name in an object,
    /// or an element in an array if the token is a valid index.
    pub fn pointer(&self, pointer: &str) -> Option<&JsonValue> {
        let mut value = self;
        for token in tokens(pointer) {
            value = match value {
                JsonValue::Object(members) => members
                    .iter()
                    .find(|(key, _)| *key == token)
                    .map(|(_, value)| value)?,
                JsonValue::Array(elements) => elements.get(array_index(&token)?)?,
                _ => return None,
            };
        }
        Some(value)
    }

    /// Sets the value at a pointer, creating it as rapidjson's `SetValueByPointer` does.
    ///
    /// Missing members are appended, and a value that is in the way is replaced:
    /// with an object for a name token, or (unless it is already an object) an array for an index.
    pub fn set_pointer(&mut self, pointer: &str, new_value: JsonValue) {
        let mut value = self;
        for token in tokens(pointer) {
            let index = array_index(&token);
            match index {
                None if !value.is_object() => *value = JsonValue::Object(Vec::new()),
                Some(_) if !matches!(value, JsonValue::Array(_) | JsonValue::Object(_)) => {
                    *value = JsonValue::Array(Vec::new());
                }
                _ => {}
            }
            value = match value {
                JsonValue::Array(elements) => {
                    // An index token always reaches here; `array_index` is `Some`.
                    let index = index.unwrap_or(0);
                    if index >= elements.len() {
                        elements.resize(index + 1, JsonValue::Null);
                    }
                    &mut elements[index]
                }
                JsonValue::Object(members) => {
                    let position = members
                        .iter()
                        .position(|(key, _)| *key == token)
                        .unwrap_or_else(|| {
                            members.push((token.into_owned(), JsonValue::Null));
                            members.len() - 1
                        });
                    &mut members[position].1
                }
                // We replaced anything else with an object or array above.
                _ => return,
            };
        }
        *value = new_value;
    }

    /// Appends a member, as rapidjson's `AddMember` does (even if the name already exists).
    ///
    /// Does nothing unless this is an object;
    /// callers check [`JsonValue::is_object`] first where mainline would fail an assertion.
    pub fn add_member(&mut self, name: String, value: JsonValue) {
        if let JsonValue::Object(members) = self {
            members.push((name, value));
        }
    }

    /// Removes the first member with this name, as rapidjson's `RemoveMember` does.
    pub fn remove_member(&mut self, name: &str) {
        if let JsonValue::Object(members) = self
            && let Some(position) = members.iter().position(|(key, _)| key == name)
        {
            members.remove(position);
        }
    }

    /// Reads the value at a pointer as `T`, as mainline's `rapidjson::get_optional<T>` does.
    pub fn get<T: FromJson>(&self, pointer: &str) -> Option<T> {
        self.pointer(pointer).and_then(T::from_json)
    }

    /// Reads the value at a pointer as `T`, or the default (mainline's `rapidjson::get<T>`).
    pub fn get_or<T: FromJson>(&self, pointer: &str, default: T) -> T {
        self.get(pointer).unwrap_or(default)
    }

    /// The array at a pointer (`get_optional<rapidjson::Value::ConstArray>`).
    pub fn get_array(&self, pointer: &str) -> Option<&[JsonValue]> {
        match self.pointer(pointer)? {
            JsonValue::Array(elements) => Some(elements),
            _ => None,
        }
    }
}

/// Splits a pointer into its tokens, unescaping `~1` and `~0`.
fn tokens(pointer: &str) -> impl Iterator<Item = Cow<'_, str>> {
    pointer.split('/').skip(1).map(|token| {
        if token.contains('~') {
            Cow::Owned(token.replace("~1", "/").replace("~0", "~"))
        } else {
            Cow::Borrowed(token)
        }
    })
}

/// The array index a pointer token denotes, if any:
/// all digits, without a leading zero unless it is `0`, and in range.
fn array_index(token: &str) -> Option<usize> {
    if token.is_empty()
        || !token.bytes().all(|b| b.is_ascii_digit())
        || (token.len() > 1 && token.starts_with('0'))
    {
        return None;
    }
    // rapidjson's index is a 32-bit `SizeType`.
    token
        .parse::<u32>()
        .ok()
        .and_then(|i| usize::try_from(i).ok())
}

/// A type that can be read from a JSON value as mainline's `rapidjson::get_optional<T>` reads it.
pub trait FromJson: Sized {
    /// Converts the value, or returns `None` where mainline's helper would.
    fn from_json(value: &JsonValue) -> Option<Self>;
}

impl FromJson for String {
    fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::String(s) => Some(s.clone()),
            _ => None,
        }
    }
}

impl FromJson for bool {
    fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::Bool(b) => Some(*b),
            JsonValue::String(s) => lexical_bool(s),
            JsonValue::Number(JsonNumber::Unsigned(u)) => Some(*u != 0),
            JsonValue::Number(JsonNumber::Negative(i)) => Some(*i != 0),
            JsonValue::Number(JsonNumber::Double(d)) => Some(*d != 0.0),
            _ => None,
        }
    }
}

impl FromJson for f64 {
    fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::Number(n) => Some(n.as_f64()),
            JsonValue::String(s) => lexical_float::<f64>(s),
            JsonValue::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }
}

impl FromJson for f32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "this is the C++ static_cast mainline applies"
    )]
    fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::Number(JsonNumber::Unsigned(u)) => Some(*u as f32),
            JsonValue::Number(JsonNumber::Negative(i)) => Some(*i as f32),
            JsonValue::Number(JsonNumber::Double(d)) => Some(*d as f32),
            JsonValue::String(s) => lexical_float::<f32>(s),
            JsonValue::Bool(b) => Some(f32::from(u8::from(*b))),
            _ => None,
        }
    }
}

impl FromJson for i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "this is the C++ static_cast mainline applies"
    )]
    fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::Number(JsonNumber::Unsigned(u)) => Some(*u as i32),
            JsonValue::Number(JsonNumber::Negative(i)) => Some(*i as i32),
            JsonValue::Number(JsonNumber::Double(d)) => Some(double_to_i32(*d)),
            JsonValue::String(s) => lexical_i32(s),
            JsonValue::Bool(b) => Some(i32::from(*b)),
            _ => None,
        }
    }
}

impl FromJson for u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "this is the C++ static_cast mainline applies"
    )]
    fn from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::Number(JsonNumber::Unsigned(u)) => Some(*u as u32),
            JsonValue::Number(JsonNumber::Negative(i)) => Some(*i as u32),
            JsonValue::Number(JsonNumber::Double(d)) => Some(double_to_u32(*d)),
            JsonValue::String(s) => lexical_u32(s),
            JsonValue::Bool(b) => Some(u32::from(*b)),
            _ => None,
        }
    }
}

/// `static_cast<int>(double)` as mainline's x86-64 builds compute it.
///
/// The conversion is undefined in C++ when the value is out of range;
/// x86-64's `cvttsd2si` then returns `INT_MIN`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is checked to be in range first"
)]
pub fn double_to_i32(d: f64) -> i32 {
    if d > f64::from(i32::MIN) - 1.0 && d < f64::from(i32::MAX) + 1.0 {
        d as i32
    } else {
        i32::MIN
    }
}

/// `static_cast<unsigned>(double)` as mainline's x86-64 builds compute it.
///
/// GCC converts through a 64-bit signed integer (`cvttsd2si` with a 64-bit destination)
/// and keeps the low 32 bits, so negative values wrap around
/// and values out of the 64-bit range become 0.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "this reproduces the truncation the C++ conversion performs"
)]
pub fn double_to_u32(d: f64) -> u32 {
    // 2^63, the first value outside the signed 64-bit range.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if d > -LIMIT && d < LIMIT {
        (d as i64) as u32
    } else {
        0
    }
}

/// `boost::lexical_cast<bool>`: an optional sign, any number of zeros, then `0` or `1`.
fn lexical_bool(s: &str) -> Option<bool> {
    let bytes = s.as_bytes();
    let (&last, _) = bytes.split_last()?;
    let rest = match bytes.first() {
        Some(b'+' | b'-') => &bytes[1..],
        _ => bytes,
    };
    // A lone sign has nothing after it (boost reads past the end; we reject it).
    let (_, leading) = rest.split_last()?;
    if !leading.iter().all(|&b| b == b'0') || !matches!(last, b'0' | b'1') {
        return None;
    }
    Some(last == b'1')
}

/// The sign and digits of an integer literal as `boost::lexical_cast` accepts it.
fn lexical_digits(s: &str) -> Option<(bool, &str)> {
    let (negative, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some((negative, digits))
}

/// `boost::lexical_cast<int>`.
fn lexical_i32(s: &str) -> Option<i32> {
    let (negative, digits) = lexical_digits(s)?;
    let magnitude: u64 = digits.parse().ok()?;
    if negative {
        0i64.checked_sub_unsigned(magnitude)
            .and_then(|v| i32::try_from(v).ok())
    } else {
        i32::try_from(magnitude).ok()
    }
}

/// `boost::lexical_cast<unsigned>`, which accepts a minus sign and wraps around.
fn lexical_u32(s: &str) -> Option<u32> {
    let (negative, digits) = lexical_digits(s)?;
    let magnitude = digits.parse::<u32>().ok()?;
    Some(if negative {
        0u32.wrapping_sub(magnitude)
    } else {
        magnitude
    })
}

/// A floating point type `boost::lexical_cast` can produce.
trait LexicalFloat: core::str::FromStr + Copy {
    fn is_infinite(self) -> bool;
}

impl LexicalFloat for f64 {
    fn is_infinite(self) -> bool {
        f64::is_infinite(self)
    }
}

impl LexicalFloat for f32 {
    fn is_infinite(self) -> bool {
        f32::is_infinite(self)
    }
}

/// `boost::lexical_cast` to a floating point type.
///
/// Boost accepts `inf`, `infinity`, and `nan` (in any case, optionally signed),
/// and otherwise a decimal literal read by the C++ stream library
/// that must use the whole string, not end in an exponent marker or sign,
/// and not overflow.
fn lexical_float<T: LexicalFloat>(s: &str) -> Option<T> {
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    let special = ["inf", "infinity", "nan"]
        .iter()
        .any(|name| unsigned.eq_ignore_ascii_case(name));
    if special {
        return s.parse().ok();
    }
    let valid = !unsigned.is_empty()
        && unsigned
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        && !s.ends_with(['e', 'E', '+', '-']);
    if !valid {
        return None;
    }
    let value: T = s.parse().ok()?;
    (!value.is_infinite()).then_some(value)
}

/// A recursive descent parser following rapidjson's grammar with default flags.
struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn error(&self) -> JsonParseError {
        JsonParseError { offset: self.pos }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.pos += 1;
        }
    }

    fn consume(&mut self, literal: &[u8]) -> Result<(), JsonParseError> {
        if self.input[self.pos..].starts_with(literal) {
            self.pos += literal.len();
            Ok(())
        } else {
            Err(self.error())
        }
    }

    fn value(&mut self, depth: usize) -> Result<JsonValue, JsonParseError> {
        match self.peek() {
            Some(b'n') => self.consume(b"null").map(|()| JsonValue::Null),
            Some(b't') => self.consume(b"true").map(|()| JsonValue::Bool(true)),
            Some(b'f') => self.consume(b"false").map(|()| JsonValue::Bool(false)),
            Some(b'"') => self.string().map(JsonValue::String),
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            _ => self.number().map(JsonValue::Number),
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonValue, JsonParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error());
        }
        self.pos += 1; // '{'
        let mut members = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(members));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.error());
            }
            let key = self.string()?;
            self.skip_whitespace();
            self.consume(b":")?;
            self.skip_whitespace();
            let value = self.value(depth)?;
            members.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(members));
                }
                _ => return Err(self.error()),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<JsonValue, JsonParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error());
        }
        self.pos += 1; // '['
        let mut elements = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(elements));
        }
        loop {
            elements.push(self.value(depth)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(elements));
                }
                _ => return Err(self.error()),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonParseError> {
        let digits = self
            .input
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.error())?;
        let mut value = 0;
        for &digit in digits {
            let nibble = char::from(digit).to_digit(16).ok_or_else(|| self.error())?;
            value = value * 16 + nibble;
        }
        self.pos += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, JsonParseError> {
        self.pos += 1; // '"'
        let mut bytes = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.error());
            };
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(String::from_utf8(bytes).unwrap_or_else(|invalid| {
                        String::from_utf8_lossy(invalid.as_bytes()).into_owned()
                    }));
                }
                b'\\' => {
                    let escape_offset = self.pos;
                    self.pos += 1;
                    let e = self.peek().ok_or_else(|| self.error())?;
                    self.pos += 1;
                    let unescaped = match e {
                        b'"' => b'"',
                        b'\\' => b'\\',
                        b'/' => b'/',
                        b'b' => 0x08,
                        b'f' => 0x0C,
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'u' => {
                            let mut codepoint = self.hex4()?;
                            if (0xD800..=0xDFFF).contains(&codepoint) {
                                let invalid = JsonParseError {
                                    offset: escape_offset,
                                };
                                if codepoint > 0xDBFF {
                                    return Err(invalid);
                                }
                                if self.consume(b"\\u").is_err() {
                                    return Err(invalid);
                                }
                                let low = self.hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(invalid);
                                }
                                codepoint =
                                    (((codepoint - 0xD800) << 10) | (low - 0xDC00)) + 0x10000;
                            }
                            let c = char::from_u32(codepoint).ok_or(JsonParseError {
                                offset: escape_offset,
                            })?;
                            let mut buffer = [0; 4];
                            bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                            continue;
                        }
                        _ => {
                            return Err(JsonParseError {
                                offset: escape_offset,
                            });
                        }
                    };
                    bytes.push(unescaped);
                }
                0..0x20 => return Err(self.error()),
                _ => {
                    bytes.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Result<JsonNumber, JsonParseError> {
        let start = self.pos;
        let minus = self.peek() == Some(b'-');
        if minus {
            self.pos += 1;
        }
        let int_start = self.pos;
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.error()),
        }
        let int_end = self.pos;
        let mut is_double = false;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return Err(self.error());
            }
            is_double = true;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return Err(self.error());
            }
            is_double = true;
        }
        // The literal is ASCII by construction.
        let literal =
            core::str::from_utf8(&self.input[start..self.pos]).map_err(|_| self.error())?;
        if !is_double {
            let digits =
                core::str::from_utf8(&self.input[int_start..int_end]).map_err(|_| self.error())?;
            if let Ok(magnitude) = digits.parse::<u64>() {
                if !minus {
                    return Ok(JsonNumber::Unsigned(magnitude));
                }
                if magnitude == 0 {
                    return Ok(JsonNumber::Unsigned(0));
                }
                if let Some(negative) = 0i64.checked_sub_unsigned(magnitude) {
                    return Ok(JsonNumber::Negative(negative));
                }
            }
            // Too large for a 64-bit integer: rapidjson falls back to a double.
        }
        let value: f64 = literal
            .parse()
            .map_err(|_| JsonParseError { offset: start })?;
        if value.is_infinite() {
            return Err(JsonParseError { offset: start });
        }
        Ok(JsonNumber::Double(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> JsonValue {
        JsonValue::parse(s.as_bytes()).expect("valid JSON")
    }

    #[test]
    fn rejects_what_rapidjson_rejects() {
        for bad in [
            "",
            " ",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            ".5",
            "+1",
            "1e",
            "--1",
            "\"\t\"",
            "\"\\x\"",
            "\"\\ud800\"",
            "\"\\udc00\"",
            "\"\\ud800\\u0041\"",
            "1 2",
            "nul",
            "NaN",
            "Infinity",
            "1e400",
            "{a:1}",
            "'a'",
        ] {
            assert!(JsonValue::parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn accepts_any_root_and_stops_at_nul() {
        assert_eq!(parse(" 5 "), JsonValue::Number(JsonNumber::Unsigned(5)));
        assert_eq!(parse("\"x\""), JsonValue::String("x".into()));
        assert_eq!(
            JsonValue::parse(b"{}\0garbage").expect("stops at NUL"),
            JsonValue::empty_object()
        );
    }

    #[test]
    fn classifies_numbers_like_rapidjson() {
        assert_eq!(parse("-0"), JsonValue::Number(JsonNumber::Unsigned(0)));
        assert_eq!(parse("-5"), JsonValue::Number(JsonNumber::Negative(-5)));
        assert_eq!(parse("1.0"), JsonValue::Number(JsonNumber::Double(1.0)));
        assert_eq!(parse("1e2"), JsonValue::Number(JsonNumber::Double(100.0)));
        assert_eq!(
            parse("18446744073709551615"),
            JsonValue::Number(JsonNumber::Unsigned(u64::MAX))
        );
        assert_eq!(
            parse("18446744073709551616"),
            JsonValue::Number(JsonNumber::Double(18_446_744_073_709_551_616.0))
        );
        assert_eq!(
            parse("-9223372036854775808"),
            JsonValue::Number(JsonNumber::Negative(i64::MIN))
        );
        assert!(matches!(
            parse("-9223372036854775809"),
            JsonValue::Number(JsonNumber::Double(_))
        ));
    }

    #[test]
    fn strings_unescape() {
        assert_eq!(
            parse(r#""a\"\\\/\b\f\n\r\t\u00e9\ud83d\ude00""#),
            JsonValue::String("a\"\\/\u{8}\u{c}\n\r\té😀".into())
        );
        // Invalid UTF-8 is accepted (and replaced).
        assert_eq!(
            JsonValue::parse(b"\"\xff\"").expect("accepted"),
            JsonValue::String("\u{fffd}".into())
        );
    }

    #[test]
    fn first_duplicate_wins() {
        let doc = parse(r#"{"a": 1, "a": 2}"#);
        assert_eq!(doc.get::<u32>("/a"), Some(1));
    }

    #[test]
    fn pointers() {
        let doc = parse(r#"{"a": [{"b": 1}, 2], "0": {"1": 3}, "x/y": 4, "": 5}"#);
        assert_eq!(doc.get::<u32>("/a/0/b"), Some(1));
        assert_eq!(doc.get::<u32>("/a/1"), Some(2));
        assert_eq!(doc.get::<u32>("/a/01"), None);
        assert_eq!(doc.get::<u32>("/a/2"), None);
        assert_eq!(doc.get::<u32>("/0/1"), Some(3));
        assert_eq!(doc.get::<u32>("/x~1y"), Some(4));
        assert_eq!(doc.get::<u32>("/"), Some(5));
        assert_eq!(doc.pointer(""), Some(&doc));
        assert_eq!(doc.get::<u32>("/a/b"), None);
    }

    #[test]
    fn set_pointer_creates_and_replaces() {
        let mut doc = parse(r#"{"a": 5}"#);
        doc.set_pointer("/a/b", JsonValue::Bool(true));
        doc.set_pointer("/c/d", JsonValue::Null);
        assert_eq!(doc, parse(r#"{"a": {"b": true}, "c": {"d": null}}"#));
        let mut doc = parse(r#"{"a": {"b": 1, "b": 2}}"#);
        doc.set_pointer("/a/b", JsonValue::Bool(false));
        assert_eq!(doc, parse(r#"{"a": {"b": false, "b": 2}}"#));
    }

    #[test]
    fn typed_reads_follow_get_optional() {
        let doc = parse(
            r#"{"t": true, "one": 1, "half": 0.5, "s1": "1", "strue": "true", "neg": -1,
                "big": 3000000000, "sf": "1.5", "sbad": "1.5x", "arr": [], "huge": 1e20,
                "sneg": "-1", "ten_billion": 1e10, "splus": "+0001", "sinf": "-INF", "s_over": "1e999"}"#,
        );
        assert_eq!(doc.get::<bool>("/t"), Some(true));
        assert_eq!(doc.get::<bool>("/one"), Some(true));
        assert_eq!(doc.get::<bool>("/half"), Some(true));
        assert_eq!(doc.get::<bool>("/s1"), Some(true));
        assert_eq!(doc.get::<bool>("/strue"), None);
        assert_eq!(doc.get::<bool>("/sneg"), Some(true));
        assert_eq!(doc.get::<bool>("/splus"), Some(true));
        assert_eq!(doc.get::<bool>("/arr"), None);
        assert_eq!(doc.get::<f64>("/t"), Some(1.0));
        assert_eq!(doc.get::<f64>("/sf"), Some(1.5));
        assert_eq!(doc.get::<f64>("/sbad"), None);
        assert_eq!(doc.get::<f64>("/sinf"), Some(f64::NEG_INFINITY));
        assert_eq!(doc.get::<f64>("/s_over"), None);
        assert_eq!(doc.get::<f32>("/half"), Some(0.5));
        assert_eq!(doc.get::<i32>("/half"), Some(0));
        assert_eq!(doc.get::<i32>("/big"), Some(-1_294_967_296));
        assert_eq!(doc.get::<i32>("/huge"), Some(i32::MIN));
        assert_eq!(doc.get::<u32>("/neg"), Some(u32::MAX));
        assert_eq!(doc.get::<u32>("/sneg"), Some(u32::MAX));
        // Beyond the signed 64-bit range, x86-64 gives 0; within it, the low 32 bits.
        assert_eq!(doc.get::<u32>("/huge"), Some(0));
        assert_eq!(doc.get::<u32>("/ten_billion"), Some(1_410_065_408));
        assert_eq!(doc.get::<u32>("/s1"), Some(1));
        assert_eq!(doc.get::<String>("/one"), None);
        assert_eq!(doc.get::<String>("/s1"), Some("1".into()));
        assert_eq!(doc.get_array("/arr").map(<[JsonValue]>::len), Some(0));
    }

    #[test]
    fn lexical_casts() {
        assert_eq!(lexical_i32("2147483647"), Some(i32::MAX));
        assert_eq!(lexical_i32("-2147483648"), Some(i32::MIN));
        assert_eq!(lexical_i32("2147483648"), None);
        assert_eq!(lexical_i32(" 1"), None);
        assert_eq!(lexical_i32(""), None);
        assert_eq!(lexical_i32("-"), None);
        assert_eq!(lexical_u32("4294967295"), Some(u32::MAX));
        assert_eq!(lexical_u32("4294967296"), None);
        assert_eq!(lexical_bool("0"), Some(false));
        assert_eq!(lexical_bool("00"), Some(false));
        assert_eq!(lexical_bool("2"), None);
        assert_eq!(lexical_bool("-"), None);
        assert_eq!(lexical_bool(""), None);
        assert_eq!(lexical_float::<f64>("1e5"), Some(100_000.0));
        assert_eq!(lexical_float::<f64>("1e+"), None);
        assert_eq!(lexical_float::<f64>("nan").map(f64::is_nan), Some(true));
        assert_eq!(lexical_float::<f32>("1e39"), None);
        assert_eq!(lexical_float::<f64>("0x10"), None);
    }

    #[test]
    fn double_conversions_match_x86_64() {
        assert_eq!(double_to_i32(-1.9), -1);
        assert_eq!(double_to_i32(2_147_483_647.9), i32::MAX);
        assert_eq!(double_to_i32(2_147_483_648.0), i32::MIN);
        assert_eq!(double_to_u32(-1.5), u32::MAX);
        assert_eq!(double_to_u32(4_294_967_296.0), 0);
        assert_eq!(double_to_u32(1e30), 0);
    }
}
