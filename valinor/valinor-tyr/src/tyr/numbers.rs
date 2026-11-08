//! Number formatting exactly as mainline writes it.
//!
//! Mainline's serializers use two JSON writers with different number formatting:
//!
//! - rapidjson's `Writer` (wrapped by `rapidjson::writer_wrapper_t`),
//!   which writes the shortest round-tripping digits of a double
//!   and *truncates* them to a maximum number of decimal places
//!   (see [`write_rapidjson_double`]).
//! - `baldr::json`, whose `fixed_t` prints a fixed number of decimal places
//!   like `printf("%.*Lf")` (see [`write_fixed`]),
//!   and whose `float_t` prints like `printf("%g")` (see [`write_general`]).

use core::fmt::Write;

/// rapidjson's default maximum number of decimal places, which means "no limit".
pub(crate) const UNLIMITED_DECIMAL_PLACES: u32 = 324;

/// Writes a double as rapidjson's `Writer::Double` does with `SetMaxDecimalPlaces`.
///
/// rapidjson's Grisu2 produces the shortest digits that round-trip
/// (it can very rarely produce one digit more than the shortest;
/// we always write the shortest, which can only differ in the last place),
/// then `Prettify` lays them out:
/// integers get a trailing `.0`,
/// decimals beyond `max_decimal_places` are cut off (not rounded),
/// keeping at least one digit after the point,
/// and very large or small magnitudes use an exponent without a `+` sign (`1e30`, `1.5e-7`).
///
/// rapidjson writes nothing at all for NaN and infinities (leaving invalid JSON);
/// we write `null`, which is what `writer_wrapper_t` does for NaN key-value pairs.
pub(crate) fn write_rapidjson_double(out: &mut String, value: f64, max_decimal_places: u32) {
    if !value.is_finite() {
        out.push_str("null");
        return;
    }
    if value == 0.0 {
        if value.is_sign_negative() {
            out.push('-');
        }
        out.push_str("0.0");
        return;
    }
    if value < 0.0 {
        out.push('-');
    }
    let (digits, k) = shortest_digits(value.abs());
    prettify(out, &digits, k, i64::from(max_decimal_places));
}

/// The shortest round-tripping decimal digits of a positive finite value,
/// and the exponent `k` such that the value is `digits × 10^k`.
fn shortest_digits(value: f64) -> (Vec<u8>, i64) {
    // `{:e}` writes the shortest round-tripping digits as `d.ddde±x`.
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let k = exponent - (i64::try_from(digits.len()).unwrap_or(i64::MAX) - 1);
    (digits, k)
}

/// rapidjson's `Prettify` (`internal/dtoa.h`), writing into `out`.
fn prettify(out: &mut String, digits: &[u8], k: i64, max_decimal_places: i64) {
    let length = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let kk = length + k; // 10^(kk-1) <= v < 10^kk
    let push_digits = |out: &mut String, digits: &[u8]| {
        out.extend(digits.iter().map(|&d| char::from(d)));
    };
    if (0..=21).contains(&kk) && k >= 0 {
        // 1234e7 -> 12340000000.0
        push_digits(out, digits);
        out.extend(core::iter::repeat_n('0', usize::try_from(k).unwrap_or(0)));
        out.push_str(".0");
    } else if 0 < kk && kk <= 21 {
        // 1234e-2 -> 12.34
        let point = usize::try_from(kk).unwrap_or(0);
        push_digits(out, &digits[..point]);
        out.push('.');
        let decimals = &digits[point..];
        if k + max_decimal_places < 0 {
            // Truncate, then drop trailing zeros but keep at least one digit.
            let keep = usize::try_from(max_decimal_places)
                .unwrap_or(0)
                .min(decimals.len());
            let kept = trim_trailing_zeros(&decimals[..keep]);
            push_digits_or_zero(out, kept);
        } else {
            push_digits(out, decimals);
        }
    } else if -6 < kk && kk <= 0 {
        // 1234e-6 -> 0.001234
        let zeros = usize::try_from(-kk).unwrap_or(0);
        let mut decimals = vec![b'0'; zeros];
        decimals.extend_from_slice(digits);
        out.push_str("0.");
        if length - kk > max_decimal_places {
            let keep = usize::try_from(max_decimal_places)
                .unwrap_or(0)
                .min(decimals.len());
            let kept = trim_trailing_zeros(&decimals[..keep]);
            push_digits_or_zero(out, kept);
        } else {
            push_digits(out, &decimals);
        }
    } else if kk < -max_decimal_places {
        // Truncated to zero.
        out.push_str("0.0");
    } else if length == 1 {
        // 1e30
        push_digits(out, digits);
        out.push('e');
        write_exponent(out, kk - 1);
    } else {
        // 1234e30 -> 1.234e33
        push_digits(out, &digits[..1]);
        out.push('.');
        push_digits(out, &digits[1..]);
        out.push('e');
        write_exponent(out, kk - 1);
    }
}

