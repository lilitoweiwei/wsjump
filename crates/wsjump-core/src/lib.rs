//! wsjump-core: all business logic shared by the CLI and TUI front-ends.

pub mod config;
pub mod create;
pub mod db;
pub mod error;
pub mod filter;
pub mod index;
pub mod meta;
pub mod progress;
pub mod query;
pub mod scan;
pub mod signature;
pub mod workspace;

#[cfg(test)]
mod test_support;
