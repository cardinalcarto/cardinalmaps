//! Addresses written as each country writes them, by OpenCage's
//! address-formatting templates (see `address_formatting/README.md`):
//! a port of their reference processor, Perl's `Geo::Address::Formatter`,
//! checked against the templates' own test cases.
//!
//! Components are OpenCage's: `house_number`, `road`, `house` (a building or
//! place's name), `neighbourhood`, `city`, `county`, `state`, `postcode`,
//! `country`, `country_code` and their aliases (`components.yaml`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;
use serde_yaml::Value;

const COMPONENTS: &str = include_str!("../address_formatting/components.yaml");
const TEMPLATES: &str = include_str!("../address_formatting/worldwide.yaml");
const STATE_CODES: &str = include_str!("../address_formatting/state_codes.yaml");
const COUNTY_CODES: &str = include_str!("../address_formatting/county_codes.yaml");

/// Countries where a `district` is a neighbourhood rather than a state
/// district.
const SMALL_DISTRICT: &[&str] = &["BR", "CR", "ES", "NI", "PY", "RO", "TG", "TM", "XK"];

/// A replacement rule: a regex on every component's value, or (as
/// `key=value`) on one component's, which it replaces outright if equal.
#[derive(Debug)]
struct Replace {
    whole: Regex,
    key: Option<(String, String, Option<Regex>)>,
    to: String,
}

#[derive(Debug, Default)]
struct Rules {
    address_template: Option<String>,
    fallback_template: Option<String>,
    replace: Vec<Replace>,
    postformat_replace: Vec<(Regex, String)>,
    use_country: Option<String>,
    change_country: Option<String>,
    add_component: Option<String>,
}

/// Codes and the names they stand for, by country.
type Codes = HashMap<String, Vec<(String, Vec<String>)>>;

/// The templates, ready to format with.
#[derive(Debug)]
pub struct Formatter {
    rules: HashMap<String, Rules>,
    /// Alias to the component it stands for.
    aliases: HashMap<String, String>,
    /// Every component and alias.
    known: HashSet<String>,
    state_codes: Codes,
    county_codes: Codes,
}

/// A Perl replacement string as a Rust one: `$1` as `${1}`, so that what
/// follows it isn't read as part of the group's name.
fn replacement(to: &str) -> String {
    static GROUP: OnceLock<Regex> = OnceLock::new();
    GROUP.get_or_init(|| Regex::new(r"\$(\d)").expect("regex")).replace_all(to, "$${$1}").into_owned()
}

fn pairs(value: Option<&Value>) -> Vec<(String, String)> {
    value
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            let pair = pair.as_sequence()?;
            Some((pair.first()?.as_str()?.to_string(), pair.get(1)?.as_str()?.to_string()))
        })
        .collect()
}

fn codes(yaml: &str) -> Codes {
    let value: Value = serde_yaml::from_str(yaml).expect("codes");
    let mut codes = HashMap::new();
    for (country, mapping) in value.as_mapping().into_iter().flatten() {
        let Some(country) = country.as_str() else { continue };
        let entries = mapping
            .as_mapping()
            .into_iter()
            .flatten()
            .filter_map(|(code, names)| {
                let code = match code {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => return None,
                };
                let names = match names {
                    Value::String(name) => vec![name.clone()],
                    Value::Mapping(by_language) => by_language.values().filter_map(|n| n.as_str().map(str::to_string)).collect(),
                    _ => return None,
                };
                Some((code, names))
            })
            .collect();
        codes.insert(country.to_string(), entries);
    }
    codes
}

