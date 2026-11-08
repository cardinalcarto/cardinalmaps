//! Valhalla-compatible location correlation: mainline's Loki.
//!
//! Loki takes a request whose `options` the front has parsed and finds where each of its
//! locations is on the graph: the edges near it (with the fraction along each,
//! the side of the street, and how far the graph reaches from it),
//! after checking the request against the service's limits.
//! See `docs/routing.md` for where it sits in the stage chain.
//!
//! - [`Loki`] holds the configuration ([`LokiConfig`], the Loki part of mainline's
//!   `valhalla.json`), the tiles, and the tile set's [`ConnectivityMap`].
//!   [`Loki::handle`] runs a request's action; [`Loki::route`], [`Loki::locate`],
//!   [`Loki::matrix`], and [`Loki::trace`] are the actions, each a stage
//!   (`fn(&mut Api) -> Result<_, ValhallaError>`).
//! - [`handle_http_request`] is the entry point of the worker that mainline's HTTP server
//!   talks to: it parses the raw HTTP request with the front, then runs Loki.
//!   [`handle_request`] does the same for a request an HTTP server has already split
//!   (the in-process router's entry point).
//! - [`search`] is the location search itself (mainline's `loki::Search`),
//!   for locations that already have Loki's defaults ([`parse_location`]).
//!
//! # What Thor gets
//!
//! Each location of the request (`options.locations`, or `sources` and `targets` for a matrix)
//! gets a `correlation` with its candidate `edges` and `filtered_edges` (the ones its heading,
//! preferred side, or layer ruled out), as `PathEdge`s: the graph ID, `percent_along` in the
//! edge's direction, the correlated point `ll`, its `distance` from the location,
//! `side_of_street`, `begin_node` and `end_node` (for a location at a node),
//! `outbound_reach` and `inbound_reach`, the edge's `heading` there, its `names`,
//! and its `bounding_circle`.
//! Exclude locations and polygons become `exclude_edges` in the costing's options.
//!
//! Results match mainline 3.9.1's, as checked against its own Loki worker
//! (see `tools/oracle/README.md`).

extern crate alloc;

mod config;
mod connectivity;
mod geo;
mod graph;
mod http;
mod polygon;
mod reach;
pub mod search;
mod tiles;
mod worker;

#[cfg(test)]
mod oracle_tests;

pub use config::{
    ConfigError, CostingLimits, LokiConfig, ServiceDefaults, ServiceLimits, TraceLimits,
};
pub use connectivity::ConnectivityMap;
pub use graph::GraphError;
pub use http::{HttpOutcome, HttpParseError, RawHttpRequest, handle_http_request, handle_request};
pub use search::{SearchError, search};
pub use worker::{Loki, LokiOutput, parse_location};