fn trim_trailing_zeros(digits: &[u8]) -> &[u8] {
    let end = digits.iter().rposition(|&d| d != b'0').map_or(0, |i| i + 1);
    &digits[..end]
}

fn push_digits_or_zero(out: &mut String, digits: &[u8]) {
    if digits.is_empty() {
        out.push('0');
    } else {
        out.extend(digits.iter().map(|&d| char::from(d)));
    }
}

fn write_exponent(out: &mut String, exponent: i64) {
    // rapidjson writes the exponent's digits without a sign for positive exponents.
    let _ = write!(out, "{exponent}");
}

/// Writes a value as `baldr::json::fixed_t` does: `std::fixed` with the given precision.
///
/// The value is a `long double` in mainline,
/// but every value it formats starts out as a `double` or `float`,
/// which a `long double` holds exactly,
/// so formatting the `f64` gives the same correctly rounded digits
/// (ties go to even, as glibc's `printf` does).
/// Non-finite values are written as quoted strings (`"inf"`, `"-inf"`, `"nan"`, or `"-nan"`),
/// as mainline does.
pub(crate) fn write_fixed(out: &mut String, value: f64, precision: usize) {
    if value.is_finite() {
        let _ = write!(out, "{value:.precision$}");
    } else {
        out.push('"');
        write_non_finite(out, value);
        out.push('"');
    }
}