impl Formatter {
    /// The formatter, made once.
    pub fn get() -> &'static Formatter {
        static FORMATTER: OnceLock<Formatter> = OnceLock::new();
        FORMATTER.get_or_init(Formatter::new)
    }

    fn new() -> Self {
        let mut aliases = HashMap::new();
        let mut known = HashSet::new();
        for document in serde_yaml::Deserializer::from_str(COMPONENTS) {
            let value = <Value as serde::Deserialize>::deserialize(document).expect("components");
            let Some(name) = value.get("name").and_then(Value::as_str) else { continue };
            known.insert(name.to_string());
            for alias in value.get("aliases").and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str) {
                known.insert(alias.to_string());
                aliases.insert(alias.to_string(), name.to_string());
            }
        }
        let templates: Value = serde_yaml::from_str(TEMPLATES).expect("templates");
        let mut rules = HashMap::new();
        for (key, value) in templates.as_mapping().expect("a mapping") {
            let (Some(key), Some(value)) = (key.as_str(), value.as_mapping()) else { continue };
            let text = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string);
            let replace = pairs(value.get("replace"))
                .into_iter()
                .map(|(from, to)| {
                    // Case-insensitively, as the reference processor does
                    // (Italy's `Città Metropolitana di` takes `Città
                    // metropolitana di Firenze`).
                    let regex = |pattern: &str| regex::RegexBuilder::new(pattern).case_insensitive(true).build();
                    let key = from
                        .split_once('=')
                        .filter(|(k, _)| k.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
                        .map(|(k, rest)| (k.to_string(), rest.to_string(), regex(rest).ok()));
                    Replace { whole: regex(&from).expect("replace regex"), key, to: replacement(&to) }
                })
                .collect();
            let postformat_replace = pairs(value.get("postformat_replace"))
                .into_iter()
                .map(|(from, to)| (Regex::new(&from).expect("postformat regex"), replacement(&to)))
                .collect();
            rules.insert(
                key.to_string(),
                Rules {
                    address_template: text("address_template"),
                    fallback_template: text("fallback_template"),
                    replace,
                    postformat_replace,
                    use_country: text("use_country"),
                    change_country: text("change_country"),
                    add_component: text("add_component"),
                },
            );
        }
        Self { rules, aliases, known, state_codes: codes(STATE_CODES), county_codes: codes(COUNTY_CODES) }
    }

    /// Whether the templates have a variant of the country's format for a
    /// language: `CA_fr`, `JP_ja`.
    #[must_use]
    pub fn has_variant(&self, country_code: &str, language: &str) -> bool {
        self.rules.contains_key(&format!("{}_{language}", country_code.to_ascii_uppercase()))
    }

    /// The country's code, after its rules: another country's format for a
    /// dependent territory, which may change the country or add a state.
    fn country_code(&self, components: &mut BTreeMap<String, String>) -> Option<String> {
        let code = components.get("country_code")?.clone();
        if code.chars().count() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        let mut code = code.to_ascii_uppercase();
        if code == "UK" {
            return Some("GB".to_string());
        }
        if let Some(rules) = self.rules.get(&code)
            && let Some(other) = &rules.use_country
        {
            if let Some(change) = &rules.change_country {
                static COMPONENT: OnceLock<Regex> = OnceLock::new();
                let component = COMPONENT.get_or_init(|| Regex::new(r"\$(\w*)").expect("regex"));
                let country = match component.captures(change) {
                    Some(c) => {
                        let value = components.get(&c[1]).cloned().unwrap_or_default();
                        change.replacen(&c[0], &value, 1)
                    }
                    None => change.clone(),
                };
                components.insert("country".to_string(), country);
            }
            if let Some((key, value)) = rules.add_component.as_deref().and_then(|a| a.split_once('='))
                && key == "state"
            {
                components.insert(key.to_string(), value.to_string());
            }
            code = other.to_ascii_uppercase();
        }
        if code == "NL"
            && let Some(state) = components.get("state").cloned()
        {
            let lower = state.to_lowercase();
            let territory = if state == "Curaçao" {
                Some(("CW", "Curaçao"))
            } else if lower.starts_with("sint maarten") {
                Some(("SX", "Sint Maarten"))
            } else if lower.starts_with("aruba") {
                Some(("AW", "Aruba"))
            } else {
                None
            };
            if let Some((territory, name)) = territory {
                code = territory.to_string();
                components.insert("country".to_string(), name.to_string());
            }
        }
        Some(code)
    }

    /// Sets components from their aliases where they aren't set.
    fn apply_aliases(&self, code: Option<&str>, components: &mut BTreeMap<String, String>) {
        let small_district = code.is_some_and(|c| SMALL_DISTRICT.contains(&c));
        let set: Vec<(String, String)> = components.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (key, value) in set {
            if key == "country_code" || key == "house_number" {
                continue;
            }
            let canonical = if key == "district" {
                Some(if small_district { "neighbourhood" } else { "state_district" }.to_string())
            } else {
                self.aliases.get(&key).cloned()
            };
            if let Some(canonical) = canonical {
                components.entry(canonical).or_insert(value);
            }
        }
    }

    /// Adds a state's or county's code, or its name if it's given as its
    /// code.
    fn add_code(codes: &Codes, key: &str, code: &str, components: &mut BTreeMap<String, String>) {
        let code_key = format!("{key}_code");
        let Some(name) = components.get(key).cloned() else { return };
        if components.get(&code_key).is_some_and(|c| *c != name) {
            return;
        }
        let Some(mapping) = codes.get(code) else { return };
        let upper = name.to_uppercase();
        'codes: for (abbreviation, names) in mapping {
            for configured in names {
                if upper == configured.to_uppercase() {
                    components.insert(code_key.clone(), abbreviation.clone());
                    break 'codes;
                }
                if upper == *abbreviation {
                    components.insert(key.to_string(), configured.clone());
                    components.insert(code_key.clone(), abbreviation.clone());
                    break 'codes;
                }
            }
        }
        if code == "US" && key == "state" && !components.contains_key("state_code") {
            let lower = name.to_lowercase();
            if lower.starts_with("united states") {
                let state = name["united states".len()..].trim_start_matches([',', ' ']).to_string();
                components.insert("state".to_string(), state.clone());
                Self::add_code(codes, key, code, components);
            } else if Regex::new(r"(?i)^washington,? d\.?c\.?").expect("regex").is_match(&name) {
                components.insert("state_code".to_string(), "DC".to_string());
                components.insert("state".to_string(), "District of Columbia".to_string());
                components.insert("city".to_string(), "Washington".to_string());
            }
        }
    }

    /// Formats components as lines of an address. With `codes`, states and
    /// counties are written by their codes where the country's format does.
    /// `language` picks a variant of the format, if there's one for it.
    #[must_use]
    pub fn format(&self, components: &BTreeMap<String, String>, language: Option<&str>, codes: bool) -> String {
        self.format_with_components(components, language, codes).0
    }

    /// As `format`, with the components as the template saw them: aliased,
    /// replaced and with any codes added.
    #[must_use]
    pub fn format_with_components(
        &self,
        components: &BTreeMap<String, String>,
        language: Option<&str>,
        codes: bool,
    ) -> (String, BTreeMap<String, String>) {
        let mut components: BTreeMap<String, String> =
            components.iter().map(|(k, v)| (k.clone(), v.trim().to_string())).filter(|(_, v)| !v.is_empty()).collect();
        let code = self.country_code(&mut components);
        if let Some(code) = &code {
            components.insert("country_code".to_string(), code.clone());
        }
        self.apply_aliases(code.as_deref(), &mut components);

        // Sanity.
        if let Some(postcode) = components.get("postcode").cloned() {
            static PAIR: OnceLock<Regex> = OnceLock::new();
            static ZIP: OnceLock<Regex> = OnceLock::new();
            if postcode.chars().count() > 20 || PAIR.get_or_init(|| Regex::new(r"\d+;\d+").expect("regex")).is_match(&postcode) {
                components.remove("postcode");
            } else if let Some(zip) = ZIP.get_or_init(|| Regex::new(r"^(\d{5}),\d{5}").expect("regex")).captures(&postcode) {
                components.insert("postcode".to_string(), zip[1].to_string());
            }
        }
        components.retain(|_, v| !v.contains("http://") && !v.contains("https://"));

        let variant = code.as_deref().zip(language).map(|(c, l)| format!("{c}_{l}")).filter(|v| self.rules.contains_key(v));
        let default = &self.rules["default"];
        let rules = variant.or(code.clone()).and_then(|c| self.rules.get(&c)).unwrap_or(default);
        let minimal = components.contains_key("road") || components.contains_key("postcode");
        let template = if minimal {
            rules.address_template.as_ref().or(default.address_template.as_ref())
        } else {
            rules.fallback_template.as_ref().or(default.fallback_template.as_ref()).or(rules.address_template.as_ref())
        }
        .expect("a template");

        // A numeric country with a state: the state is the country.
        if let (Some(country), true) = (components.get("country"), components.contains_key("state"))
            && country.parse::<f64>().is_ok()
        {
            let state = components.remove("state").expect("state");
            components.insert("country".to_string(), state);
        }
        let keys: Vec<String> = components.keys().filter(|k| k.as_str() != "country_code").cloned().collect();
        for key in keys {
            for rule in &rules.replace {
                let value = components.get_mut(&key).expect("key");
                let regex = match &rule.key {
                    Some((k, rest, regex)) if *k == key => {
                        if value == rest {
                            *value = rule.to.clone();
                            None
                        } else {
                            regex.as_ref()
                        }
                    }
                    _ => Some(&rule.whole),
                };
                if let Some(regex) = regex {
                    *value = regex.replace(value, rule.to.as_str()).into_owned();
                }
            }
        }
        if codes && let Some(code) = &code {
            Self::add_code(&self.state_codes, "state", code, &mut components);
            Self::add_code(&self.county_codes, "county", code, &mut components);
        }
        let mut unknown: Vec<&String> = components.keys().filter(|k| !self.known.contains(*k)).collect();
        unknown.sort();
        if !unknown.is_empty() {
            let attention = unknown.iter().map(|k| components[*k].as_str()).collect::<Vec<_>>().join(", ");
            components.insert("attention".to_string(), attention);
        }

        let text = render(template, &components);
        (clean(&postformat(&text, &rules.postformat_replace)), components)
    }
}

