//! Archive (zip / tar / tar.gz) listing. The reading lives in the `archive` plugin
//! (the `konoma-table` crate); the host keeps this path for the names it already used.

#[cfg(test)]
pub use konoma_table::archive::*;
