//! A linear-chain CRF for labelling the words of geocoding queries: which
//! words are what's sought (`venue`), a house number, a road, a postcode, or
//! the neighbourhood, locality, county, region or country it's in.
//!
//! The model follows libpostal's address parser
//! (<https://github.com/openvenues/libpostal>, MIT): state features weighted
//! per label, state-transition features weighted per pair of labels, plain
//! transitions, trained by structured averaged perceptron. Its features add
//! what libpostal's can't know: which words the index's gazetteer of
//! administrative areas has, by band, how well they match (exactly, as the
//! beginning of a word still being typed, or with typos), how well known the
//! areas are and how far they are from where the map is.

pub mod crf;
pub mod dictionaries;
pub mod features;
pub mod labels;
pub mod lexicon;
pub mod model;
pub mod text;
pub mod tagger;
pub mod train;

pub use crf::{Label, Lattice, TokenFeatures, Weights};
pub use model::Model;
pub use tagger::{Interpretation, Tagger};
pub use train::{Encoded, Interner, Trainer};
