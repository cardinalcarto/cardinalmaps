# Geocoder query design

Status: agreed plan. Nothing here is built. Numbers marked *illustrative* are starting points to calibrate, not measurements. Story numbers refer to [user-stories.md](user-stories.md).

## Why start over

Today's ranking adds up scores that can't be compared. A place's text score (BM25) is unbounded and depends on word rarity, while a category match is a small number: near Fremont, a Goodwill 3 km away scores 15 and a shop 8.7 km away with "Thrift Store" in its name scores 332. Every patch since has worked around that gap, which is how we reached a generic-search classifier, a second scoring path and 44 constants in `index.rs`, plus 14 in `interpret.rs`.

The tagger and the older rule-based `readings` also still interpret every query separately, and they disagreed in the original Fremont bug.

## Part 1: What we know about a query

**Context from the app.** The API changes to accept both of these, each optional:

- `viewport=west,south,east,north`: the map area left uncovered by the sheet. West greater than east means it crosses the antimeridian, as with today's `bbox`. It replaces `focus`. Unlike `bbox`, it never filters; it only informs ranking.
- `location=lon,lat`: where the user is. It's used only when the query asks for it, so it isn't logged and doesn't otherwise affect results.

Today the app sends only a focus point, sampled when the search field gets focus, and `prefix` is always on, so a finished search looks like one still being typed. It should also send `prefix=false` on submit.

**Tagger.** A CRF giving whole-query readings with probabilities, using gazetteer features (band, match quality, how known the area is, distance from the focus). Tags: Venue, Near, HouseNumber, Road, Unit, Postcode, Neighbourhood, Locality, County, Region, Country, Other. Its focus feature would use the viewport's center.

**We can't tell a name from a kind.** Venue covers both `starbucks` and `coffee shop`, and no model we can build will separate them: the permutations of every POI type in every language are too many, and we have no real user queries to train on. The design doesn't try. Every sought phrase is treated as possibly both.

The tagger's existing Near tag and its dictionaries of phrases like `near me` are enough to tell when a query means "here".

**Categorical model.** Per language, from sought words: categories, cuisines and diets, with probabilities. It says what *kind* of place the words suggest, for names as well as kinds: `starbucks` gets cafe at 0.96. It also makes mistakes: `mcdonalds` gets bus stop at 0.37. Its probabilities split across equivalent tags (charity 0.49, second-hand 0.39), so a single tag's probability isn't confidence.

**Documents.**

| Signal | Field | Cost to use |
|---|---|---|
| Name words, all indexed names | `name` | Postings |
| Whole-name forms | `name_exact` | One term lookup |
| Street, house number, unit, postcode | Text fields | Postings |
| Containing areas' names, by band | `admin_*` | Postings |
| Containing areas' IDs | `admin_ids` | One term lookup, and unambiguous |
| OSM tags, cuisine, diet | `indexed_tag` | One term lookup |
| Kind, band, country | `kind`, `placetype`, `country_code` | One term lookup |
| Location | `center` (fast), `s2_cover` | Per-document read; cell terms |
| Prominence | `importance` (fast) | Per-document read |

`importance` already folds in QRank, Wikidata and shape area for POIs, population for areas, road class for streets, and a discount for transit stops. It's on one scale: an ordinary POI at about 0.08, and the Eiffel Tower around 0.45 by the formula.

## Part 2: The design

Two rules: **retrieval decides which documents qualify, and ranking is one formula with bounded terms.** No search modes, and no deciding what kind of query it is.

### Step 1: Interpret, once

1. Run the existing tagger, which replaces `interpret.rs`. Readings that differ only in an area's band are grouped into one search, with their probabilities added, so `bookstore fremont` as Venue + Neighbourhood (0.36) and as Venue + Locality (0.23) is one search at 0.59. Groups are kept until they cover 90% of the tagger's belief, at most 3.
2. Run the category model on each distinct set of Venue words, ignoring `building=*` tags, which describe a structure and not what's in it.
3. **Find candidate areas** for each group's area words, keeping several so that one wrong guess can't empty the results.

