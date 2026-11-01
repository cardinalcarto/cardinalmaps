"""Train one language's categorical search model and export it for airmail.

Input is that language's file from `categories-languages`: POIs whose names
are in the language, or which are where it's spoken. Three fastText heads,
each its own model:

- `categories`: softmax over POI category tags. A POI with several tags
  contributes each of them, so the output is one distribution and P(food) is a
  sum over the food categories.
- `cuisine`: multi-label, trained only on cuisine-tagged POIs (untagged POIs
  say nothing about cuisine), with each value's taxonomy ancestors as labels.
- `diet`: whether the *text* asks for a diet. Translated diet labels are
  positives; POI names are negatives, except names of POIs tagged with a diet
  and names containing one of the language's diet labels, which may well
  mention a diet ("Vegan Garden", "Tawakal Halal").
"""

import argparse
import collections
import hashlib
import json
import random
import re
import struct
from pathlib import Path

import fasttext
import numpy as np

from airmail_categories.tags import DIET_KEYS, DIET_POSITIVE_VALUES, normalize, split_values

MAGIC = b"AMCATS01"
HEADS = ("categories", "cuisine", "diet")
LOSSES = {"categories": "softmax", "cuisine": "ova", "diet": "ova"}


def label(value: str) -> str:
    return "__label__" + value.replace(" ", "_")


def load_taxonomy(path: Path) -> dict[str, list[str]]:
    parents = {}
    for line in path.read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            child, *ps = line.split()
            parents[child] = ps
    return parents


def load_list(path: Path) -> list[str]:
    """Non-comment lines of a dictionary file."""
    return [line for raw in path.read_text().splitlines() if (line := raw.split("#", 1)[0].strip())]


def with_ancestors(values: list[str], parents: dict[str, list[str]]) -> list[str]:
    out, stack = set(), list(values)
    while stack:
        value = stack.pop()
        if value not in out:
            out.add(value)
            stack.extend(parents.get(value, []))
    return sorted(out)


def held_out(record: dict) -> bool:
    """Split by normalized first name, so chain branches never straddle train and test."""
    key = normalize(record["names"][0]).encode()
    return int(hashlib.sha1(key).hexdigest(), 16) % 10 == 0


def capped(records: list[dict], key: str, cap: int, seed: int = 0) -> list[dict]:
    """Keep each record while at least one of its labels has fewer than `cap` examples."""
    records = records[:]
    random.Random(seed).shuffle(records)
    counts = collections.Counter()
    kept = []
    for r in records:
        if any(counts[l] < cap for l in r[key]):
            kept.append(r)
            counts.update(r[key])
    return kept


def fit(path: Path, rows: list[tuple[list[str], str]], params: dict, loss: str) -> fasttext.FastText._FastText:
    lines = [" ".join(label(l) for l in labels) + " " + normalize(text) + "\n" for labels, text in rows if labels]
    random.Random(0).shuffle(lines)
    path.write_text("".join(lines))
    return fasttext.train_supervised(str(path), loss=loss, verbose=0, **params)


def serves_a_diet(record: dict) -> bool:
    return any(v in DIET_POSITIVE_VALUES for v in record["diet"].values())


def diet_label_pattern(labels: list[str]) -> re.Pattern | None:
    """Matches text containing a diet label at a word start, so "vegan" also
    catches "Vegano" and "Veganes". `None` without labels: an empty pattern
    would match every name."""
    words = sorted({normalize(label) for label in labels if label.strip()}, key=len, reverse=True)
    if not words:
        return None
    return re.compile(r"(?<!\w)(" + "|".join(map(re.escape, words)) + ")")


