#![doc = include_str!("../README.md")]

mod error;
pub mod http_protocol;
mod interrupt;
mod result;
mod service;

pub use error::{Error, InvalidMessage, SocketRole};
pub use interrupt::{Interrupt, Interrupted};
pub use result::WorkerResult;
pub use service::{Stage, TickOutcome, ValhallaMicroservice, ValhallaMicroserviceBuilder};