**Finding candidate areas.** One admin query per group:

1. **Band weights** come from the tagger: `washington` might be region 0.9, locality 0.1. Only bands the tagger reads are searched.
2. **The query** finds admins named exactly the run's words, or beginning with them if the last word is unfinished, with any other areas in the query required in the admin's own hierarchy. So `fremont seattle` and `fremont california` each leave the right Fremont, and more words give better results. The most specific area is the one searched; the others only narrow it.
3. **Each match gets a weight:**
   ```
   candidate = P(band) × mass × nearness to the viewport
   ```
   This is the same `mass` and `nearness` as the ranking below. Weights are normalised to sum to 1.
4. **The top 3 are kept.**

A reading that retrieves nothing doesn't count: `bookshop fremont` as two neighbourhoods has no area called "bookshop fremont", so the Venue + area reading takes over without any special handling. A misspelled area also matches nothing, and gets only what the safety-net pass finds.

Candidate weights with the values used below (*illustrative*):

| Query | Map on | Candidates |
|---|---|---|
| `pizza washington` | Seattle | Washington State 97%, DC 3% |
| `pizza washington` | Arlington, VA | Washington State 73%, DC 27%, if the tagger's band weights stay the same |
| `coffee paris` | Seattle | Paris, France 89%, Paris, Texas 11% |
| `thrift store ballard` | Seattle | Seattle's Ballard 95%, Ballard, Mississippi 5% |
| `thrift store ballard` | None | Both about even, each with results, so it still works |

### Step 2: Retrieve

One query per reading, from fixed rules with no optional clauses:

| Words in reading | Requires | Notes |
|---|---|---|
| Venue | **Name arm** all in `name`, or **category arm** any predicted tag in `indexed_tag` | Both, always. Cuisine if predicted, otherwise category. Diets only rank |
| Road + HouseNumber | All in `street`, number exact, kind = address | Unit is never required |
| Road alone | All in `name` or `street` | |
| Postcode | Exact, or required with an address | |
| Area, neighbourhood or locality | In a candidate's `admin_ids`, or at a discount within its bounding box plus the margin (5 km), by S2 cells | Any of the candidates. A point-only area still gets the margin |
| Area, county or larger | In a candidate's `admin_ids` | A bounding box would be too loose: Washington's plus 5 km contains Portland |
| Only areas | All of the most specific area's words in `name` | An area's name is a name, whether it's the area's or a mountain's. Others narrow it. So the band only chooses where to search, never how areas rank |

A word is never dropped from the name arm. `eiffel tower` can't turn into `tower` near Seattle.

**Passes, stopping as soon as there are enough good results:**
1. Exact words, with the last word as a prefix only if unfinished.
2. Typos on the name arm, only if pass 1 found too little.
3. Every word in any text field, as a safety net for a misread query, scored at a discount. Not if the tagger's best reading is at least 0.9 likely: then nothing found means there's nothing to find.

**Limiting common matches.** Without a named area, the category arm searches outward from the anchor in S2 rings: the viewport, then 8× wider, then 64×, stopping once there are enough. A named area's box is its limit, with no widening (decision 3). Either way, a category's size doesn't set the cost. The name arm is global when its rarest word is selective, which is what lets `eiffel tower` work, and uses the same rings when it isn't. A one- or two-letter prefix matches only areas and important places, or places within the viewport.

### Step 3: Rank

Each retrieved document gets one score, with the best reading winning:

```
score    = P(group) × candidate × match × mass × nearness

mass     = 1 + W × importance
nearness = 1 / (1 + distance / reach)
reach    = scale × 10^(k × importance)
```

`candidate` is 1 when the query names no area. A neighbourhood or locality is searched twice: in its `admin_ids`, and in its box plus the margin with `candidate` discounted by `O`. So places really in it are always retrieved and rank first. A place takes its best score.

