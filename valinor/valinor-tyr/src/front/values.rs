//! Small value checks the front needs: narrative locales, dates, and polylines.

use alloc::vec::Vec;

/// The narrative locales mainline 3.9.1 ships (`locales/*.json`), with their aliases.
///
/// A request's `language` is kept only if it is one of these (`odin::get_locales()`);
/// otherwise the default `en-US` is used.
pub const LOCALES: &[&str] = &[
    "ar",
    "ar-SA",
    "au",
    "bg",
    "bg-BG",
    "ca",
    "ca-ES",
    "cs",
    "cs-CZ",
    "da",
    "da-DK",
    "de",
    "de-DE",
    "el",
    "el-GR",
    "en",
    "en-AU",
    "en-GB",
    "en-US",
    "en-US-x-pirate",
    "en-x-pirate",
    "es",
    "es-ES",
    "et",
    "et-EE",
    "fi",
    "fi-FI",
    "fr",
    "fr-FR",
    "he",
    "he-IL",
    "hi",
    "hi-IN",
    "hu",
    "hu-HU",
    "it",
    "it-IT",
    "ja",
    "ja-JP",
    "ko",
    "ko-KR",
    "mn",
    "mn-MN",
    "nb",
    "nb-NO",
    "nl",
    "nl-NL",
    "pl",
    "pl-PL",
    "pt",
    "pt-BR",
    "pt-PT",
    "ro",
    "ro-RO",
    "ru",
    "ru-RU",
    "sk",
    "sk-SK",
    "sl",
    "sl-SI",
    "sv",
    "sv-SE",
    "tr",
    "tr-TR",
    "uk",
    "uk-UA",
    "vi",
    "vi-VN",
];

/// Whether a date and time is valid, as mainline's `baldr::DateTime::is_iso_valid` checks it.
///
/// Mainline requires exactly 16 characters with `-`, `-`, `T`, and `:` at the positions of
/// `YYYY-MM-DDTHH:MM`, then reads the string with `std::get_time` and that format,
/// and accepts it if `tm_year` came out positive (a year after 1900).
/// We simulate the libstdc++ (GCC 13) `std::get_time` that mainline's image uses:
/// each field takes one digit up to its width (stopping at a non-digit) and must be in range
/// (month 1–12, day 1–31 in any month, hour 0–23, minute 0–59),
/// a day may be preceded by a space,
/// literals must match, and anything after the last field is ignored.
/// So `2026-02-31T08:00` and `2026-11-09T08:5x` are both valid, as they are in mainline.
pub fn is_iso_valid(date_time: &str) -> bool {
    let b = date_time.as_bytes();
    if b.len() != 16 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' {
        return false;
    }

    // The format is %Y-%m-%dT%H:%M.
    let Some((year, rest)) = after_field(b, 0, 9999, 4, false) else {
        return false;
    };
    let fields = [
        (b'-', 1, 12, false),
        (b'-', 1, 31, true),
        (b'T', 0, 23, false),
        (b':', 0, 59, false),
    ];
    let mut rest = rest;
    for (separator, min, max, skip_space) in fields {
        let Some((_, after)) = after_literal(rest, separator)
            .and_then(|input| after_field(input, min, max, 2, skip_space))
        else {
            return false;
        };
        rest = after;
    }
    year > 1900
}

/// Reads one field as libstdc++'s `_M_extract_num` does,
/// returning its value and the input after it.
fn after_field(
    input: &[u8],
    min: u32,
    max: u32,
    width: usize,
    skip_space: bool,
) -> Option<(u32, &[u8])> {
    let mut input = input;
    if skip_space && input.first() == Some(&b' ') {
        input = &input[1..];
    }
    let mut digits = 0;
    let mut value = 0;
    while digits < width {
        let Some(&digit @ b'0'..=b'9') = input.first() else {
            break;
        };
        value = value * 10 + u32::from(digit - b'0');
        input = &input[1..];
        digits += 1;
        if value > max {
            break;
        }
    }
    (digits > 0 && (min..=max).contains(&value)).then_some((value, input))
}

/// Matches one literal character of the format.
fn after_literal(input: &[u8], expected: u8) -> Option<&[u8]> {
    input.strip_prefix(&[expected])
}

