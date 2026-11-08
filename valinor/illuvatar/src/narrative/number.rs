//! Locale-specific number formatting for lengths.
//!
//! Mainline writes distances into a `std::stringstream` imbued with the locale's
//! `std::locale(posix_locale)`, so the decimal point and digit grouping follow glibc's
//! locale data in the container mainline runs in.
//! The table below was measured in the pinned `ghcr.io/valhalla/valhalla:3.9.1` image
//! with libstdc++'s `std::numpunct<char>`:
//! every locale either groups by threes or not at all,
//! and libstdc++ narrows the multi-byte separators some locales use
//! (a no-break or narrow no-break space) to a plain space.

/// How a locale writes numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NumberFormat {
    decimal_point: char,
    thousands_separator: char,
    groups_by_three: bool,
}

impl NumberFormat {
    /// The "C" locale, which mainline falls back to when a locale cannot be loaded.
    const C: Self = Self {
        decimal_point: '.',
        thousands_separator: ',',
        groups_by_three: false,
    };

    /// The format for one of the locale files' `posix_locale` values.
    pub(crate) fn for_posix_locale(posix_locale: &str) -> Self {
        let (decimal_point, thousands_separator, groups_by_three) = match posix_locale {
            "ar_SA.UTF-8" => ('.', ',', false),
            "bg_BG.UTF-8" | "pt_PT.UTF-8" => (',', ',', false),
            "el_GR.UTF-8" | "sl_SI.UTF-8" => (',', '.', false),
            "en_AU.UTF-8" | "en_GB.UTF-8" | "en_US.UTF-8" | "he_IL.UTF-8" | "hi_IN.UTF-8"
            | "ja_JP.UTF-8" | "ko_KR.UTF-8" => ('.', ',', true),
            "ca_ES.UTF-8" | "da_DK.UTF-8" | "de_DE.UTF-8" | "es_ES.UTF-8" | "hu_HU.UTF-8"
            | "it_IT.UTF-8" | "mn_MN.UTF-8" | "nl_NL.UTF-8" | "pt_BR.UTF-8" | "ro_RO.UTF-8"
            | "tr_TR.UTF-8" | "vi_VN.UTF-8" => (',', '.', true),
            "cs_CZ.UTF-8" | "et_EE.UTF-8" | "fi_FI.UTF-8" | "fr_FR.UTF-8" | "nb_NO.UTF-8"
            | "pl_PL.UTF-8" | "ru_RU.UTF-8" | "sk_SK.UTF-8" | "sv_SE.UTF-8" | "uk_UA.UTF-8" => {
                (',', ' ', true)
            }
            _ => return Self::C,
        };
        Self {
            decimal_point,
            thousands_separator,
            groups_by_three,
        }
    }

    /// `std::fixed << std::setprecision(precision) << value`.
    pub(crate) fn fixed(&self, value: f32, precision: usize) -> String {
        self.localize(&format!("{:.*}", precision, f64::from(value)))
    }

    /// `<< value` with the stream's default formatting (`%g` with six significant digits).
    ///
    /// Mainline only writes whole numbers up to a few thousand this way,
    /// which `%g` prints as plain integers.
    pub(crate) fn general(&self, value: f32) -> String {
        self.localize(&format_general(f64::from(value)))
    }

    /// Swaps in the locale's decimal point and groups the integer digits.
    fn localize(&self, plain: &str) -> String {
        let (sign, rest) = plain
            .strip_prefix('-')
            .map_or(("", plain), |rest| ("-", rest));
        let (integer, fraction) = match rest.find(['.', 'e']) {
            Some(pos) => rest.split_at(pos),
            None => (rest, ""),
        };
        let mut out = String::with_capacity(plain.len() + integer.len() / 3);
        out.push_str(sign);
        if self.groups_by_three && integer.bytes().all(|b| b.is_ascii_digit()) {
            for (i, digit) in integer.chars().enumerate() {
                if i > 0 && (integer.len() - i) % 3 == 0 {
                    out.push(self.thousands_separator);
                }
                out.push(digit);
            }
        } else {
            out.push_str(integer);
        }
        match fraction.strip_prefix('.') {
            Some(digits) => {
                out.push(self.decimal_point);
                out.push_str(digits);
            }
            None => out.push_str(fraction),
        }
        out
    }
}

