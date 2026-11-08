//! Exit and guide signs on a maneuver (`odin::Sign` and `odin::Signs`).

use crate::error::OdinError;
use crate::markup_formatter::MarkupFormatter;
use crate::verbal_text_formatter::VerbalTextFormatter;
use valhalla_proto::{Pronunciation, TripSignElement};

/// How many kinds of guide signs [`Signs::guide_string`] splits its budget between.
const NUMBER_OF_GUIDE_SIGN_TYPES: u32 = 2;

/// One sign element: its text, whether it is a route number,
/// and how many consecutive maneuvers share it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sign {
    pub(crate) text: String,
    pub(crate) is_route_number: bool,
    pub(crate) consecutive_count: u32,
    pub(crate) pronunciation: Option<Pronunciation>,
}

impl Sign {
    pub(crate) fn new(
        text: String,
        is_route_number: bool,
        pronunciation: Option<Pronunciation>,
    ) -> Self {
        Self {
            text,
            is_route_number,
            consecutive_count: 0,
            pronunciation,
        }
    }

    /// A sign from a trip sign element.
    ///
    /// Mainline does not copy the element's consecutive count; it counts its own.
    pub(crate) fn from_proto(element: &TripSignElement) -> Self {
        Self::new(
            element.text.clone(),
            element.is_route_number,
            element.pronunciation.clone(),
        )
    }

    /// The sign as a directions sign element.
    pub(crate) fn to_proto(&self) -> TripSignElement {
        TripSignElement {
            text: self.text.clone(),
            is_route_number: self.is_route_number,
            consecutive_count: self.consecutive_count,
            pronunciation: self.pronunciation.as_ref().map(|p| Pronunciation {
                alphabet: p.alphabet,
                value: p.value.clone(),
            }),
            ..TripSignElement::default()
        }
    }
}

/// How to render a list of signs as text.
#[derive(Clone, Copy)]
pub(crate) struct SignFormat<'a> {
    /// The maximum number of signs, or zero for all of them.
    pub(crate) max_count: u32,
    /// Stop at the first sign with a different consecutive count than the first one.
    pub(crate) limit_by_consecutive_count: bool,
    /// The separator between signs.
    pub(crate) delim: &'a str,
    /// The verbal formatter (and markup formatter) to apply, for verbal instructions.
    pub(crate) verbal: Option<(VerbalTextFormatter, &'a MarkupFormatter)>,
}

impl SignFormat<'static> {
    /// Mainline's defaults: every sign, separated by `/`, unformatted.
    pub(crate) const DEFAULT: Self = Self {
        max_count: 0,
        limit_by_consecutive_count: false,
        delim: "/",
        verbal: None,
    };

    /// A textual (non-verbal) format with the given limits and the `/` separator.
    pub(crate) const fn text(max_count: u32, limit_by_consecutive_count: bool) -> Self {
        Self {
            max_count,
            limit_by_consecutive_count,
            delim: "/",
            verbal: None,
        }
    }
}

/// The sign lists of a maneuver.
#[derive(Debug, Clone, PartialEq, Default)]
#[expect(clippy::struct_field_names, reason = "mainline's names")]
pub(crate) struct Signs {
    pub(crate) exit_number_list: Vec<Sign>,
    pub(crate) exit_branch_list: Vec<Sign>,
    pub(crate) exit_toward_list: Vec<Sign>,
    pub(crate) exit_name_list: Vec<Sign>,
    pub(crate) guide_branch_list: Vec<Sign>,
    pub(crate) guide_toward_list: Vec<Sign>,
    pub(crate) junction_name_list: Vec<Sign>,
}

impl Signs {
    /// Sorts signs by descending consecutive count (`Signs::Sort`).
    ///
    /// Mainline uses `std::sort`, which is not stable in general,
    /// but libstdc++ sorts ranges of 16 or fewer elements by insertion sort, which is.
    /// A stable sort matches it for every realistic sign list.
    pub(crate) fn sort(signs: &mut [Sign]) {
        signs.sort_by_key(|sign| core::cmp::Reverse(sign.consecutive_count));
    }

