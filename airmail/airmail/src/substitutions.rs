use std::{collections::HashMap, error::Error};

use crate::language::Language;
use regex::Regex;

lazy_static! {
    static ref ASCII_WHITESPACE_RE: Regex = Regex::new(r"[ \t\r\n]+").unwrap();
}

lazy_static! {
    static ref EN_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("en/street_types.txt").expect("dictionary")).unwrap();
    static ref CA_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("ca/street_types.txt").expect("dictionary")).unwrap();
    static ref ES_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("es/street_types.txt").expect("dictionary")).unwrap();
    static ref AR_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("ar/street_types.txt").expect("dictionary")).unwrap();
    static ref FR_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("fr/street_types.txt").expect("dictionary")).unwrap();
    static ref DE_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("de/street_types.txt").expect("dictionary")).unwrap();
    static ref IT_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("it/street_types.txt").expect("dictionary")).unwrap();
    static ref PT_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("pt/street_types.txt").expect("dictionary")).unwrap();
    static ref RU_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("ru/street_types.txt").expect("dictionary")).unwrap();
    static ref ZH_STREET_TYPES: SubstitutionDict =
        SubstitutionDict::from_str(airmail_tagger::dictionaries::file("zh/street_types.txt").expect("dictionary")).unwrap();
    static ref EMPTY_SUBS: SubstitutionDict = SubstitutionDict::empty();
}

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub(super) struct SubstitutionDict {
    subs: Vec<(String, Vec<String>)>,
}

impl SubstitutionDict {
    fn empty() -> Self {
        Self { subs: vec![] }
    }

    pub(super) fn from_str(contents: &str) -> Result<Self, Box<dyn Error>> {
        let mut subs: HashMap<String, Vec<String>> = HashMap::new();
        for line in contents.lines() {
            let components: Vec<_> = line.split('|').collect();
            for component in &components {
                if let Some(existing_subs) = subs.get_mut(*component) {
                    for component_to_add in &components {
                        if !existing_subs.contains(&component_to_add.to_string()) {
                            existing_subs.push(component_to_add.to_string());
                        }
                    }
                } else {
                    subs.insert(
                        component.to_string(),
                        components.iter().map(|s| s.to_string()).collect(),
                    );
                }
            }
        }
        Ok(Self {
            subs: subs.into_iter().collect(),
        })
    }

    /// Returns `token` followed by its synonyms, without duplicates.
    pub fn substitute(&self, token: &str) -> Vec<String> {
        let mut substitutions = vec![token.to_string()];
        for (key, subs) in &self.subs {
            if key == token {
                for sub in subs {
                    if !substitutions.contains(sub) {
                        substitutions.push(sub.clone());
                    }
                }
            }
        }
        substitutions
    }
}

fn sanitize(field: &str) -> String {
    ASCII_WHITESPACE_RE
        .replace_all(&deunicode::deunicode(field).to_lowercase(), " ")
        .to_string()
}

/// The most forms of a road name to index. Each word with synonyms multiplies
/// them (an "ave" has 8), so a long name made of such words, as OSM has, would
/// otherwise mean billions of strings.
const MAX_ROAD_FORMS: usize = 64;

/// Every way of writing `words` with `dict`'s synonyms, the words as they are
/// first, up to [`MAX_ROAD_FORMS`]. Later words vary before earlier ones, so a
/// capped list still has the street type's forms.
pub(super) fn apply_subs(words: &[String], dict: &SubstitutionDict) -> Vec<String> {
    fn walk(prefix: &mut Vec<String>, remaining: &[String], dict: &SubstitutionDict, out: &mut Vec<String>) {
        if out.len() >= MAX_ROAD_FORMS {
            return;
        }
        let Some((first, rest)) = remaining.split_first() else {
            out.push(prefix.join(" "));
            return;
        };
        for sub in dict.substitute(first) {
            prefix.push(sub);
            walk(prefix, rest, dict, out);
            prefix.pop();
            if out.len() >= MAX_ROAD_FORMS {
                return;
            }
        }
    }
    let mut out = Vec::new();
    walk(&mut Vec::with_capacity(words.len()), words, dict, &mut out);
    out
}

pub fn permute_road(road: &str, language: &Language) -> Result<Vec<String>, Box<dyn Error>> {
    let sub_dict: &SubstitutionDict = match language {
        Language::English => &EN_STREET_TYPES,
        Language::Arabic => &AR_STREET_TYPES,
        Language::Spanish => &ES_STREET_TYPES,
        Language::French => &FR_STREET_TYPES,
        Language::German => &DE_STREET_TYPES,
        Language::Italian => &IT_STREET_TYPES,
        Language::Portuguese => &PT_STREET_TYPES,
        Language::Russian => &RU_STREET_TYPES,
        Language::Chinese => &ZH_STREET_TYPES,
        Language::Catalan => &CA_STREET_TYPES,
        _ => &EMPTY_SUBS,
    };
    let road_tokens: Vec<String> = sanitize(road)
        .split_ascii_whitespace()
        .map(|s| s.to_string())
        .collect();
    Ok(apply_subs(&road_tokens, sub_dict))
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use crate::language::Language;

    use crate::substitutions::permute_road;

    fn assert_unique(permutations: &[String]) {
        let unique: HashSet<_> = permutations.iter().collect();
        assert_eq!(unique.len(), permutations.len(), "{permutations:?}");
    }

    #[test]
    fn test_permute_road() {
        let permutations = permute_road("fremont ave n", &Language::English).unwrap();
        assert_unique(&permutations);
        assert_eq!(permutations[0], "fremont ave n");
        assert!(permutations.contains(&"fremont avenue north".to_string()));
        assert!(permutations.contains(&"fremont av n".to_string()));
        // "fremont" has no synonyms, "ave" has 8 forms and "n" has 2.
        assert_eq!(permutations.len(), 8 * 2);
    }

    #[test]
    fn test_permute_road_cat() {
        let permutations = permute_road("carrer de villarroel", &Language::Catalan).unwrap();
        assert_unique(&permutations);
        assert_eq!(
            permutations,
            vec![
                "carrer de villarroel",
                "carr de villarroel",
                "c / de villarroel"
            ]
        );
    }

    #[test]
    fn long_roads_have_a_bounded_number_of_forms() {
        // Each "ave" has 8 forms: 8^30 of them in all.
        let road = vec!["fremont ave"; 30].join(" ");
        let start = std::time::Instant::now();
        let permutations = permute_road(&road, &Language::English).unwrap();
        assert!(start.elapsed().as_millis() < 100);
        assert_eq!(permutations.len(), 64);
        assert_eq!(permutations[0], road);
        assert_unique(&permutations);
        // The last word varies first.
        assert!(permutations[1].ends_with("fremont avenue"));
    }

    #[test]
    fn test_permute_road_no_dictionary() {
        let permutations = permute_road("Fremont  Ave", &Language::Tamil).unwrap();
        assert_eq!(permutations, vec!["fremont ave"]);
    }
}
