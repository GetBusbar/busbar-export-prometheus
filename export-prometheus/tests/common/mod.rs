// SPDX-License-Identifier: Apache-2.0
// Wire fixture interpretation stays with the exporter that owns its format.
#[path = "../../src/snapshot_reader.rs"]
mod snapshot_reader;
pub(crate) use snapshot_reader::parse_families;
