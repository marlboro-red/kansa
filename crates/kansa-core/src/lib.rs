//! kansa-core — state store, objects, segmentation, reconciliation, export.
//! The CLI and the desktop app are thin skins over this crate (`ui~core-parity~1`).

pub mod api;
pub mod coverage;
pub mod export;
pub mod id;
pub mod model;
pub mod ops;
pub mod reconcile;
pub mod repo;
pub mod segment;
pub mod snapshot;
pub mod store;

pub use anyhow::{Error, Result};