/// `printf("%g", value)`: six significant digits, trailing zeros removed,
/// and exponent notation outside `1e-4..1e6`.
fn format_general(value: f64) -> String {
    const PRECISION: i32 = 6;
    if value == 0.0 {
        return "0".to_owned();
    }
    if !value.is_finite() {
        return if value.is_nan() {
            "nan".to_owned()
        } else if value > 0.0 {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        };
    }
    // The exponent after rounding to six significant digits decides the notation.
    let scientific = format!("{:.*e}", (PRECISION - 1) as usize, value);
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if (-4..PRECISION).contains(&exponent) {
        let decimals = usize::try_from(PRECISION - 1 - exponent).unwrap_or(0);
        trim_fraction(&format!("{value:.decimals$}"))
    } else {
        let mantissa = trim_fraction(mantissa);
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.unsigned_abs())
    }
}

/// Removes trailing zeros (and a trailing decimal point) from a decimal number.
fn trim_fraction(number: &str) -> String {
    if number.contains('.') {
        number
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    } else {
        number.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values from `tools/oracle` measurement program, per locale group.
    #[test]
    fn matches_the_measured_libstdcxx_output() {
        let cases: &[(&str, [&str; 9])] = &[
            (
                "en_US.UTF-8",
                [
                    "1.5",
                    "2.5",
                    "12",
                    "1,234",
                    "12,345",
                    "1,234,567",
                    "10",
                    "900",
                    "1,000",
                ],
            ),
            (
                "ar_SA.UTF-8",
                [
                    "1.5", "2.5", "12", "1234", "12345", "1234567", "10", "900", "1000",
                ],
            ),
            (
                "bg_BG.UTF-8",
                [
                    "1,5", "2,5", "12", "1234", "12345", "1234567", "10", "900", "1000",
                ],
            ),
            (
                "ca_ES.UTF-8",
                [
                    "1,5",
                    "2,5",
                    "12",
                    "1.234",
                    "12.345",
                    "1.234.567",
                    "10",
                    "900",
                    "1.000",
                ],
            ),
            (
                "fr_FR.UTF-8",
                [
                    "1,5",
                    "2,5",
                    "12",
                    "1 234",
                    "12 345",
                    "1 234 567",
                    "10",
                    "900",
                    "1 000",
                ],
            ),
            (
                "el_GR.UTF-8",
                [
                    "1,5", "2,5", "12", "1234", "12345", "1234567", "10", "900", "1000",
                ],
            ),
            (
                "unknown",
                [
                    "1.5", "2.5", "12", "1234", "12345", "1234567", "10", "900", "1000",
                ],
            ),
        ];
        for (locale, expected) in cases {
            let format = NumberFormat::for_posix_locale(locale);
            let fixed = [1.5_f32, 2.5, 12.0, 1234.0, 12345.0, 1_234_567.0].map(|v| {
                let precision = usize::from(v != v.trunc());
                format.fixed(v, precision)
            });
            let general = [10.0_f32, 900.0, 1000.0].map(|v| format.general(v));
            let actual: Vec<&str> = fixed
                .iter()
                .chain(general.iter())
                .map(String::as_str)
                .collect();
            assert_eq!(actual, expected, "{locale}");
        }
    }

    #[test]
    fn general_format_matches_printf() {
        assert_eq!(format_general(0.0), "0");
        assert_eq!(format_general(100.0), "100");
        assert_eq!(format_general(123_456.0), "123456");
        assert_eq!(format_general(1_234_567.0), "1.23457e+06");
        assert_eq!(format_general(0.5), "0.5");
        assert_eq!(format_general(0.000_01), "1e-05");
        assert_eq!(format_general(-2.5), "-2.5");
    }
}
