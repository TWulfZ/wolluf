//! Library: the `IndexLibrary` job that parses and labels the catalog's charts, and the queries
//! over the result (F1, architecture §5.4).

pub mod dto;
mod index;
mod service;
#[cfg(test)]
pub(crate) mod testkit;

pub use index::IndexLibraryJob;
pub use service::{ChartRows, LibraryService};