**Match** decides between name and category, as the higher of two values:

```
by name:     coverage × rarity
by category: C × (p / top)

coverage = share of the name's words the query covers: 1 for a whole name
rarity   = 1 + r × log10(max(1, T / share of documents with all the words in their names))
```

Here `C` is below 1, `p` is the tag's probability, and `top` is the highest probability among the predicted tags. So a charity shop isn't worth much less than a second-hand shop because the model split its belief between them.

**The idea:** nothing here asks what the query means. Two ordinary facts about a document's match do the work:

- **How much of its name was asked for.** "Starbucks" is all of a Starbucks' name, while "thrift store" is a third of Saint Vincent de Paul Thrift Store's. So having the words in a long name counts for little, and a category match can beat it.
- **How few places match.** Few places have all of `eiffel tower` or `charlie's queer books` in their names, so a match is strong evidence and can overcome thousands of kilometres. Very many have "starbucks" or "bookstore", so a match is weak evidence and distance decides. This counts places with all the words, not the rarest word: in the Puget index "bookstore" (30 documents) is as rare as "needle" (33).

`starbucks` and `coffee shop` are treated the same way, and both come out reasonably: whole-name matches are favoured nearby, and category matches take over with distance.

**Diets** multiply the match of places with the diet by a small bonus (`vegan pizza`).

**The anchor** sets `distance` and `scale`. Distance is measured from the anchor's center, and a box's size sets the scale. The first row that applies wins:

| Query | Anchor | `scale` |
|---|---|---|
| Ends in a standalone Near phrase (`near me`), with a `location` | The user's location, as a point | Minimum |
| Has a neighbourhood or locality candidate | That candidate's center | Half its bounding box's diagonal, at least the minimum |
| Has a `viewport`, including when it names a larger area | The viewport | Half its diagonal, at least the minimum |
| Has only a `location` | The user's location, as a point | Minimum |
| Has neither | None: nearness is 1 | None |

A larger area limits retrieval through `admin_ids`, so `coffee washington` means cafes in Washington, nearest the map first. `near me` with no `location` falls through to the viewport. A viewport without a `near me` always beats the user's location, which is what F2 needs: the map is panned to Tokyo, so `ramen` means Tokyo.

**Importance has a longer reach.** With `scale` 5 km and `k` 5, an ordinary POI's reach is about 13 km and the Eiffel Tower's about 900 km. Zooming in orders results sharply by distance; zooming out raises `scale`, so importance decides (D8, F3, F4).

**Worked examples.** These predate the spike: they're computed with `W` 2, `k` 5, `C` 0.7, `r` 5, `scale` 5 km and rarity from the rarest word, which the spike replaced. The spike has `r` 15 and `T` a share of 4e-6. D2 and F1 also use `O` 0.7. Those values and the importance and document frequency figures are *illustrative*, and they're there to show the shape.

