// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE PROMETHEUS EXPORT SINK — `export.<name>.module: prometheus`.
//!
//! A PULL sink on the export kind's memory ABI. The host keeps what only the host can own: the
//! recorder every emit site writes, its scrape-time gauges, the well-known `/metrics` route it
//! serves and that route's content type. This sink owns what an exposition IS: it carries the
//! `metrics` stream (the instance subscribed to it is the one the host's scrape asks to render), it
//! validates the settings an operator writes for it, and on every scrape the host hands it the
//! recorder's WHOLE snapshot (`ScrapeIn::families`, in the recorder's order) and it renders the
//! text into the host's buffer.
//!
//! One door, both ways in: [`door::door`] is the row a busbar build links, and the sibling
//! `busbar-export-prometheus-plugin` cdylib exports the same door as its one symbol.

#![deny(unsafe_code)]

pub mod door;

use busbar_contract::abi::sdk::{MetricFamily, MetricSample};

/// Render the host recorder's snapshot (export ABI minor 6) in the Prometheus TEXT exposition
/// format, family by family: `# HELP` (when present), `# TYPE`, the samples, a blank line. Every
/// label value and number is written as the snapshot carries it, so rendering a snapshot of the
/// host's own exposition reproduces it byte for byte.
fn render_exposition(families: &[MetricFamily]) -> String {
    let mut out = String::new();
    for f in families {
        if let Some(help) = &f.help {
            out.push_str(&format!("# HELP {} {help}\n", f.name));
        }
        out.push_str(&format!("# TYPE {} {}\n", f.name, f.kind));
        for s in &f.samples {
            out.push_str(&s.name);
            if !s.labels.is_empty() {
                let labels: Vec<String> = s
                    .labels
                    .iter()
                    .map(|(k, v)| format!("{k}=\"{v}\""))
                    .collect();
                out.push_str(&format!("{{{}}}", labels.join(",")));
            }
            out.push_str(&format!(" {}\n", s.value));
        }
        out.push('\n');
    }
    out
}

/// The row's canonical name.
pub const NAME: &str = "busbar-export-prometheus";

/// The `module:` an operator writes — the name 1.5.5 spelled the built-in by — the row's alias on
/// either door.
pub const ALIAS: &str = "prometheus";

/// What this sink DECLARES to the host (the manifest's `declares` section, `declares.json`, which
/// `busbar-plugin-pack --declares-file` signs into the tarball): the contract-ABI range it speaks
/// (`contract_abi`), and nothing else. It reports no series, raises no code of its own and has no
/// destination.
pub const DECLARES: &str = include_str!("../declares.json");

/// The `settings:` an operator writes for this sink — the same shape, field for field, the
/// configuration grammar has frozen for it since 1.5.3, so a refusal reads exactly as it always has.
/// The VALUES are the host's to act on (the recorder's retention window, the scrape-time gauge
/// bound); this sink only says whether they are well formed.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PrometheusSettings {
    /// REQUIRED: the retention window of the rolling quantile summary, in seconds — positive.
    buffer_seconds: u64,
    /// The bound on per-key scrape-time gauges.
    #[serde(default)]
    #[allow(dead_code)]
    key_gauge_limit: usize,
}

/// The zero-retention refusal, word for word as the configuration has always reported it.
const ZERO_RETENTION: &str =
    "the `module: prometheus` export instance sets settings.buffer_seconds: 0, \
     which retains no observations — every scrape would report empty quantiles while still paying \
     the recording cost. Name a positive retention window in seconds, or remove the instance to \
     turn metrics off";

/// The settings refusal, as one complete line: a malformed bag is `settings: <serde's words>`; a
/// zero retention window asks the recorder to keep nothing while still paying to record it, which
/// is refused rather than served inert (omitting the instance is how collection is turned off).
///
/// # Errors
/// The line, when the settings are refused.
pub fn validate(settings: &[u8]) -> Result<(), String> {
    let parsed = serde_json::from_slice(settings)
        .and_then(serde_json::from_value::<PrometheusSettings>)
        .map_err(|e| format!("settings: {e}"))?;
    if parsed.buffer_seconds == 0 {
        return Err(ZERO_RETENTION.to_string());
    }
    Ok(())
}

/// The Prometheus text exposition of the snapshot: `# HELP`, `# TYPE`, the samples and the
/// family-closing blank line, every label value and number exactly as the recorder wrote it — in
/// the STABLE order [`canonical_order`] gives it.
pub fn render(families: &[MetricFamily]) -> String {
    render_exposition(&canonical_order(families))
}

/// The snapshot in a STABLE order: families grouped by KIND — every counter, then every gauge,
/// then every histogram/summary (a distribution) — and by name within a kind's group; and within a
/// family its series by their labels. The recorder hands families and series over in its maps' hash
/// order, which differs from boot to boot; the same metrics must scrape to the same bytes.
///
/// THE KIND GROUPING is not this sink's invention: it is v1.5.5's own wire behaviour, forced by the
/// `metrics-exporter-prometheus` renderer both v1.5.5 and this host still link — `render_to_write`
/// drains its counters map whole, then its gauges map whole, then its distributions (histogram +
/// summary) map whole, three separate passes in that fixed order, every release. A v1.5.5 capture's
/// FAMILY ORDER WITHIN a kind is not code-determined (hash-map order, "varies between runs" — a
/// prior capture cannot pin it), but WHICH KIND'S BLOCK COMES FIRST is code-determined and never
/// varies. Sorting by name within a kind is a legitimate, deterministic pick from among the orders
/// v1.5.5's own hash-random renderer could already have produced.
///
/// A SERIES is every sample sharing one label set once `le` / `quantile` are set aside — a
/// histogram's `_bucket` lines with its `_sum` and `_count`, a summary's quantiles with its `_sum`
/// and `_count`. A series is moved as a unit and its own lines keep the order the recorder wrote
/// them in (buckets ascending, then `_sum`, then `_count`), so every output is one of the orders
/// the recorder could already print.
fn kind_rank(kind: &str) -> u8 {
    match kind {
        "counter" => 0,
        "gauge" => 1,
        "histogram" | "summary" => 2,
        _ => 3, // "untyped" or anything else the wire never actually carries
    }
}

fn canonical_order(families: &[MetricFamily]) -> Vec<MetricFamily> {
    let mut out = families.to_vec();
    out.sort_by(|a, b| {
        kind_rank(&a.kind)
            .cmp(&kind_rank(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    for family in &mut out {
        let mut series: std::collections::BTreeMap<Vec<(String, String)>, Vec<MetricSample>> =
            std::collections::BTreeMap::new();
        for sample in std::mem::take(&mut family.samples) {
            let key = sample
                .labels
                .iter()
                .filter(|(k, _)| k != "le" && k != "quantile")
                .cloned()
                .collect();
            series.entry(key).or_default().push(sample);
        }
        family.samples = series.into_values().flatten().collect();
    }
    out
}

#[cfg(test)]
#[path = "tests/lib_tests.rs"]
mod tests;
