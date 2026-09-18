// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! **busbar-export-prometheus** — the built-in PULL /metrics exporter (Metrics stream), as a busbar `kind: export` sink (COLD/JSON ABI lane).
//!
//! This is the DISTRIBUTION half of the `prometheus` observability sink, promoted out of
//! busbar-core's compiled-in `export::prometheus` module into a real, both-ways export-kind plugin
//! (busbar 1.6.0 DECISIONS #3 / #30 / #11). All the sink logic lives here in this rlib; the sibling
//! `busbar-export-prometheus-plugin` cdylib is only the ABI adapter.
//!
//! ## SCAFFOLD STATUS
//!
//! This is a COMPILING SKELETON. [`open`] returns a clear `unimplemented` error and no real sink
//! logic has been copied in yet — the byte-identical extraction from busbar-core is a separate,
//! oracle-gated step (the sink's on-wire output — the /metrics exposition, the OTLP request, the
//! webhook payload, the JSONL line — is a money/behavior byte-identity surface and must move under
//! an oracle, not by hand).

use busbar_plugin_sdk::{ExportHandler, ExportStream};

/// The `prometheus` export sink. SCAFFOLD: zero-sized placeholder — the real state (config, targets,
/// recorder handle, tracer provider, …) lands with the extraction.
pub struct Sink;

impl ExportHandler for Sink {
    fn streams(&self) -> Vec<ExportStream> {
        // The stream this sink carries once the real logic is extracted.
        vec![ExportStream::Metrics]
    }
    // `deliver` / `routes` / `handle_http` keep their SDK defaults until the extraction fills them in.
}

/// Construct the `prometheus` export sink from the engine's JSON config (COLD/JSON lane).
///
/// SCAFFOLD: returns `unimplemented` — the real config parse (serde_json, like the store plugins)
/// and sink construction land with the byte-identical extraction from busbar-core.
pub fn open(cfg: &str) -> Result<Box<dyn ExportHandler>, String> {
    // Parsed here (not swallowed) so the config-shape contract is visible from day one; the parsed
    // value is intentionally unused until the extraction wires it into `Sink`.
    let _cfg: serde_json::Value = if cfg.trim().is_empty() {
        serde_json::Value::Object(Default::default())
    } else {
        serde_json::from_str(cfg).map_err(|e| format!("invalid prometheus export plugin config: {e}"))?
    };
    let _ = Sink; // keep the skeleton type referenced until `open` really constructs it.
    Err("unimplemented: busbar-export-prometheus is a scaffold; the prometheus sink logic has not yet been extracted          from busbar-core (busbar 1.6.0 DECISIONS #3, oracle-gated byte-identity move)"
        .to_string())
}

#[cfg(test)]
mod tests;