/// Fills in a template's `{{{component}}}`s, then picks each
/// `{{#first}} a || b {{/first}}`'s first alternative that isn't empty.
fn render(template: &str, components: &BTreeMap<String, String>) -> String {
    static VARIABLE: OnceLock<Regex> = OnceLock::new();
    static FIRST: OnceLock<Regex> = OnceLock::new();
    static ALTERNATIVES: OnceLock<Regex> = OnceLock::new();
    let variable = VARIABLE.get_or_init(|| Regex::new(r"\{\{\{?\s*([A-Za-z_]+)\s*\}?\}\}").expect("regex"));
    let first = FIRST.get_or_init(|| Regex::new(r"(?s)\{\{#first\}\}\s*(.+?)\s*\{\{/first\}\}").expect("regex"));
    let alternatives = ALTERNATIVES.get_or_init(|| Regex::new(r"\s*\|\|\s*").expect("regex"));
    let filled = variable.replace_all(template, |c: &regex::Captures| components.get(&c[1]).cloned().unwrap_or_default());
    let chosen = first.replace_all(&filled, |c: &regex::Captures| {
        alternatives.split(&c[1]).find(|a| !a.is_empty()).unwrap_or_default().to_string()
    });
    let out = clean(&chosen);
    // Nothing but one component: that alone.
    if !out.chars().any(char::is_alphanumeric) && components.len() == 1 {
        return components.values().next().cloned().unwrap_or_default();
    }
    out
}

