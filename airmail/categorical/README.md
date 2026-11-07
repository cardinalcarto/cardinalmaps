# Categorical search model

Trains the models `airmail::categories` uses to tell what a query asks for: POI
categories (`amenity=restaurant`, `natural=peak`, …), cuisines and diets. There is
one model per language.

Each model is three fastText models exported into one file:

- **categories**: POI names → their category tags, as one softmax. Category tags are
  the POI keys and tags from `poi_service/src/rules.rs`. Each label has at most
  `--cap` examples, so common tags like `waterway=stream` don't swamp the rest.
- **cuisine**: names of cuisine-tagged POIs → their cuisines, plus every containing
  cuisine from `../airmail/dictionaries/cuisine_parents.txt`. POIs without a cuisine
  tag are left out rather than treated as "no cuisine". Cuisines only count for
  queries likely to be for a category in `../airmail/dictionaries/food_categories.txt`.
- **diet**: whether the *text* asks for a diet. The language's translated diet labels
  from the iD editor's tagging schema are positives. POI names are negatives,
  except names of POIs tagged with a diet and names containing a diet label.

Translated preset names and cuisine and diet labels from the iD tagging schema
(`augment.py`) fill in words that rarely appear in POI names.

## Which names train which language

`categories_export` (in `airmail_indexer`) looks up the languages Who's On First
lists as spoken where each POI is. A plain `name` trains every one of them, so POIs
in Quebec train both English and French. `name:xx` trains language xx only where xx
is spoken, so a transliterated `name:en` in Vietnam doesn't teach English what
Vietnamese words mean in Vietnam.

`categories-languages` splits the export by language and lists which languages
qualify for a model: they need an iD translation and enough POIs. (Who's On First
lists minority languages as spoken across whole countries, so without the iD
requirement Romansh would get a model trained on Swiss German names.)

## Running

```bash
cargo build --release -p airmail_indexer
../target/release/categories_export --wof-db ../data/wof-ch.spatial.db \
  --admin-cache ../data/admin-cache-ch data/switzerland-low.osm.pbf data/switzerland.export.jsonl

uv sync
uv run categories-augment data/augment.jsonl
uv run categories-languages data/switzerland.export.jsonl data/augment.jsonl data/languages-ch
uv run categories-train data/languages-ch/deu.jsonl data/augment.jsonl data/ch-deu.amcats \
  --language deu --work data/work-ch-deu
uv run categories-evaluate data/ch-deu.amcats data/work-ch-deu
```

`categories_export` reads any PBF, including a POI-filtered planet such as
`poi_service`'s `pois-*.osm.pbf`; for the planet, pass `--pip-tree` too.
`eval/queries.<language>.tsv` are small hand-written query sets;
`categories-evaluate` scores a model against the one for its language.

## Keeping Rust and Python in step

`tags.py` (`POI_KEYS`, `POI_TAGS`, `UNINFORMATIVE_VALUES`, `normalize`) and
`evaluate.understand` mirror `airmail/src/categories.rs`. After changing the export
format or inference, regenerate the parity-test model:

```bash
uv run categories-train data/languages-ch/deu.jsonl data/augment.jsonl \
  ../airmail/tests/fixtures/categories_tiny.amcats --language deu --work data/work-tiny \
  --fixtures ../airmail/tests/fixtures/categories_tiny_fixtures.json \
  --dim 8 --bucket 2000 --word-min-count 200 --epoch 5 --cap 2000 --diet-negatives 50000
```