    /// Counts the signs that a maneuver shares with the next one, then sorts both
    /// (`Signs::CountAndSort`).
    pub(crate) fn count_and_sort(prev_signs: &mut [Sign], curr_signs: &mut [Sign]) {
        for curr in curr_signs.iter_mut() {
            for prev in prev_signs.iter_mut() {
                if curr.text == prev.text {
                    curr.consecutive_count += 1;
                    prev.consecutive_count = curr.consecutive_count;
                }
            }
        }
        Self::sort(prev_signs);
        Self::sort(curr_signs);
    }

    #[inline]
    pub(crate) fn has_exit(&self) -> bool {
        self.has_exit_number()
            || self.has_exit_branch()
            || self.has_exit_toward()
            || self.has_exit_name()
    }

    #[inline]
    pub(crate) fn has_exit_number(&self) -> bool {
        !self.exit_number_list.is_empty()
    }

    #[inline]
    pub(crate) fn has_exit_branch(&self) -> bool {
        !self.exit_branch_list.is_empty()
    }

    #[inline]
    pub(crate) fn has_exit_toward(&self) -> bool {
        !self.exit_toward_list.is_empty()
    }

    #[inline]
    pub(crate) fn has_exit_name(&self) -> bool {
        !self.exit_name_list.is_empty()
    }

    #[inline]
    pub(crate) fn has_guide(&self) -> bool {
        self.has_guide_branch() || self.has_guide_toward()
    }

    #[inline]
    pub(crate) fn has_guide_branch(&self) -> bool {
        !self.guide_branch_list.is_empty()
    }

    #[inline]
    pub(crate) fn has_guide_toward(&self) -> bool {
        !self.guide_toward_list.is_empty()
    }

    #[inline]
    pub(crate) fn has_junction_name(&self) -> bool {
        !self.junction_name_list.is_empty()
    }

    /// Whether any exit, guide, or junction sign exists (`Maneuver::HasSigns`).
    #[inline]
    pub(crate) fn has_any(&self) -> bool {
        self.has_exit() || self.has_guide() || self.has_junction_name()
    }

    pub(crate) fn exit_number_string(&self, format: SignFormat<'_>) -> Result<String, OdinError> {
        list_to_string(&self.exit_number_list, format)
    }

    pub(crate) fn exit_branch_string(&self, format: SignFormat<'_>) -> Result<String, OdinError> {
        list_to_string(&self.exit_branch_list, format)
    }

    pub(crate) fn exit_toward_string(&self, format: SignFormat<'_>) -> Result<String, OdinError> {
        list_to_string(&self.exit_toward_list, format)
    }

    pub(crate) fn exit_name_string(&self, format: SignFormat<'_>) -> Result<String, OdinError> {
        list_to_string(&self.exit_name_list, format)
    }

    pub(crate) fn guide_branch_string(&self, format: SignFormat<'_>) -> Result<String, OdinError> {
        list_to_string(&self.guide_branch_list, format)
    }

    pub(crate) fn guide_toward_string(&self, format: SignFormat<'_>) -> Result<String, OdinError> {
        list_to_string(&self.guide_toward_list, format)
    }

    pub(crate) fn junction_name_string(&self, format: SignFormat<'_>) -> Result<String, OdinError> {
        list_to_string(&self.junction_name_list, format)
    }

    /// The guide branch and toward signs as one string (`GetGuideString`).
    ///
    /// With both kinds present, the budget is split:
    /// branch signs get `round(max / 2)` and toward signs `max / 2`.
    pub(crate) fn guide_string(&self, format: SignFormat<'_>) -> Result<String, OdinError> {
        let max = format.max_count;
        if self.has_guide_branch() && self.has_guide_toward() && (max == 0 || max > 1) {
            let branch_max = half_rounded(max);
            let branch = self.guide_branch_string(SignFormat {
                max_count: branch_max,
                ..format
            })?;
            let toward = self.guide_toward_string(SignFormat {
                max_count: max / NUMBER_OF_GUIDE_SIGN_TYPES,
                ..format
            })?;
            Ok(format!("{branch}{}{toward}", format.delim))
        } else if self.has_guide_branch() {
            self.guide_branch_string(format)
        } else if self.has_guide_toward() {
            self.guide_toward_string(format)
        } else {
            Ok(String::new())
        }
    }
}

