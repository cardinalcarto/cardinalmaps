"""Which OSM elements count as POIs, and whether two versions of one are the
same place.

`is_poi` and `same_place_label` mirror `poi_service/src/rules.rs`, which assigns
identifiers in production; a change to one needs the same change to the other
(the Rust test `matches_python_labels` compares them). `tags_to_text` renders
tags compactly for reviewing pairs.
"""

import unicodedata
from collections.abc import Mapping
from difflib import SequenceMatcher

# Keys listed at https://wiki.openstreetmap.org/wiki/Top-level_tag (fetched
# 2026-10-30), including the ones that only qualify as top-level in some
# contexts, such as `junction` or `club`.
TOP_LEVEL_KEYS = (
    "advertising", "aerialway", "aeroway", "amenity", "barrier", "boundary",
    "building", "club", "craft", "departures_board", "education", "emergency",
    "geological", "healthcare", "highway", "historic", "landcover", "landuse",
    "leisure", "man_made", "military", "natural", "office", "piste:type",
    "place", "power", "public_transport", "railway", "route", "shop",
    "telecom", "tourism", "waterway", "type",
    "addr:interpolation", "allotments", "area:highway", "attraction",
    "building:part", "bridge:support", "cemetery", "entrance", "ford", "golf",
    "indoor", "junction", "noexit", "playground", "roller_coaster",
    "traffic_calming", "traffic_sign",
)

# Top-level keys for transportation infrastructure; `addr:interpolation`,
# which describes address ranges rather than places; and `type`, which on
# relations only says how to read the relation (`type=route`,
# `type=multipolygon`, ...).
NON_POI_TOP_LEVEL_KEYS = frozenset({
    "addr:interpolation", "aerialway", "aeroway", "area:highway", "bridge:support",
    "departures_board", "ford", "highway", "junction", "noexit", "railway",
    "route", "traffic_calming", "traffic_sign", "type",
})

# Keys whose presence makes a named element a POI: every top-level key except
# those above. `public_transport` stays, since stations and stops are places
# people save. Must match `POI_KEYS` in `poi_service/src/rules.rs`.
POI_KEYS = tuple(key for key in TOP_LEVEL_KEYS if key not in NON_POI_TOP_LEVEL_KEYS)

# Tags that make a named element a POI though their key doesn't, such as bus
# stops mapped only as `highway=bus_stop`. Must match `POI_TAGS` in
# `poi_service/src/rules.rs`.
POI_TAGS = frozenset({
    ("highway", "bus_stop"),
    ("highway", "emergency_access_point"),
    ("highway", "rest_area"),
    ("highway", "trailhead"),
    ("railway", "subway_entrance"),
})

# POIs that, without a name or brand, are named by their operator (or the
# other keys listed), as the planetiler fork labels them. Must match
# `OPERATOR_NAMED` in `poi_service/src/rules.rs`.
OPERATOR_NAMED = {
    ("amenity", "atm"): ("operator", "network"),
    ("amenity", "charging_station"): ("operator",),
    ("amenity", "parcel_locker"): ("operator",),
}

# Keys an element needs at least one of to be a POI, for filtering files.
POI_FILTER_KEYS = POI_KEYS + tuple(sorted({key for key, _ in POI_TAGS}))

# Keys placed first in the text, in this order, so they survive truncation and
# identity-defining tags are easy to compare.
LEADING_KEYS = (
    "name",
    "brand",
    "operator",
    *POI_KEYS,
    "cuisine",
    "addr:housenumber",
    "addr:street",
    "addr:unit",
    "level",
)

# Tags that say nothing about which place this is. Dropping them keeps text
# short and means edits that only touch them produce identical text, which
# needs no review.
_DROPPED_KEYS = frozenset({
    "created_by",
    "fixme",
    "FIXME",
    "note",
    "description",
    "source",
    "attribution",
})
_DROPPED_PREFIXES = (
    "name:",
    "old_name:",
    "alt_name:",
    "official_name:",
    "brand:wikipedia",
    "wikipedia:",
    "source:",
    "note:",
    "fixme:",
    "check_date",
    "survey",
    "lastcheck",
    "last_checked",
    "mapillary",
    "image",
    "panoramax",
)

MAX_VALUE_CHARS = 80
MAX_TEXT_CHARS = 1000


def is_poi(tags: Mapping[str, str]) -> bool:
    """True if `tags` describe a named POI."""
    if "name" not in tags and "brand" not in tags and operator_name(tags) is None:
        return False
    return (any(key in tags for key in POI_KEYS)
            or any(tags.get(key) == value for key, value in POI_TAGS))


def keep_tag(key: str) -> bool:
    return key not in _DROPPED_KEYS and not key.startswith(_DROPPED_PREFIXES)


def tags_to_text(tags: Mapping[str, str]) -> str:
    """Serialize tags as `key=value; key=value` with leading keys first."""
    kept = {k: v for k, v in tags.items() if keep_tag(k)}
    ordered = [k for k in LEADING_KEYS if k in kept]
    leading = set(ordered)
    ordered += sorted(k for k in kept if k not in leading)
    parts = [f"{k}={kept[k][:MAX_VALUE_CHARS]}" for k in ordered]
    return "; ".join(parts)[:MAX_TEXT_CHARS]