| Story | Candidates and scores | Result |
|---|---|---|
| B1 `eiffel tower`, Seattle | Eiffel Tower, 8,000 km, rare: **3.04**. Space Needle by category: 1.26. Local attraction by category: 0.75 | Right |
| B4 `charlie's queer books`, from Spokane | Charlie's, 370 km, unique: **0.80**. Spokane bookshop by category: 0.75 | Right, **barely** |
| B10 `museum of glass`, Seattle | Museum of Glass, 40 km: **20**. Nearby museum by category: 0.94 | Right |
| C1 `starbucks`, Fremont | Starbucks at 4 km: **0.88**. Cafe at 100 m: 0.81. Starbucks at 15 km: 0.53 | **Mixed**: see below |
| C3 `starbucks`, rural | Cafe at 1 km: **0.75**. Starbucks at 60 km: 0.20 | **Changes the story**: see below |
| Small chain, rural | Chain with 100 locations, at 60 km: **2.21**. Cafe at 1 km: 0.75 | Right |
| D1 `thrift store`, Fremont | Goodwill at 3 km: **0.66**. Magpie Thrift Store at 10 km: 0.43. Place named Thrift Store at 22 km: 0.42 | Right |
| D2 `thrift store ballard`, Seattle | Shop inside Ballard at 1 km: **0.72**. Goodwill, just outside by the data, at 1.1 km: 0.50. Shop outside at 4 km: 0.37 | Right |
| F1 `coffee paris`, Seattle | Cafe in Paris, France: **0.64**. Cafe in Paris, Texas: 0.08 | Right |
| D10 `bookstore`, Seattle | Bookshop at 2 km: **0.70**. Place named Bookstore at 225 km: 0.06 | Right |
| E3 `new york`, Seattle | New York: **5.56**. New York Alki at 8 km: 2.13 | Right |
| E5 `vancouver`, in Vancouver, WA | Vancouver, BC: **13.46**. Vancouver, WA: 12.71 | **Wrong** with these values |
| E9 `leavenworth`, Seattle | Washington's: **4.86**. Kansas's: 0.36 | Right |

### Stories this changed

Without a way to tell a name from a kind, two stories were changed, as agreed:

- **C1 `starbucks`, nearby:** Starbucks locations mostly come first, but a cafe very close by can rank above a Starbucks a few km away.
- **C3 `starbucks`, rural:** nearby cafes rank above a Starbucks 60 km away. To keep it reachable, the nearest whole-name match is always kept in the results, at the end if necessary. A smaller chain still ranks first, because its name is rarer.

The formula can't separate these without calibration:

- **E5:** a larger namesake nearly matches the area under the map.
- **B4:** a unique name from far away only just beats local category matches. Raising `r` helps, but that also strengthens small chains.
- **`pizza washington` from Arlington:** band weights can outweigh nearness, as the candidate table shows. The tagger does use distance as a feature, so its weights may shift, but that's untested.
- **Margin size:** 5 km turns Ballard's 4 km box into about 14 km, covering Fremont and Greenwood. `inside` keeps Ballard first, but how it feels needs trying on the phone.

**The model's mistakes reach the results.** `mcdonalds` retrieves bus stops by category, and with these values a bus stop at 100 m scores about 0.75, above any McDonald's more than about 5 km away. `wendys` (pawnbroker) and `dennys` (station) look the same. This is probably an apostrophe mismatch rather than a flaw in the design: the model learned from names like "McDonald's", while the tagger path sends it words with the apostrophe already removed. Training on names with occasional missing apostrophes should fix it. That's a later change, separate from this work.

### Parameters

A dozen in total: `W`, `k`, minimum scale, area margin, `O`, `C`, `r`, `T`, unseen coverage, diet bonus, the typo and safety-net discounts. Plus retrieval limits (reading groups, candidates, enough results, unselective threshold), which affect speed more than order.

These are tuned on the whole story set at once. **A new parameter or rule has to improve several stories; one query isn't enough.**

`rarity` depends on how many documents hold a word, so **it can only be calibrated on the planet index**. The Puget index has POIs from only one state, so words look rarer than they are.

### What goes away and what stays

- **Goes:** BM25 in final ranking, `interpret.rs` and its 14 constants, phrase subsequences, exact-prefix constants, all of today's generic-search and named-area code, literal-query discounts, the two-factor focus boost, and area matching against `admin_*` text for ordinary places.
- **Stays:** the tagger and category models, both unchanged and not retrained; the gazetteer, importance values, S2 covers, de-duplication of areas and merging of street pieces.
- **To retest, then probably go:** `rank_areas_above_their_namesakes` and the ancestor reordering in `rerank_admins`, since areas-only queries now rank by candidate weight.

### Reindexing