/// `std::round(float(max) / 2)`, which rounds halves away from zero.
#[inline]
fn half_rounded(max: u32) -> u32 {
    max / NUMBER_OF_GUIDE_SIGN_TYPES + max % NUMBER_OF_GUIDE_SIGN_TYPES
}

/// Renders a sign list (`Signs::ListToString`).
fn list_to_string(signs: &[Sign], format: SignFormat<'_>) -> Result<String, OdinError> {
    let mut out = String::new();
    let mut consecutive_count = 0_u32;
    for (count, sign) in signs.iter().enumerate() {
        if format.max_count > 0 && count == format.max_count as usize {
            break;
        }
        if format.limit_by_consecutive_count {
            if count == 0 {
                consecutive_count = sign.consecutive_count;
            } else if sign.consecutive_count != consecutive_count {
                break;
            }
        }
        if !out.is_empty() {
            out.push_str(format.delim);
        }
        match format.verbal {
            Some((verbal, markup)) => out.push_str(&verbal.format_sign(sign, markup)?),
            None => out.push_str(&sign.text),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signs(texts: &[(&str, u32)]) -> Vec<Sign> {
        texts
            .iter()
            .map(|(text, count)| Sign {
                consecutive_count: *count,
                ..Sign::new((*text).to_owned(), false, None)
            })
            .collect()
    }

    #[test]
    fn count_and_sort_propagates_counts() {
        let mut prev = signs(&[("A", 0), ("B", 0)]);
        let mut curr = signs(&[("C", 0), ("B", 0)]);
        Signs::count_and_sort(&mut prev, &mut curr);
        assert_eq!(
            prev.iter()
                .map(|s| (s.text.as_str(), s.consecutive_count))
                .collect::<Vec<_>>(),
            [("B", 1), ("A", 0)]
        );
        assert_eq!(
            curr.iter()
                .map(|s| (s.text.as_str(), s.consecutive_count))
                .collect::<Vec<_>>(),
            [("B", 1), ("C", 0)]
        );
    }

    #[test]
    fn list_to_string_limits() {
        let list = signs(&[("A", 2), ("B", 2), ("C", 1)]);
        assert_eq!(list_to_string(&list, SignFormat::DEFAULT).unwrap(), "A/B/C");
        assert_eq!(
            list_to_string(&list, SignFormat::text(2, false)).unwrap(),
            "A/B"
        );
        assert_eq!(
            list_to_string(&list, SignFormat::text(0, true)).unwrap(),
            "A/B"
        );
    }

    #[test]
    fn guide_string_splits_the_budget() {
        let signs_with_guides = Signs {
            guide_branch_list: signs(&[("B1", 0), ("B2", 0), ("B3", 0)]),
            guide_toward_list: signs(&[("T1", 0), ("T2", 0), ("T3", 0)]),
            ..Signs::default()
        };
        assert_eq!(
            signs_with_guides
                .guide_string(SignFormat::text(4, false))
                .unwrap(),
            "B1/B2/T1/T2"
        );
        assert_eq!(
            signs_with_guides
                .guide_string(SignFormat::text(3, false))
                .unwrap(),
            "B1/B2/T1"
        );
        assert_eq!(
            signs_with_guides
                .guide_string(SignFormat::text(1, false))
                .unwrap(),
            "B1"
        );
        assert_eq!(
            signs_with_guides
                .guide_string(SignFormat::text(0, false))
                .unwrap(),
            "B1/B2/B3/T1/T2/T3"
        );
        assert_eq!(half_rounded(3), 2);
        assert_eq!(half_rounded(4), 2);
        assert_eq!(half_rounded(1), 1);
    }
}
