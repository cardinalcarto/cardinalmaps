"""Tag conventions shared by augmentation and training."""

# Keys whose tags are POI categories. Copied from `POI_KEYS` in
# `poi_service/src/rules.rs`.
POI_KEYS = frozenset({
    "advertising", "allotments", "amenity", "attraction", "barrier", "boundary", "building",
    "building:part", "cemetery", "club", "craft", "education", "emergency", "entrance", "geological",
    "golf", "healthcare", "historic", "indoor", "landcover", "landuse", "leisure", "man_made",
    "military", "natural", "office", "piste:type", "place", "playground", "power", "public_transport",
    "roller_coaster", "shop", "telecom", "tourism", "waterway",
})

# Tags that are POI categories though their key isn't. Copied from `POI_TAGS`
# in `poi_service/src/rules.rs`.
POI_TAGS = frozenset({("highway", "bus_stop"), ("railway", "subway_entrance")})

# Values that don't say what the place is, such as `building=yes` or
# `shop=vacant`. They are not category labels.
UNINFORMATIVE_VALUES = frozenset({"yes", "no", "vacant", "disused"})

# Diet keys the model predicts intent for. Values `yes` and `only` count as serving it.
DIET_KEYS = ("vegan", "vegetarian", "halal", "gluten_free")
DIET_POSITIVE_VALUES = ("yes", "only")


def split_values(value: str | None) -> list[str]:
    """Split a `;`-separated OSM value into lowercase, trimmed parts."""
    return sorted({part.strip().lower() for part in (value or "").split(";") if part.strip()})


def normalize(text: str) -> str:
    """Model input normalization. Must match `normalize` in airmail's categories module."""
    return " ".join(text.lower().split())