def export(models: dict, configs: dict, meta: dict, out: Path) -> None:
    """Write the model file read by `airmail::categories`.

    Layout: magic, u32 header length, JSON header, then for each head in
    `head_order`: input matrix as i8 (rows x dim), one f32 scale per row, and
    the output matrix as f32 (labels x dim). All little-endian.
    """
    header = {"version": 1, "head_order": list(HEADS), "heads": {}, **meta}
    blobs = []
    for name in HEADS:
        model, cfg = models[name], configs[name]
        words = model.get_words()
        labels = [l.removeprefix("__label__") for l in model.get_labels()]
        inp = model.get_input_matrix().astype(np.float32)
        outm = model.get_output_matrix().astype(np.float32)
        assert inp.shape == (len(words) + cfg["bucket"], cfg["dim"])
        scale = np.abs(inp).max(axis=1) / 127.0
        q = np.where(scale[:, None] > 0, np.rint(inp / np.where(scale > 0, scale, 1)[:, None]), 0).astype(np.int8)
        header["heads"][name] = {
            "dim": cfg["dim"], "minn": cfg["minn"], "maxn": cfg["maxn"], "bucket": cfg["bucket"],
            "word_ngrams": cfg["wordNgrams"], "loss": LOSSES[name], "words": words, "labels": labels,
        }
        blobs += [q.tobytes(), scale.astype("<f4").tobytes(), outm.astype("<f4").tobytes()]
    head = json.dumps(header, ensure_ascii=False).encode()
    with out.open("wb") as f:
        f.write(MAGIC + struct.pack("<I", len(head)) + head)
        for blob in blobs:
            f.write(blob)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pois", type=Path, help="one language's file from categories-languages")
    parser.add_argument("augment", type=Path)
    parser.add_argument("out", type=Path, help="exported model file")
    parser.add_argument("--language", required=True, help="ISO 639-3 code, e.g. deu")
    parser.add_argument("--taxonomy", type=Path, default=Path("../airmail/dictionaries/cuisine_parents.txt"))
    parser.add_argument("--work", type=Path, default=Path("data/work"))
    parser.add_argument("--cap", type=int, default=20000, help="max POIs per category label")
    parser.add_argument("--min-count", type=int, default=20, help="min POIs for a label to be learned")
    parser.add_argument("--diet-negatives", type=int, default=1_000_000, help="POI names used as diet negatives")
    parser.add_argument("--dim", type=int, default=64)
    parser.add_argument("--bucket", type=int, default=200_000)
    parser.add_argument("--word-min-count", type=int, default=5)
    parser.add_argument("--epoch", type=int, default=25)
    parser.add_argument("--threads", type=int, default=16)
    parser.add_argument("--food-categories", type=Path, default=Path("../airmail/dictionaries/food_categories.txt"))
    parser.add_argument("--fixtures", type=Path, help="write fastText predictions for parity tests here")
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=True)

    parents = load_taxonomy(args.taxonomy)
    records = [json.loads(line) for line in args.pois.open()]
    aug = [a for a in map(json.loads, args.augment.open()) if a["language"] == args.language]
    if not aug:
        raise SystemExit(f"no iD translation for {args.language}")
    train = [r for r in records if not held_out(r)]

    category_counts = collections.Counter(c for r in train for c in r["categories"])
    category_vocab = {c for c, n in category_counts.items() if n >= args.min_count}
    for r in train:
        r["categories"] = [c for c in r["categories"] if c in category_vocab]
        r["cuisines"] = with_ancestors(split_values(r["cuisine"]), parents)
    cuisine_counts = collections.Counter(c for r in train for c in r["cuisines"])
    cuisine_vocab = {c for c, n in cuisine_counts.items() if n >= args.min_count}

    food = sorted(c for c in load_list(args.food_categories) if c in category_vocab)

    small = dict(dim=args.dim, minn=2, maxn=5, wordNgrams=2, bucket=args.bucket, epoch=args.epoch,
                 lr=0.5, minCount=args.word_min_count, thread=args.threads)
    configs = {name: small for name in HEADS}

    cat_rows = [(r["categories"], n) for r in capped([r for r in train if r["categories"]], "categories", args.cap)
                for n in r["names"]]
    cat_rows += [(a["categories"], a["text"]) for a in aug if a.get("categories")]

    cui_rows = [([c for c in r["cuisines"] if c in cuisine_vocab], n) for r in train if r["cuisines"] for n in r["names"]]
    cui_rows += [(with_ancestors(a["cuisines"], parents), a["text"]) for a in aug if a.get("cuisines")]

    mentions_diet = diet_label_pattern([a["text"] for a in aug if a.get("diet_label")])
    negatives = [n for r in train if not serves_a_diet(r) for n in r["names"]
                 if mentions_diet is None or not mentions_diet.search(normalize(n))]
    random.Random(0).shuffle(negatives)
    diet_rows = [(a["diets"], a["text"]) for a in aug if a.get("diets")]
    diet_rows += [(["none"], a["text"]) for a in aug if a.get("diets") == []]
    diet_rows += [(["none"], n) for n in negatives[: args.diet_negatives]]

    models = {}
    for name, rows in (("categories", cat_rows), ("cuisine", cui_rows), ("diet", diet_rows)):
        print(f"training {name} on {len(rows)} rows", flush=True)
        models[name] = fit(args.work / f"{name}.txt", rows, configs[name], LOSSES[name])
        models[name].save_model(str(args.work / f"{name}.bin"))

    meta = {
        "language": args.language,
        "food_categories": food,
        "diets": list(DIET_KEYS),
        "thresholds": {"categories": 0.2, "cuisine": 0.5, "diet": 0.5},
    }
    export(models, configs, meta, args.out)
    print(f"exported {args.out} ({args.out.stat().st_size / 1e6:.1f} MB); food categories: {food}")

    if args.fixtures:
        queries = ["restaurant", "vegan pizza", "chinesisches Restaurant", "Bäckerei", "zürich hb",
                   "Café Sprüngli", "  Thai  FOOD ", "ristorante italiano", "unknownword xyz", "halal kebab"]
        fixtures = []
        for q in queries:
            entry = {"query": q}
            for name in HEADS:
                labels, probs = models[name].predict(normalize(q), k=10, threshold=0.0)
                entry[name] = {l.removeprefix("__label__"): float(p) for l, p in zip(labels, probs)}
            fixtures.append(entry)
        args.fixtures.write_text(json.dumps(fixtures, ensure_ascii=False, indent=1))
