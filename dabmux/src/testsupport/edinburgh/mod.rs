//! Independent FIC decoder used as a test oracle for the FIG writers.
//!
//! Vendored from the EDInburgh EDI receiver (`__ref/edinburgh/shared/src/dab/`,
//! GPL-2.0) with only these changes: `use` paths adapted to this module, the
//! CRC helper moved into `utils.rs`, and the FIG 0/9 fields made public.
//! Keep it otherwise verbatim so it stays an independent reading of EN 300 401.
//!
//! Known gaps: no FIG 0/7 or 0/17 decoder, FIG 0/1 skips SubChId > 30,
//! FIG 0/9 `lto` is whole hours, FIB CRC failures and undecodable FIGs are
//! dropped silently.
#![allow(dead_code, unused_imports, clippy::all)]

pub mod fic;
pub mod tables;
pub mod utils;
