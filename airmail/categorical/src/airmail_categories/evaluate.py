"""Score a trained model against the hand-written query set.

Applies the same rules as `airmail::categories` to turn head outputs into a
query's categories, cuisines and diets.
"""

import argparse
import json
import struct
from pathlib import Path

import fasttext

from airmail_categories.tags import normalize
from airmail_categories.train import MAGIC, load_taxonomy, with_ancestors


def read_header(path: Path) -> dict:
    with path.open("rb") as f:
        assert f.read(8) == MAGIC
        (length,) = struct.unpack("<I", f.read(4))
        return json.loads(f.read(length))


def probs(model, text: str) -> dict[str, float]:
    labels, ps = model.predict(normalize(text), k=-1, threshold=0.0)
    return {l.removeprefix("__label__"): float(p) for l, p in zip(labels, ps)}


def understand(models: dict, meta: dict, parents: dict, query: str) -> dict:
    t = meta["thresholds"]
    cats = probs(models["categories"], query)
    food = min(1.0, sum(cats.get(c, 0.0) for c in meta["food_categories"]))
    cuisines = {c: food * p for c, p in probs(models["cuisine"], query).items() if food * p >= t["cuisine"]}
    # Keep only the most specific confident cuisines.
    implied = {a for c in cuisines for a in with_ancestors([c], parents) if a != c}
    diets = probs(models["diet"], query)
    return {
        "categories": sorted((c for c, p in cats.items() if p >= t["categories"] and not c.startswith("building")), key=lambda c: -cats[c]),
        "best_category": max(cats, key=cats.get),
        "food": food,
        "cuisines": sorted(c for c in cuisines if c not in implied),
        "diets": sorted(d for d in meta["diets"] if diets.get(d, 0.0) >= t["diet"]),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("model", type=Path, help="exported model file (for thresholds and food categories)")
    parser.add_argument("work", type=Path, help="training work directory holding the fastText .bin files")
    parser.add_argument("--queries", type=Path, help="default: eval/queries.<model language>.tsv")
    parser.add_argument("--taxonomy", type=Path, default=Path("../airmail/dictionaries/cuisine_parents.txt"))
    parser.add_argument("--show", action="store_true", help="print every query's result")
    args = parser.parse_args()

    meta = read_header(args.model)
    queries = args.queries or Path(f"eval/queries.{meta['language']}.tsv")
    parents = load_taxonomy(args.taxonomy)
    models = {name: fasttext.load_model(str(args.work / f"{name}.bin")) for name in meta["head_order"]}

    totals = {"category": [0, 0], "cuisine": [0, 0], "no_cuisine": [0, 0], "diet": [0, 0], "no_diet": [0, 0]}
    for line in queries.open():
        if line.startswith("#") or not line.strip():
            continue
        query, category, cuisine, diet = (line.rstrip("\n").split("\t") + ["", "", ""])[:4]
        want_cuisines = {c for c in cuisine.split(";") if c}
        want_diets = {d for d in diet.split(";") if d}
        got = understand(models, meta, parents, query)
        checks = {
            "category": got["best_category"] == category,
            ("cuisine" if want_cuisines else "no_cuisine"):
                bool(want_cuisines & set(got["cuisines"])) if want_cuisines else not got["cuisines"],
            ("diet" if want_diets else "no_diet"): set(got["diets"]) == want_diets,
        }
        for key, ok in checks.items():
            totals[key][0] += ok
            totals[key][1] += 1
        if args.show or not all(checks.values()):
            mark = "  " if all(checks.values()) else "✗ "
            print(f"{mark}{query!r}: best={got['best_category']} food={got['food']:.2f} "
                  f"cuisines={got['cuisines']} diets={got['diets']}  want={category} {sorted(want_cuisines)} {sorted(want_diets)}")
    print()
    for key, (ok, n) in totals.items():
        print(f"{key:>10}: {ok}/{n}")
