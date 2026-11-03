//! libpostal's dictionaries of address words
//! (<https://github.com/openvenues/libpostal>, MIT), by language: street
//! types, directionals, unit and level types, words like "near", stopwords,
//! and others. Each line is a canonical form and its alternatives,
//! separated by `|`.

include!(concat!(env!("OUT_DIR"), "/dictionaries.rs"));

/// A dictionary file's contents, by path relative to `dictionaries/`, such
/// as `en/street_types.txt`.
#[must_use]
pub fn file(path: &str) -> Option<&'static str> {
    FILES.binary_search_by(|(p, _)| (*p).cmp(path)).ok().map(|i| FILES[i].1)
}

/// Every dictionary of a type, such as `street_types`, with its language.
pub fn of_type(kind: &str) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
    FILES.iter().filter_map(move |(path, contents)| {
        let (language, name) = path.split_once('/')?;
        (name.strip_suffix(".txt") == Some(kind)).then_some((language, *contents))
    })
}
