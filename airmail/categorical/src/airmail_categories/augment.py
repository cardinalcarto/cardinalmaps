"""Build training examples from the iD editor's translated tagging schema.

Each row is `{"language", "text", "categories"?, "cuisines"?, "diets"?,
"diet_label"?}`, with `language` the ISO 639-3 code of the translation. A
missing key means the row says nothing about that head; an empty list means
"none". `diet_label` marks a bare translated diet label, such as "vegan".
Regional translations (`de-CH`, `pt-BR`) count as their base language.
"""

import argparse
import io
import json
import tarfile
import urllib.request
from pathlib import Path

import langcodes

from airmail_categories.tags import DIET_KEYS, POI_KEYS, POI_TAGS, UNINFORMATIVE_VALUES

SCHEMA_VERSION = "7.3.0"
SCHEMA_URL = f"https://registry.npmjs.org/@openstreetmap/id-tagging-schema/-/id-tagging-schema-{SCHEMA_VERSION}.tgz"
# Presets whose names combine with cuisine and diet labels ("thai restaurant").
FOOD_PRESETS = ("amenity/restaurant", "amenity/fast_food", "amenity/cafe")


def load_schema(cache: Path) -> dict[str, dict]:
    """The schema's presets and per-language translations, downloading once into `cache`."""
    archive = cache / f"id-tagging-schema-{SCHEMA_VERSION}.tgz"
    if not archive.exists():
        cache.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(SCHEMA_URL) as response:
            archive.write_bytes(response.read())
    files = {}
    with tarfile.open(archive) as tar:
        for member in tar.getmembers():
            name = member.name.removeprefix("package/dist/")
            is_translation = name.startswith("translations/") and not name.endswith((".min.json", "index.json"))
            if name == "presets.json" or (is_translation and name.endswith(".json")):
                files[name] = json.load(io.TextIOWrapper(tar.extractfile(member)))
    return files


def iso_639_3(locale: str) -> str:
    """ISO 639-3 code of an iD locale such as `pt-BR` or `be@tarask`."""
    return langcodes.Language.get(locale.split("@")[0]).to_alpha3()


def preset_category(preset: dict) -> str | None:
    """`key=value` if the preset is exactly one POI category tag."""
    tags = preset.get("tags", {})
    if len(tags) != 1:
        return None
    (key, value), = tags.items()
    if value == "*" or value in UNINFORMATIVE_VALUES:
        return None
    if key in POI_KEYS or (key, value) in POI_TAGS:
        return f"{key}={value}"
    return None


def rows_for_language(presets: dict, translation: dict) -> list[dict]:
    """Rows for one translation; regional variants may only cover part of the schema."""
    t_presets, t_fields = translation.get("presets", {}), translation.get("fields", {})
    rows = []

    for preset_id, t_preset in t_presets.items():
        category = preset_category(presets.get(preset_id, {}))
        if category is None:
            continue
        for text in [t_preset.get("name", "")] + t_preset.get("terms", []):
            if text.strip():
                rows.append({"text": text, "categories": [category]})

    food = [(preset_category(presets[p]), t_presets[p]["name"]) for p in FOOD_PRESETS
            if t_presets.get(p, {}).get("name")]
    cuisine_labels = t_fields.get("cuisine", {}).get("options", {})
    # `pescetarian` is deliberately excluded: it isn't a modelled diet, and
    # some translations of it ("Fleischlos") mean vegetarian.
    diet_labels = {k: v for k, v in t_fields.get("diet_multi", {}).get("options", {}).items() if k in DIET_KEYS}

    for _, word in food:
        rows.append({"text": word, "diets": []})
    for value, text in cuisine_labels.items():
        rows.append({"text": text, "cuisines": [value], "diets": []})
        for category, word in food:
            for phrase in (f"{text} {word}", f"{word} {text}"):
                rows.append({"text": phrase, "categories": [category], "cuisines": [value], "diets": []})
    for key, text in diet_labels.items():
        rows.append({"text": text, "diets": [key], "diet_label": True})
        for category, word in food:
            for phrase in (f"{text} {word}", f"{word} {text}"):
                rows.append({"text": phrase, "categories": [category], "diets": [key]})
        for value, cuisine in cuisine_labels.items():
            for phrase in (f"{text} {cuisine}", f"{cuisine} {text}"):
                rows.append({"text": phrase, "cuisines": [value], "diets": [key]})
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path, help="JSONL output path")
    parser.add_argument("--cache", type=Path, default=Path("data"), help="where to keep the downloaded schema")
    args = parser.parse_args()

    files = load_schema(args.cache)
    seen, rows = set(), []
    for name, contents in sorted(files.items()):
        if not name.startswith("translations/"):
            continue
        locale = name.removeprefix("translations/").removesuffix(".json")
        language = iso_639_3(locale)
        for row in rows_for_language(files["presets.json"], contents[locale].get("presets", {})):
            row = {"language": language, **row}
            key = json.dumps(row, sort_keys=True, ensure_ascii=False)
            if key not in seen:
                seen.add(key)
                rows.append(row)
    with args.out.open("w") as out:
        for row in rows:
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
    languages = {row["language"] for row in rows}
    print(f"{len(rows)} augmentation rows in {len(languages)} languages written to {args.out}")
