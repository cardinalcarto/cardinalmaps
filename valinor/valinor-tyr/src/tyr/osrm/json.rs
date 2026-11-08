//! A small `baldr::json` look-alike: the value tree mainline's OSRM serializer builds.
//!
//! The serializer adds to steps after creating them
//! (banner and voice instructions go on the *previous* step),
//! and `baldr::json` maps are `std::unordered_map`s whose `emplace` keeps an existing key.
//! [`Map::emplace`] does the same, so a second value for a key is dropped as in mainline.
//! Key order is not something clients can rely on (mainline's is a hash order),
//! so maps keep insertion order.

use crate::tyr::writer::JsonWriter;

/// A JSON value, as `baldr::json::Value`.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Json {
    /// A string, escaped as `baldr::json` escapes it.
    Str(String),
    /// An unsigned integer.
    U64(u64),
    /// A fixed-precision number (`baldr::json::fixed_t`).
    Fixed(f64, usize),
    /// A boolean.
    Bool(bool),
    /// `null`.
    Null,
    /// An object.
    Map(Map),
    /// An array.
    Array(Vec<Json>),
    /// Already-serialized JSON (`baldr::json::RawJSON`).
    Raw(String),
}

impl Json {
    /// A string value.
    pub(super) fn str(value: impl Into<String>) -> Self {
        Self::Str(value.into())
    }

    /// Writes this value.
    pub(super) fn write(&self, writer: &mut JsonWriter) {
        match self {
            Self::Str(value) => {
                writer.baldr_string(value);
            }
            Self::U64(value) => {
                writer.u64(*value);
            }
            Self::Fixed(value, precision) => {
                writer.fixed(*value, *precision);
            }
            Self::Bool(value) => {
                writer.bool(*value);
            }
            Self::Null => {
                writer.null();
            }
            Self::Map(map) => map.write(writer),
            Self::Array(values) => {
                writer.start_array();
                for value in values {
                    value.write(writer);
                }
                writer.end_array();
            }
            Self::Raw(json) => {
                writer.raw(json);
            }
        }
    }
}

impl From<Map> for Json {
    fn from(map: Map) -> Self {
        Self::Map(map)
    }
}

impl From<Vec<Json>> for Json {
    fn from(values: Vec<Json>) -> Self {
        Self::Array(values)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<u64> for Json {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<u32> for Json {
    fn from(value: u32) -> Self {
        Self::U64(u64::from(value))
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Self::Str(value.to_string())
    }
}

/// A JSON object whose [`Map::emplace`] keeps the first value for a key.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Map(Vec<(String, Json)>);

impl Map {
    /// An empty object.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// `std::unordered_map::emplace`: adds the key unless it is already there.
    pub(super) fn emplace(&mut self, key: impl Into<String>, value: impl Into<Json>) {
        let key = key.into();
        if !self.contains(&key) {
            self.0.push((key, value.into()));
        }
    }

    /// Whether the object has the key.
    pub(super) fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    /// Whether the object has no keys.
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Writes this object.
    pub(super) fn write(&self, writer: &mut JsonWriter) {
        writer.start_object();
        for (key, value) in &self.0 {
            writer.key(key);
            value.write(writer);
        }
        writer.end_object();
    }

    /// Serializes this object.
    pub(super) fn to_json(&self) -> String {
        let mut writer = JsonWriter::new();
        self.write(&mut writer);
        writer.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emplace_keeps_the_first_value() {
        let mut map = Map::new();
        map.emplace("a", 1_u64);
        map.emplace("a", 2_u64);
        map.emplace("b", Json::Fixed(1.5, 3));
        map.emplace(
            "c",
            vec![Json::Null, Json::str("x/y"), Json::Raw("{}".into())],
        );
        assert_eq!(map.to_json(), r#"{"a":1,"b":1.500,"c":[null,"x\/y",{}]}"#);
        assert!(map.contains("b") && !map.is_empty());
    }
}