/// Removes repeated pieces between commas, then applies the country's
/// replacements: each once, as Perl's `s///` does.
fn postformat(text: &str, replacements: &[(Regex, String)]) -> String {
    let mut seen = HashSet::new();
    let pieces: Vec<&str> = text
        .split(", ")
        .map(str::trim_start)
        .filter(|piece| piece.eq_ignore_ascii_case("new york") || seen.insert(piece.to_string()))
        .collect();
    let mut text = pieces.join(", ");
    // Perl's `$` matches before a final newline too.
    let ended = text.ends_with('\n');
    if ended {
        text.pop();
    }
    for (regex, to) in replacements {
        text = regex.replace(&text, to.as_str()).into_owned();
    }
    if ended {
        text.push('\n');
    }
    text
}

/// Tidies a rendered address: stray commas and spaces, empty and repeated
/// lines, and pieces repeated within a line.
fn clean(text: &str) -> String {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            (r"&#39;", "'"),
            (r"[\},\s]+$", ""),
            (r"^[,\s]+", ""),
            (r"^- ", ""),
            (r",\s*,", ", "),
            (r"[\t\p{Zs}]+,[\t\p{Zs}]+", ", "),
            (r"[\t\p{Zs}][\t\p{Zs}]+", " "),
            (r"[\t\p{Zs}]\n", "\n"),
            (r"\n,", "\n"),
            (r",,+", ","),
            (r",\n", "\n"),
            (r"\n[\t\p{Zs}]+", "\n"),
            (r"\n\n+", "\n"),
        ]
        .into_iter()
        .map(|(from, to)| (Regex::new(from).expect("regex"), to))
        .collect()
    });
    let mut out = text.to_string();
    for (i, (regex, to)) in rules.iter().enumerate() {
        // The first few are once, as in Perl; the rest everywhere.
        out = if (1..=3).contains(&i) { regex.replace(&out, *to).into_owned() } else { regex.replace_all(&out, *to).into_owned() };
    }
    let mut seen_lines = HashSet::new();
    let mut lines = Vec::new();
    for line in out.split('\n') {
        let line = line.trim_matches([' ', '\t']);
        if !seen_lines.insert(line.to_string()) {
            continue;
        }
        let mut seen_words = HashSet::new();
        let words: Vec<&str> = line
            .split(',')
            .map(|w| w.trim_matches([' ', '\t']))
            .filter(|w| w.eq_ignore_ascii_case("new york") || seen_words.insert(w.to_string()))
            .collect();
        lines.push(words.join(", "));
    }
    let mut out = lines.join("\n").trim().to_string();
    out.push('\n');
    out
}