/// Writes a value as `baldr::json::float_t` does: `std::defaultfloat` with precision 6,
/// which is `printf("%g")`.
pub(crate) fn write_general(out: &mut String, value: f64) {
    const PRECISION: i64 = 6;
    if !value.is_finite() {
        out.push('"');
        write_non_finite(out, value);
        out.push('"');
        return;
    }
    if value == 0.0 {
        out.push_str(if value.is_sign_negative() { "-0" } else { "0" });
        return;
    }
    // Round to the precision first: that decides the exponent, as `%g` specifies.
    let scientific = format!("{value:.5e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i64 = exponent.parse().unwrap_or(0);
    if (-4..PRECISION).contains(&exponent) {
        let decimals = usize::try_from(PRECISION - 1 - exponent).unwrap_or(0);
        let fixed = format!("{value:.decimals$}");
        out.push_str(strip_fraction_zeros(&fixed));
    } else {
        out.push_str(strip_fraction_zeros(mantissa));
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        let _ = write!(out, "{:02}", exponent.abs());
    }
}

/// Drops trailing zeros after a decimal point, and the point itself if nothing is left.
fn strip_fraction_zeros(number: &str) -> &str {
    if number.contains('.') {
        number.trim_end_matches('0').trim_end_matches('.')
    } else {
        number
    }
}

/// How glibc's `printf` writes non-finite values.
fn write_non_finite(out: &mut String, value: f64) {
    if value.is_sign_negative() {
        out.push('-');
    }
    out.push_str(if value.is_nan() { "nan" } else { "inf" });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rapidjson(value: f64, max_decimal_places: u32) -> String {
        let mut out = String::new();
        write_rapidjson_double(&mut out, value, max_decimal_places);
        out
    }

    fn fixed(value: f64, precision: usize) -> String {
        let mut out = String::new();
        write_fixed(&mut out, value, precision);
        out
    }

    fn general(value: f64) -> String {
        let mut out = String::new();
        write_general(&mut out, value);
        out
    }

    /// The examples in rapidjson's own `dtoa` and `Writer` tests.
    #[test]
    fn rapidjson_doubles() {
        let unlimited = UNLIMITED_DECIMAL_PLACES;
        assert_eq!(rapidjson(0.0, unlimited), "0.0");
        assert_eq!(rapidjson(-0.0, unlimited), "-0.0");
        assert_eq!(rapidjson(1.0, unlimited), "1.0");
        assert_eq!(rapidjson(-1.0, unlimited), "-1.0");
        assert_eq!(rapidjson(1.2345, unlimited), "1.2345");
        assert_eq!(rapidjson(1.234_567_8, unlimited), "1.2345678");
        assert_eq!(rapidjson(0.123_456_78, unlimited), "0.12345678");
        assert_eq!(rapidjson(1.234_567_8e30, unlimited), "1.2345678e30");
        assert_eq!(rapidjson(1.234_567_8e-30, unlimited), "1.2345678e-30");
        assert_eq!(rapidjson(1.0e30, unlimited), "1e30");
        assert_eq!(rapidjson(1.0e-30, unlimited), "1e-30");
        assert_eq!(rapidjson(1.2345e-7, unlimited), "1.2345e-7");
        assert_eq!(rapidjson(0.000_001, unlimited), "0.000001");
        assert_eq!(rapidjson(1e21, unlimited), "1e21");
        assert_eq!(rapidjson(1e20, unlimited), "100000000000000000000.0");
        assert_eq!(rapidjson(f64::MAX, unlimited), "1.7976931348623157e308");
        assert_eq!(rapidjson(5e-324, unlimited), "5e-324");
        // Floats are promoted to doubles before formatting.
        assert_eq!(
            rapidjson(f64::from(2.207_f32), unlimited),
            "2.2070000171661377"
        );

        // Truncation with SetMaxDecimalPlaces (rapidjson's writertest).
        assert_eq!(rapidjson(0.0, 3), "0.0");
        assert_eq!(rapidjson(1.0, 3), "1.0");
        assert_eq!(rapidjson(0.123_45, 3), "0.123");
        assert_eq!(rapidjson(0.123_95, 3), "0.123");
        assert_eq!(rapidjson(1.2345, 3), "1.234");
        assert_eq!(rapidjson(1.2, 3), "1.2");
        assert_eq!(rapidjson(1.102, 2), "1.1");
        assert_eq!(rapidjson(1.002, 2), "1.0");
        assert_eq!(rapidjson(0.102, 2), "0.1");
        assert_eq!(rapidjson(0.0012, 2), "0.0");
        assert_eq!(rapidjson(0.0001, 3), "0.0");
        assert_eq!(rapidjson(-0.0001, 3), "-0.0");
        assert_eq!(rapidjson(1e-5, 3), "0.0");
        assert_eq!(rapidjson(1e-7, 3), "0.0");
        assert_eq!(rapidjson(123_456.789, 1), "123456.7");
        assert_eq!(rapidjson(f64::from(2.207_f32), 3), "2.207");
        assert_eq!(rapidjson(f64::from(42.507_59_f32), 6), "42.507591");
        assert_eq!(rapidjson(1e30, 3), "1e30");

        assert_eq!(rapidjson(f64::NAN, 3), "null");
        assert_eq!(rapidjson(f64::INFINITY, 3), "null");
    }

    #[test]
    fn fixed_doubles() {
        assert_eq!(fixed(17.0, 3), "17.000");
        assert_eq!(fixed(1.5211, 6), "1.521100");
        assert_eq!(fixed(0.0625, 3), "0.062");
        assert_eq!(fixed(0.1875, 3), "0.188");
        assert_eq!(fixed(-0.0, 3), "-0.000");
        assert_eq!(fixed(2.5, 0), "2");
        assert_eq!(fixed(f64::from(2.207_f32), 3), "2.207");
        assert_eq!(fixed(f64::INFINITY, 3), "\"inf\"");
        assert_eq!(fixed(f64::NEG_INFINITY, 3), "\"-inf\"");
        assert_eq!(fixed(f64::NAN, 3), "\"nan\"");
    }

    /// Checked against glibc's `printf("%g")`.
    #[test]
    fn general_doubles() {
        assert_eq!(general(0.0), "0");
        assert_eq!(general(-0.0), "-0");
        assert_eq!(general(1.0), "1");
        assert_eq!(general(0.1), "0.1");
        assert_eq!(general(0.0001), "0.0001");
        assert_eq!(general(0.000_012_345), "1.2345e-05");
        assert_eq!(general(123_456.0), "123456");
        assert_eq!(general(123_456.5), "123456");
        assert_eq!(general(1_234_567.0), "1.23457e+06");
        assert_eq!(general(999_999.5), "1e+06");
        assert_eq!(general(2.5e100), "2.5e+100");
        assert_eq!(general(-3.25), "-3.25");
        assert_eq!(general(f64::from(2.207_f32)), "2.207");
        assert_eq!(general(f64::NAN), "\"nan\"");
    }
}