`coverage` needs the word count of a document's primary name, which isn't available cheaply today. `name` holds every name form together, so its length doesn't help, and reading stored documents means decoding their geometry. So the design adds one small fast field, `name_words`, which needs a reindex and fits the planet build we still have to run. Whole names in any language are already found through `name_exact`. A spike can read stored names and accept the slowdown.

## Part 3: Performance

**Measured on the dev index** (7.2 million documents, repeat queries on this machine, three queries per class or more):

| Query class | Finished: median | App params: median | App params: max |
|---|---|---|---|
| Finished name | 5 ms | 78 ms | 79 ms |
| One or two letters, short words | 22 ms | 193 ms | 715 ms |
| Partial name | 5 ms | 63 ms | 127 ms |
| Category | 2 ms | 82 ms | 94 ms |
| Category + area | 3 ms | 114 ms | 299 ms |
| Address | 6 ms | 141 ms | 216 ms |
| Long query | 25 ms | 475 ms | 619 ms |

The "finished" column has typos and prefix off. Those two options cost roughly 10 to 40 times more, and the app sends them on every search. A planet index will be much larger, and these numbers don't tell us how much worse it gets.

| Today's cost | Design change |
|---|---|
| Fuzzy and prefix matching for every word, in up to 10 fields | Typos only as a later pass, on the name arm only |
| Every word against 5 admin text fields, for every document | One admin query per group, then S2 cells or `admin_ids` |
| Phrase queries for every run of words: grows with the square of query length | None; `coverage` replaces them |
| Literal query plus up to 4 interpretations, plus fallbacks | At most 3 reading groups, with duplicate Venue words searched once, safety net only if needed |
| Preflight lookups (generic test, well-known names) | None for these. Rarity is a term lookup: document frequency, already in the index |
| Reading and scoring every match of a common word | Rings for the category arm and for unselective names |
| Decoding stored documents, geometry included, for at least 20 candidates beyond the limit | Rank from fast fields; decode only a little over the limit |

The new costs are two arms for every Venue reading, and finding candidates for each area. Most queries should still get cheaper, because the arms are simple and the typo pass is usually skipped.

**Targets to confirm on the planet:** p50 under 30 ms and p99 under 150 ms with app parameters.

**Unknowns to measure in a spike, before committing:**
- Whether S2 ring queries are as cheap as assumed when combined with a tag.
- The cost of finding candidates, especially reading the geometry of up to 3 large areas for their bounding boxes. If it's slow, bounding boxes could get their own stored field, which needs a reindex.
- Tagger and gazetteer time per query, on the planet. Not measured yet.
- Whether reading fast fields per match is acceptable for the largest name postings.
- Whether "enough good results" stops early often enough to skip typos during typing.

## Part 4: Rollout

1. **Turn the stories into evals.** Extend `eval/search.tsv` with viewports, about 80 cases across stories, plus Pelias acceptance tests, and report by story group. Many stories need the planet: B1, B8, E4 and F1 can't pass on the Puget index, which has no POIs outside Washington.
2. **Spike** retrieval and ranking behind a request parameter, next to the old path, reading stored names for `coverage`. Answer the performance unknowns.
3. **`viewport`, `location` and submit** in the API and app.
4. **Add `name_words`** to the index, in the planet build.
5. **Calibrate** on the planet, on the whole eval, once.
6. **Delete the old path.**

Today's uncommitted ranking work wouldn't carry over, except the trailing-space fix.

## Decisions made

1. **C1 and C3** change as described above, and the nearest whole-name match is always kept.
2. **BM25 comes out of final ranking.** `rarity` keeps its word-rarity signal from document frequency, and `coverage` replaces its length normalisation. Term frequency is dropped. Adding more of it back can be revisited later.

## Following the recommendation for now

3. **Nothing within the margin** shows nothing, with no widening in rings. `museum fremont` shows museums within 5 km of Fremont. Check how often it comes up.
4. **A tagger is always loaded,** which removes a second code path. Its size matters if it ships on-device.
