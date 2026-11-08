//! Street name lists (`baldr::StreetName`, `StreetNames`, and their US variants).
//!
//! Mainline distinguishes US street names, whose base name ignores directional prefixes
//! and suffixes ("North Main Street" and "Main Street" share a base name),
//! from all others, whose base name is the whole name.
//! The distinction is per element *and* per list:
//! an element keeps the kind it was created with,
//! while the operations that build a new list (finding common names, cloning, and filtering)
//! create elements of the list's kind.
//! [`StreetNames`] tracks both so that mixed lists behave exactly as in mainline.

use valhalla_proto::Pronunciation;

/// Directional prefixes that US base names ignore.
const US_PRE_DIRS: [&str; 8] = [
    "North ",
    "East ",
    "South ",
    "West ",
    "Northeast ",
    "Southeast ",
    "Southwest ",
    "Northwest ",
];
/// Directional suffixes that US base names ignore.
const US_POST_DIRS: [&str; 8] = [
    " North",
    " East",
    " South",
    " West",
    " Northeast",
    " Southeast",
    " Southwest",
    " Northwest",
];
/// Cardinal suffixes that mark a US route direction, like "US 30 West".
const US_POST_CARDINAL_DIRS: [&str; 4] = [" North", " East", " South", " West"];

/// Which flavor of street name rules applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum NameKind {
    /// Names are compared whole.
    #[default]
    Generic,
    /// US rules: directional prefixes and suffixes are not part of the base name.
    Us,
}

impl NameKind {
    /// The kind for a country code, as `StreetNamesFactory::Create` chooses it.
    #[inline]
    pub(crate) fn for_country(country_code: &str) -> Self {
        if country_code == "US" {
            Self::Us
        } else {
            Self::Generic
        }
    }
}

/// One street name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StreetName {
    value: String,
    is_route_number: bool,
    pronunciation: Option<Pronunciation>,
    kind: NameKind,
}

impl StreetName {
    pub(crate) fn new(
        value: String,
        is_route_number: bool,
        pronunciation: Option<Pronunciation>,
        kind: NameKind,
    ) -> Self {
        Self {
            value,
            is_route_number,
            pronunciation,
            kind,
        }
    }

    #[inline]
    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    #[inline]
    pub(crate) fn is_route_number(&self) -> bool {
        self.is_route_number
    }

    #[inline]
    pub(crate) fn pronunciation(&self) -> Option<&Pronunciation> {
        self.pronunciation.as_ref()
    }

    /// Mainline's `StreetName::operator==`, which ignores the pronunciation.
    #[inline]
    fn same_name(&self, other: &Self) -> bool {
        self.value == other.value && self.is_route_number == other.is_route_number
    }

    fn pre_dir(&self) -> &'static str {
        match self.kind {
            NameKind::Generic => "",
            NameKind::Us => US_PRE_DIRS
                .into_iter()
                .find(|dir| self.value.starts_with(dir))
                .unwrap_or(""),
        }
    }

    fn post_dir(&self) -> &'static str {
        match self.kind {
            NameKind::Generic => "",
            NameKind::Us => US_POST_DIRS
                .into_iter()
                .find(|dir| self.value.ends_with(dir))
                .unwrap_or(""),
        }
    }

    fn post_cardinal_dir(&self) -> &'static str {
        match self.kind {
            NameKind::Generic => "",
            NameKind::Us => US_POST_CARDINAL_DIRS
                .into_iter()
                .find(|dir| self.value.ends_with(dir))
                .unwrap_or(""),
        }
    }

    /// The name without its directional prefix and suffix (`GetBaseName`).
    fn base_name(&self) -> &str {
        let start = self.pre_dir().len();
        let post = self.post_dir().len();
        // When the prefix and suffix overlap ("North East"),
        // mainline's `substr` length underflows and it takes the rest of the string.
        let end = self
            .value
            .len()
            .checked_sub(post)
            .filter(|end| *end >= start);
        match end {
            Some(end) => &self.value[start..end],
            None => &self.value[start..],
        }
    }

    /// A copy of this name with another kind, as mainline copies into a list of that kind.
    fn with_kind(&self, kind: NameKind) -> Self {
        Self {
            kind,
            ..self.clone()
        }
    }
}

/// An ordered list of street names.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct StreetNames {
    names: Vec<StreetName>,
    kind: NameKind,
}

impl StreetNames {
    /// The names on a trip edge, as `StreetNamesFactory::Create` builds them.
    pub(crate) fn from_proto(kind: NameKind, names: &[valhalla_proto::StreetName]) -> Self {
        Self {
            names: names
                .iter()
                .map(|name| {
                    StreetName::new(
                        name.value.clone(),
                        name.is_route_number,
                        name.pronunciation.clone(),
                        kind,
                    )
                })
                .collect(),
            kind,
        }
    }