def normalize_name(name: str) -> str:
    """`name` casefolded, without accents or punctuation."""
    name = unicodedata.normalize("NFKD", name)
    name = "".join(c if c.isalnum() else " " for c in name if not unicodedata.combining(c))
    return " ".join(name.casefold().split())


def operator_name(tags: Mapping[str, str]) -> str | None:
    """The name an operator-named POI goes by without a name or brand."""
    for (key, value), name_keys in OPERATOR_NAMED.items():
        if tags.get(key) == value:
            return next((tags[k] for k in name_keys if k in tags), None)
    return None


def normalized_name(tags: Mapping[str, str]) -> str:
    """The name (or brand, or for some POIs operator) of `tags`, normalized."""
    if "name" in tags or "brand" in tags:
        return normalize_name(tags.get("name", tags.get("brand", "")))
    return normalize_name(operator_name(tags) or "")


# Other names a place goes by. `old_name` is deliberately absent: mappers set
# it to the previous business's name when a new one moves in.
ALTERNATIVE_NAME_KEYS = ("short_name", "official_name", "alt_name", "loc_name", "int_name")


def alternative_names(tags: Mapping[str, str]) -> set[str]:
    """Normalized translations, transliterations and other names of a place."""
    values = [v for k, v in tags.items() if k.startswith("name:") or k in ALTERNATIVE_NAME_KEYS]
    names = {normalize_name(part) for value in values for part in value.split(";")}
    names.discard("")
    return names


# How alike two names' characters must be to count as a spelling change.
NAME_CHAR_SIMILARITY = 0.8


def similar_strings(na: str, nb: str) -> bool:
    """True if normalized name `nb` could be an edit of `na`.

    Accepts spelling fixes ("starbuks" / "starbucks"), spacing ("7 eleven" /
    "7eleven"), added or removed words ("cvs" / "cvs pharmacy"), and
    mostly-shared words ("joe s pizza bar" / "joe s pizza grill"). Names
    sharing only a generic word ("cafe milano" / "cafe deluxe") are not
    similar.
    """
    compact_a, compact_b = na.replace(" ", ""), nb.replace(" ", "")
    if compact_a in compact_b or compact_b in compact_a:
        return True
    words_a, words_b = set(na.split()), set(nb.split())
    if len(words_a & words_b) * 2 >= len(words_a | words_b):
        return True
    return SequenceMatcher(None, compact_a, compact_b).ratio() >= NAME_CHAR_SIMILARITY


def names_similar(a: Mapping[str, str], b: Mapping[str, str]) -> bool:
    """True if `b`'s name could be an edit of `a`'s rather than another place's.

    Besides `similar_strings` on the two names, a name also matches the other
    version's translations and alternative names, so switching the name to
    another language or script ("Asadaya Ryokan" / "浅田屋旅館") counts.
    """
    na, nb = normalized_name(a), normalized_name(b)
    if similar_strings(na, nb):
        return True
    return (any(similar_strings(na, alt) for alt in alternative_names(b))
            or any(similar_strings(alt, nb) for alt in alternative_names(a)))


def brand_changed(a: Mapping[str, str], b: Mapping[str, str]) -> bool:
    """True if both versions have a brand and it switched to another one.

    Matching `brand:wikidata` means the same brand however it's written
    ("セブン-イレブン" / "7-ELEVEN"); otherwise the brands must be dissimilar
    ("OMV" / "MOL", not "Total" / "TotalEnergies").
    """
    brand_a, brand_b = a.get("brand"), b.get("brand")
    if not brand_a or not brand_b:
        return False
    qid_a, qid_b = a.get("brand:wikidata"), b.get("brand:wikidata")
    if qid_a and qid_b and qid_a == qid_b:
        return False
    return not similar_strings(normalize_name(brand_a), normalize_name(brand_b))


def same_place_label(a: Mapping[str, str], b: Mapping[str, str]) -> bool:
    """True if two POI versions of one element describe the same place.

    A brand change only counts when the name changed too; with the name kept,
    it's nearly always a fix to a wrongly tagged brand.

    A POI known only by its operator (see `OPERATOR_NAMED`) is the same place
    as long as its operator is, so adding or removing a name doesn't change
    its identity.
    """
    def has_name(tags):
        return "name" in tags or "brand" in tags

    if not has_name(a) or not has_name(b):
        oa, ob = operator_name(a), operator_name(b)
        if oa is not None and ob is not None and similar_strings(normalize_name(oa), normalize_name(ob)):
            return True
    if not names_similar(a, b):
        return False
    return normalized_name(a) == normalized_name(b) or not brand_changed(a, b)


def changed_top_level_keys(a: Mapping[str, str], b: Mapping[str, str]) -> list[str]:
    """Top-level keys added, removed, or given a different value."""
    return [key for key in TOP_LEVEL_KEYS if a.get(key) != b.get(key)]
