//! A Rust port of the [Pelias parser](https://github.com/pelias/parser)
//! (MIT), a dictionary- and rule-based classifier for geocoding queries.
//!
//! Given `"123 main st springfield il"` it proposes ranked interpretations
//! such as `housenumber: 123, street: main st, locality: springfield,
//! region: il`. It is meant as a **soft signal** for ranking: it knows
//! addresses and admin names but not categories, so for `"vegan food"` it
//! says little or nothing, and its output should never filter results.
//!
//! # Usage
//!
//! ```
//! use airmail_parser::Kind;
//!
//! let parse = airmail_parser::parse("123 main st springfield il");
//! let best = parse.best().unwrap();
//! assert_eq!(best.spans[0].kind, Kind::HouseNumber);
//! assert_eq!(best.spans[1].text, "main st");
//!
//! // soft per-token signal, aggregated over all solutions
//! for tw in parse.token_weights() {
//!     println!("{} admin={:.2} street={:.2}", tw.token.text, tw.weights.admin(), tw.weights.get(Kind::Street));
//! }
//! ```
//!
//! # Pipeline
//!
//! 1. **Tokenization**: sections split at commas, tabs, newlines and
//!    quotes, words at whitespace (and again at hyphens and slashes), phrases
//!    are runs of up to ten adjacent words.
//! 2. **Classification**: ~30 classifiers label words and phrases, from
//!    dictionaries (libpostal street types, stop words, given names, …;
//!    Who's On First locality/region/country names; chromium-i18n postcode
//!    patterns) and patterns (house numbers, ordinals), then composite
//!    schemes combine adjacent labels (`{alpha} {street_suffix}` → street,
//!    `{place} {stop_word} {area}` → venue, …).
//! 3. **Solving**: every combination of at most one span per public label
//!    that doesn't overlap is a candidate solution, scored by the
//!    confidence-weighted fraction of the query it covers; filters then drop
//!    implausible orders (a postcode before a street, a country before a
//!    locality, …) and penalties apply. The top ten are kept.
//!
//! # Faithfulness
//!
//! The port reproduces upstream's behaviour, including its quirks (operator
//! precedence in the ordinal regex, lodash path semantics when adding
//! plurals, the in-place phrase sort that changes later iteration order,
//! upstream's solution caps). Offsets are computed in UTF-16 units
//! internally, as in JavaScript, so scores match. All 367 cases of
//! upstream's functional test suite pass. Differences:
//!
//! - Ties between equally-scored solutions: upstream's comparator is not a
//!   consistent ordering when both solutions start with the same admin kind,
//!   so with V8's sort the winner flips on each of the ~30 sorts per parse.
//!   Here the tie is broken by the next admin span (locality before region
//!   before country), which is what upstream's tests expect (see `compare`
//!   in `parser.rs`).
//! - The npm packages `remove-accents` and `pluralize` are reimplemented
//!   (accent folding by a Latin character table, `resources/accents.tsv`;
//!   pluralize's rule list), so a few rare names may fold or pluralize
//!   differently.
//! - Postcode patterns use Rust's `regex`, which lacks lookahead; the single
//!   pattern that uses it (Netherlands) is handled explicitly.
//! - Upstream's unit classifiers exist but are not enabled in its
//!   `AddressParser`; here they are opt-in via
//!   [`ParserOptions::unit_classifiers`]. The unused `AdjacencyClassifier`,
//!   the HTTP server, CLI and debug output builder are not ported.
//!
//! # Cost
//!
//! The dictionaries (4.6 MB of text, 85% Who's On First locality names) are
//! compiled into the binary with `include_str!`. Building the parser hashes
//! them into sets (borrowing the embedded text where normalization leaves
//! it unchanged): about 100 ms and 26 MiB of resident memory in a release
//! build. [`parser()`] does this once, lazily; call it at startup to avoid a
//! slow first query. Parsing takes 5–50 µs for typical queries and up to
//! ~0.6 ms for long multi-part addresses (release build).
//!
//! # Using it in a ranker
//!
//! The parser's knowledge is narrow, so use it to *adjust weights*, never to
//! decide what matches:
//!
//! - **Admin field weight per token.** For each query token, take
//!   [`Parse::weights`] over its byte range. If `weights.admin()` is low and
//!   the token is classified as part of a venue or street (or is simply
//!   unclassified while other tokens are confidently admin), lower that
//!   token's weight in admin/containing-place fields; if it is high, raise
//!   it, and lower its weight in the name field. This addresses
//!   "springfield"/"coffee" matching counties.
//! - **Address structure.** A confident `housenumber` + `street` pair
//!   suggests boosting address documents whose house number and street both
//!   match, and treating the remaining admin-like tokens as location.
//! - **Unclassified tokens are the subject.** In `"starbucks seattle"` only
//!   `seattle` is classified (locality); the unclassified words are what the
//!   user is looking for and should be matched against names and
//!   categories, with little weight in admin fields.
//! - **Treat venue/street labels on generic words with suspicion.** The
//!   parser has place keywords (`coffee`, `park`, `hospital`) and generous
//!   street schemes, so `"coffee near capitol hill"` comes out as
//!   `venue: coffee near` + `locality: capitol hill` (or as one street).
//!   That is still useful (it says `coffee near` is not an admin), but it is
//!   not evidence that a venue named "coffee near" exists. A good rule of
//!   thumb: use the parse to *demote* admin interpretations of words it
//!   labels as non-admin, and to *promote* admin interpretations of words it
//!   confidently labels as admin, rather than the other way round.
//! - **Ambiguity is information.** `"il"` gets region 0.5 / country 0.5;
//!   don't collapse it to the best solution, use the weights.
//! - **Features.** [`Parse::features`] exposes raw dictionary hits (e.g. a
//!   phrase is a known locality name even if no solution used it), useful as
//!   ranking features in their own right.

mod classification;
mod classifier;
mod js;
mod parse;
mod parser;
mod resources;
mod solver;
mod text;
mod tokenization;

use std::sync::LazyLock;

pub use classification::Kind;
pub use parse::{
    ClassifiedSpan, Feature, Parse, Solution, TextRange, Token, TokenWeights, Weights,
};
pub use parser::{AddressParser, ParserOptions};

static PARSER: LazyLock<AddressParser> = LazyLock::new(AddressParser::new);

/// A shared parser with upstream's default configuration, built on first
/// use.
pub fn parser() -> &'static AddressParser {
    &PARSER
}

/// Parses a query with the shared parser.
pub fn parse(input: &str) -> Parse {
    parser().parse(input)
}
