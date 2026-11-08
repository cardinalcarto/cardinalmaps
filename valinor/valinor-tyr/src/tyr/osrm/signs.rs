//! Destination, exit, and junction strings from guide signs
//! (`destinations`, `exits`, and the sign element helpers in `route_serializer_osrm.cc`).

use valhalla_proto::{TripSign, TripSignElement};

const SIGN_ELEMENT_DELIMITER: &str = ", ";
const DESTINATIONS_DELIMITER: &str = ": ";

/// Joins `parts` that are non-empty with `delimiter`, as mainline's repeated
/// "if neither is empty add the delimiter" steps do.
fn join_non_empty(parts: &[&str], delimiter: &str) -> String {
    let mut out = String::new();
    for part in parts {
        if !out.is_empty() && !part.is_empty() {
            out.push_str(delimiter);
        }
        out.push_str(part);
    }
    out
}

/// Appends each text, preceded by the delimiter unless nothing has been written yet,
/// as mainline's loops do (so empty texts add no delimiter until something is written).
fn append_each(texts: &[&str], delimiter: &str) -> String {
    let mut out = String::new();
    for text in texts {
        if !out.is_empty() {
            out.push_str(delimiter);
        }
        out.push_str(text);
    }
    out
}

/// `get_sign_element_refs` or `get_sign_element_nonrefs`.
fn elements(elements: &[TripSignElement], route_numbers: bool) -> String {
    let texts: Vec<&str> = elements
        .iter()
        .filter(|element| element.is_route_number == route_numbers)
        .map(|element| element.text.as_str())
        .collect();
    append_each(&texts, SIGN_ELEMENT_DELIMITER)
}

/// `get_sign_elements`: every element's text.
pub(super) fn sign_elements(elements: &[TripSignElement]) -> String {
    let texts: Vec<&str> = elements
        .iter()
        .map(|element| element.text.as_str())
        .collect();
    append_each(&texts, SIGN_ELEMENT_DELIMITER)
}

/// `exit_destinations`.
fn exit_destinations(sign: &TripSign) -> String {
    let refs = join_non_empty(
        &[
            &elements(&sign.exit_onto_streets, true),
            &elements(&sign.exit_toward_locations, true),
        ],
        SIGN_ELEMENT_DELIMITER,
    );
    let branch_nonrefs = elements(&sign.exit_onto_streets, false);
    let toward_nonrefs = elements(&sign.exit_toward_locations, false);
    // Exit names only count when there's nothing else.
    let name_nonrefs = if branch_nonrefs.is_empty() && toward_nonrefs.is_empty() {
        elements(&sign.exit_names, false)
    } else {
        String::new()
    };
    let nonrefs = join_non_empty(
        &[&branch_nonrefs, &toward_nonrefs, &name_nonrefs],
        SIGN_ELEMENT_DELIMITER,
    );
    join_non_empty(&[&refs, &nonrefs], DESTINATIONS_DELIMITER)
}

/// `guide_destinations`.
fn guide_destinations(sign: &TripSign) -> String {
    let refs = join_non_empty(
        &[
            &elements(&sign.guide_onto_streets, true),
            &elements(&sign.guide_toward_locations, true),
        ],
        SIGN_ELEMENT_DELIMITER,
    );
    let nonrefs = join_non_empty(
        &[
            &elements(&sign.guide_onto_streets, false),
            &elements(&sign.guide_toward_locations, false),
        ],
        SIGN_ELEMENT_DELIMITER,
    );
    join_non_empty(&[&refs, &nonrefs], DESTINATIONS_DELIMITER)
}

/// `destinations`: `<refs>: <non-refs>` from exit signs if there are any, else guide signs,
/// for example `I 99, US 220, US 30: Altoona, Johnstown`.
pub(super) fn destinations(sign: &TripSign) -> String {
    let exit_destinations_exist = !sign.exit_onto_streets.is_empty()
        || !sign.exit_toward_locations.is_empty()
        || !sign.exit_names.is_empty();
    if exit_destinations_exist {
        exit_destinations(sign)
    } else {
        guide_destinations(sign)
    }
}

/// `exits`: the exit numbers, joined with `; `.
pub(super) fn exits(sign: &TripSign) -> String {
    let numbers: Vec<&str> = sign.exit_numbers.iter().map(|n| n.text.as_str()).collect();
    append_each(&numbers, "; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(text: &str, is_route_number: bool) -> TripSignElement {
        TripSignElement {
            text: text.to_string(),
            is_route_number,
            ..TripSignElement::default()
        }
    }

    #[test]
    fn exit_destinations_combine_refs_and_names() {
        let sign = TripSign {
            exit_numbers: vec![element("165A", false), element("165B", false)],
            exit_onto_streets: vec![element("I 99", true), element("US 220", true)],
            exit_toward_locations: vec![element("US 30", true), element("Altoona", false)],
            exit_names: vec![element("Ignored", false)],
            guide_onto_streets: vec![element("Ignored too", false)],
            ..TripSign::default()
        };
        assert_eq!(destinations(&sign), "I 99, US 220, US 30: Altoona");
        assert_eq!(exits(&sign), "165A; 165B");
    }

    #[test]
    fn exit_names_only_without_other_nonrefs() {
        let sign = TripSign {
            exit_onto_streets: vec![element("A 1", true)],
            exit_names: vec![element("Main Exit", false), element("E 5", true)],
            ..TripSign::default()
        };
        assert_eq!(destinations(&sign), "A 1: Main Exit");
        let names_only = TripSign {
            exit_names: vec![element("Main Exit", false)],
            ..TripSign::default()
        };
        assert_eq!(destinations(&names_only), "Main Exit");
    }

    #[test]
    fn guide_destinations_without_exit_signs() {
        let sign = TripSign {
            guide_toward_locations: vec![element("Seattle", false), element("I 5", true)],
            junction_names: vec![element("Mercer", false), element("Junction", false)],
            ..TripSign::default()
        };
        assert_eq!(destinations(&sign), "I 5: Seattle");
        assert_eq!(sign_elements(&sign.junction_names), "Mercer, Junction");
        assert_eq!(destinations(&TripSign::default()), "");
        assert_eq!(exits(&TripSign::default()), "");
        // Mainline only adds a delimiter after something has been written.
        assert_eq!(
            sign_elements(&[element("", false), element("A", false)]),
            "A"
        );
    }
}
