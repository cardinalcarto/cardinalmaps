"""Split `categories_export` output into one training file per language, and
report which languages have enough data for a model.

A language qualifies if the iD tagging schema translates it (its only source
of category vocabulary: Who's On First lists minority languages as spoken
across whole countries, so name counts alone would ship models for languages
whose corpus is really another language's names) and it has enough POIs.
"""

import argparse
import collections
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("export", type=Path, help="categories_export JSONL")
    parser.add_argument("augment", type=Path, help="categories-augment JSONL")
    parser.add_argument("out", type=Path, help="directory for <language>.jsonl files")
    parser.add_argument("--min-pois", type=int, default=50_000)
    parser.add_argument("--min-cuisine", type=int, default=1_000, help="min cuisine-tagged POIs")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    translated = {json.loads(line)["language"] for line in args.augment.open()}
    files, pois, cuisine, diet = {}, collections.Counter(), collections.Counter(), collections.Counter()
    try:
        for line in args.export.open():
            record = json.loads(line)
            for language, names in record["names"].items():
                if language not in files:
                    files[language] = (args.out / f"{language}.jsonl").open("w")
                row = {"names": names, **{k: record[k] for k in ("categories", "cuisine", "diet")}}
                files[language].write(json.dumps(row, ensure_ascii=False) + "\n")
                pois[language] += 1
                cuisine[language] += bool(record["cuisine"])
                diet[language] += bool(record["diet"])
    finally:
        for f in files.values():
            f.close()

    qualifying = []
    print(f"{'language':>8} {'POIs':>10} {'cuisine':>9} {'diet':>7}  iD  qualifies")
    for language, n in pois.most_common():
        ok = language in translated and n >= args.min_pois and cuisine[language] >= args.min_cuisine
        if ok:
            qualifying.append(language)
        print(f"{language:>8} {n:>10} {cuisine[language]:>9} {diet[language]:>7}  "
              f"{'yes' if language in translated else ' - '}  {'yes' if ok else ''}")
    (args.out / "qualifying.txt").write_text("".join(f"{l}\n" for l in qualifying))
    print(f"\n{len(qualifying)} languages qualify: {' '.join(qualifying)}")