/// Why a polyline could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("decode polyline: Bad encoded polyline")]
pub struct BadPolyline;

/// Decodes a polyline into `(lon, lat)` pairs as mainline's `midgard::decode` does.
///
/// Like mainline's `Shape5Decoder`, this reads bytes as signed `char`s,
/// wraps around on overflow, and fails only when the input ends mid-number.
pub fn decode_polyline(encoded: &[u8], precision: f64) -> Result<Vec<(f64, f64)>, BadPolyline> {
    let mut points = Vec::new();
    let mut i = 0;
    let (mut lat, mut lon) = (0i32, 0i32);
    let zigzag = |value: u32| {
        #[expect(
            clippy::cast_possible_wrap,
            reason = "zigzag decoding reinterprets the bits"
        )]
        let decoded = ((value >> 1) as i32) ^ -((value & 1) as i32);
        decoded
    };
    while i < encoded.len() {
        let lat_diff = read_varint(encoded, &mut i)?;
        let lon_diff = read_varint(encoded, &mut i)?;
        lat = lat.wrapping_add(zigzag(lat_diff));
        lon = lon.wrapping_add(zigzag(lon_diff));
        points.push((f64::from(lon) * precision, f64::from(lat) * precision));
    }
    Ok(points)
}

/// Reads one polyline number (`Shape5Decoder::read_varint`).
fn read_varint(encoded: &[u8], i: &mut usize) -> Result<u32, BadPolyline> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    loop {
        let &byte = encoded.get(*i).ok_or(BadPolyline)?;
        *i += 1;
        // `uint32_t(char)` sign-extends bytes above 0x7f.
        #[expect(
            clippy::cast_possible_wrap,
            clippy::cast_sign_loss,
            reason = "mainline reads the byte as a signed char"
        )]
        let byte = (i32::from(byte as i8) as u32).wrapping_sub(63);
        // Shifting past 31 bits is undefined in C++; x86-64 masks the shift, as Rust's wrapping shift does.
        result |= (byte & 0x1f).wrapping_shl(shift);
        shift += 5;
        if byte < 0x20 {
            return Ok(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates() {
        assert!(is_iso_valid("2026-11-09T08:00"));
        assert!(is_iso_valid("1901-01-01T00:00"));
        assert!(is_iso_valid("2026-02-31T23:59"));
        assert!(is_iso_valid("2026-11- 9T08:00"));
        assert!(!is_iso_valid("1900-01-01T00:00"));
        assert!(!is_iso_valid("2026-13-01T00:00"));
        assert!(!is_iso_valid("2026-00-01T00:00"));
        assert!(!is_iso_valid("2026-01-00T00:00"));
        assert!(!is_iso_valid("2026-01- 0T00:00"));
        assert!(!is_iso_valid("2026-01-01T24:00"));
        assert!(!is_iso_valid("2026-01-01T00:60"));
        assert!(!is_iso_valid("2026-01-01 00:00"));
        assert!(!is_iso_valid("2026-1-01T00:00"));
        assert!(!is_iso_valid("2026-01-01T00:00Z"));
        assert!(!is_iso_valid("current"));
        // libstdc++ stops a field at a non-digit and ignores what follows the last one.
        assert!(is_iso_valid("2026-11-09T08:5x"));
        assert!(!is_iso_valid("2026-11-09T08:x5"));
        assert!(!is_iso_valid("2026-1x-09T08:00"));
    }

    #[test]
    fn polylines() {
        // The polyline5 example from Google's documentation.
        let points = decode_polyline(b"_p~iF~ps|U_ulLnnqC_mqNvxq`@", 1e-5).expect("valid");
        let expected = [(-120.2, 38.5), (-120.95, 40.7), (-126.453, 43.252)];
        assert_eq!(points.len(), expected.len());
        for ((lon, lat), (e_lon, e_lat)) in points.iter().zip(expected) {
            assert!((lon - e_lon).abs() < 1e-9 && (lat - e_lat).abs() < 1e-9);
        }
        assert_eq!(decode_polyline(b"_p~iF", 1e-5), Err(BadPolyline));
        assert_eq!(decode_polyline(b"", 1e-6), Ok(Vec::new()));
    }
}