    /// A list built from `(value, is_route_number)` pairs, without pronunciations.
    pub(crate) fn from_pairs(kind: NameKind, names: Vec<(String, bool)>) -> Self {
        Self {
            names: names
                .into_iter()
                .map(|(value, is_route_number)| StreetName::new(value, is_route_number, None, kind))
                .collect(),
            kind,
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }

    #[inline]
    pub(crate) fn iter(&self) -> core::slice::Iter<'_, StreetName> {
        self.names.iter()
    }

    #[inline]
    pub(crate) fn first(&self) -> Option<&StreetName> {
        self.names.first()
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.names.clear();
    }

    /// A copy whose elements take this list's kind (mainline's virtual `clone`).
    pub(crate) fn duplicate(&self) -> Self {
        self.collect(self.names.iter().cloned())
    }

    fn collect(&self, names: impl Iterator<Item = StreetName>) -> Self {
        Self {
            names: names.map(|name| name.with_kind(self.kind)).collect(),
            kind: self.kind,
        }
    }

    /// The names in this list that also appear in `other` (`FindCommonStreetNames`).
    pub(crate) fn common_street_names(&self, other: &Self) -> Self {
        self.collect(
            self.names
                .iter()
                .filter(|name| other.names.iter().any(|other| name.same_name(other)))
                .cloned(),
        )
    }

    /// The names in this list whose base names appear in `other` (`FindCommonBaseNames`).
    ///
    /// When a match has a cardinal suffix ("US 30 West"), the suffixed name wins.
    pub(crate) fn common_base_names(&self, other: &Self) -> Self {
        self.collect(self.names.iter().filter_map(|name| {
            let other = other
                .names
                .iter()
                .find(|other| name.base_name() == other.base_name())?;
            if !name.post_cardinal_dir().is_empty() {
                Some(name.clone())
            } else if !other.post_cardinal_dir().is_empty() {
                Some(other.clone())
            } else {
                Some(name.clone())
            }
        }))
    }

    /// The names in this list that are not route numbers.
    pub(crate) fn non_route_numbers(&self) -> Self {
        self.collect(
            self.names
                .iter()
                .filter(|name| !name.is_route_number)
                .cloned(),
        )
    }
}

impl<'a> IntoIterator for &'a StreetNames {
    type Item = &'a StreetName;
    type IntoIter = core::slice::Iter<'a, StreetName>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.names.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(kind: NameKind, values: &[&str]) -> StreetNames {
        StreetNames::from_pairs(
            kind,
            values.iter().map(|v| ((*v).to_owned(), false)).collect(),
        )
    }

    fn values(names: &StreetNames) -> Vec<&str> {
        names.iter().map(StreetName::value).collect()
    }

    #[test]
    fn us_base_names_ignore_directionals() {
        let a = names(NameKind::Us, &["North Main Street", "US 30 West"]);
        let b = names(NameKind::Us, &["Main Street South", "US 30"]);
        // The name with a cardinal suffix wins, whichever list it is in.
        assert_eq!(
            values(&a.common_base_names(&b)),
            ["Main Street South", "US 30 West"]
        );
        assert_eq!(
            values(&b.common_base_names(&a)),
            ["Main Street South", "US 30 West"]
        );
        // Without a cardinal suffix on either side, this list's name wins.
        let c = names(NameKind::Us, &["Main Street"]);
        assert_eq!(values(&a.common_base_names(&c)), ["North Main Street"]);
        assert_eq!(values(&c.common_base_names(&a)), ["Main Street"]);
    }

    #[test]
    fn generic_base_names_are_whole_names() {
        let a = names(NameKind::Generic, &["North Main Street"]);
        let b = names(NameKind::Generic, &["Main Street"]);
        assert!(a.common_base_names(&b).is_empty());
    }

    #[test]
    fn overlapping_prefix_and_suffix_keep_the_rest() {
        let name = StreetName::new("North East".to_owned(), false, None, NameKind::Us);
        assert_eq!(name.base_name(), "East");
        let name = StreetName::new("North ".to_owned(), false, None, NameKind::Us);
        assert_eq!(name.base_name(), "");
    }

    #[test]
    fn common_names_compare_route_number_flags() {
        let a = StreetNames::from_pairs(NameKind::Generic, vec![("A1".to_owned(), true)]);
        let b = StreetNames::from_pairs(NameKind::Generic, vec![("A1".to_owned(), false)]);
        assert!(a.common_street_names(&b).is_empty());
        assert_eq!(values(&a.common_street_names(&a)), ["A1"]);
    }

    #[test]
    fn new_lists_take_the_list_kind() {
        // A US element in a generic list becomes generic when copied.
        let mut list = names(NameKind::Generic, &["North Main Street"]);
        list.names[0].kind = NameKind::Us;
        let copy = list.duplicate();
        assert_eq!(copy.names[0].kind, NameKind::Generic);
        assert_eq!(copy.names[0].base_name(), "North Main Street");
    }
}