#[cfg(test)]
mod test {
    use super::*;

    fn components(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn formats_as_countries_write_addresses() {
        let f = Formatter::get();
        let us = components(&[
            ("house_number", "301"),
            ("road", "Hamilton Avenue"),
            ("city", "Palo Alto"),
            ("postcode", "94303"),
            ("state", "California"),
            ("country", "United States of America"),
            ("country_code", "us"),
        ]);
        assert_eq!(f.format(&us, None, true), "301 Hamilton Avenue\nPalo Alto, CA 94303\nUnited States of America\n");
        assert_eq!(f.format(&us, None, false), "301 Hamilton Avenue\nPalo Alto, California 94303\nUnited States of America\n");
        let ch = components(&[("house_number", "1"), ("road", "Bahnhofstrasse"), ("city", "Zürich"), ("postcode", "8001"), ("country_code", "ch")]);
        assert_eq!(f.format(&ch, None, true), "Bahnhofstrasse 1\n8001 Zürich\n");
    }

    /// Every test case of a checkout of the templates, if
    /// `ADDRESS_FORMATTING` points at one.
    #[test]
    fn passes_the_templates_test_cases() {
        let Ok(root) = std::env::var("ADDRESS_FORMATTING") else { return };
        let f = Formatter::get();
        let (mut passed, mut failed) = (0, Vec::new());
        let mut paths: Vec<_> = std::fs::read_dir(format!("{root}/testcases/countries")).unwrap().map(|e| e.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read_to_string(&path).unwrap();
            for document in serde_yaml::Deserializer::from_str(&text) {
                let Ok(case) = <Value as serde::Deserialize>::deserialize(document) else { continue };
                let Some(input) = case.get("components").and_then(Value::as_mapping) else { continue };
                let input: BTreeMap<String, String> = input
                    .iter()
                    .filter_map(|(k, v)| {
                        let v = match v {
                            Value::String(s) => s.clone(),
                            Value::Number(n) => n.to_string(),
                            _ => return None,
                        };
                        Some((k.as_str()?.to_string(), v))
                    })
                    .collect();
                let expected = case.get("expected").and_then(Value::as_str).unwrap_or_default();
                let got = f.format(&input, None, true);
                if got == expected {
                    passed += 1;
                } else {
                    failed.push(format!("{}: {:?}\n  expected {expected:?}\n  got      {got:?}", path.display(), case.get("description")));
                }
            }
        }
        for failure in failed.iter().take(30) {
            eprintln!("{failure}");
        }
        assert!(failed.is_empty(), "{passed} passed, {} failed", failed.len());
    }
}
