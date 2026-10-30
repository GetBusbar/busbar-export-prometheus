// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE PROMETHEUS EXPORT SINK — `export.<name>.module: prometheus`.
//!
//! A PULL sink on the export kind's memory ABI. The host keeps what only the host can own: the
//! recorder every emit site writes, its scrape-time gauges and the hook-metrics cache, which it
//! lends through its kind-neutral snapshot service. This sink owns the rest: it carries the
//! `metrics` stream, validates its settings, declares `GET /metrics` and `GET /metrics/hooks` as its
//! own routes behind the data key, and its `serve` reads the host's families (`snapshot.read`) and
//! answers the exposition. A snapshot the host hands to `scrape` renders the same way into the
//! host's buffer.
//!
//! [`door::door`] is the row a busbar build links; the sibling `busbar-export-prometheus-plugin`
//! cdylib exports the same door as its one symbol.

#![forbid(unsafe_code)]

pub mod door;

use busbar_contract::abi::sdk::export::second_instances;
use busbar_contract::abi::sdk::life::settings_as;
use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
use std::fmt::Write as _;

/// One family in the Prometheus TEXT exposition format: `# HELP` (when present), `# TYPE`, the
/// `samples` as handed, and a blank line when `blank_line` (the host exposition closes each family
/// with one; the hook exposition does not). Every label value and number is written as the snapshot
/// carries it, so rendering a snapshot of the host's own exposition reproduces it byte for byte.
fn write_family<'a>(
    out: &mut String,
    f: &MetricFamily,
    samples: impl Iterator<Item = &'a MetricSample>,
    blank_line: bool,
) {
    if let Some(help) = &f.help {
        let _ = writeln!(out, "# HELP {} {help}", f.name);
    }
    let _ = writeln!(out, "# TYPE {} {}", f.name, f.kind);
    for s in samples {
        out.push_str(&s.name);
        if !s.labels.is_empty() {
            out.push('{');
            for (i, (k, v)) in s.labels.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{k}=\"{v}\"");
            }
            out.push('}');
        }
        let _ = writeln!(out, " {}", s.value);
    }
    if blank_line {
        out.push('\n');
    }
}

/// Render the host's HOOK families (`/metrics/hooks`, the export kind's `scrape` with
/// `SCRAPE_FLAG_HOOK_FAMILIES`): families by name, `# HELP` when present then `# TYPE`, each
/// family's samples in the order handed, and NO blank line between families. Every label value and
/// number is written as the host spelled it.
pub fn render_hooks(families: &[MetricFamily]) -> String {
    let mut ordered: Vec<&MetricFamily> = families.iter().collect();
    ordered.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::new();
    for f in ordered {
        write_family(&mut out, f, f.samples.iter(), false);
    }
    out
}

/// The limits check across every configured instance, in configuration order: the sink states the
/// `one_instance` mark, so each extra instance is refused, naming the first.
pub fn check_limits(instances: &[String]) -> Vec<String> {
    second_instances(
        instances,
        ALIAS,
        "Prometheus serves the ONE well-known /metrics route, so a second instance could \
         only be silently ignored — keep a single instance.",
    )
}

/// The plugin's canonical name.
pub const NAME: &str = "busbar-export-prometheus";

/// The `module:` an operator writes, the alias on either door.
pub const ALIAS: &str = "prometheus";

/// What this sink DECLARES to the host (the manifest's `declares` section, `declares.json`, which
/// `busbar-plugin-pack --declares-file` signs into the tarball): it reports no series, raises no
/// code of its own and has no destination.
pub const DECLARES: &str = include_str!("../declares.json");

/// The `settings:` an operator writes for this sink, field for field the configuration grammar's
/// shape. The VALUES are the host's to act on (retention window, scrape-time gauge bound); this
/// sink only says whether they are well formed.
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
/// Empty settings read as `{}` (the SDK's `settings_object` rule, as every export sibling reads
/// them), so they are refused for the missing `buffer_seconds`.
///
/// # Errors
/// The line, when the settings are refused.
pub fn validate(settings: &[u8]) -> Result<(), String> {
    let parsed: PrometheusSettings = settings_as(settings).map_err(|r| {
        r.text()
            .map_or_else(String::new, std::string::ToString::to_string)
    })?;
    if parsed.buffer_seconds == 0 {
        return Err(ZERO_RETENTION.to_string());
    }
    Ok(())
}

/// The Prometheus text exposition of the snapshot: `# HELP`, `# TYPE`, the samples and the
/// family-closing blank line, every label value and number exactly as the recorder wrote it — in
/// the STABLE order [`canonical_order`] gives it.
pub fn render(families: &[MetricFamily]) -> String {
    let mut out = String::new();
    for f in canonical_order(families) {
        write_family(&mut out, f, series_order(f), true);
    }
    out
}

/// A family kind's place in the stable order: counters, then gauges, then distributions
/// (histogram and summary), then anything else.
fn kind_rank(kind: &str) -> u8 {
    match kind {
        "counter" => 0,
        "gauge" => 1,
        "histogram" | "summary" => 2,
        _ => 3, // "untyped" or anything else the wire never actually carries
    }
}

/// The snapshot in a STABLE order: families grouped by KIND — every counter, then every gauge,
/// then every histogram/summary (a distribution) — and by name within a kind's group; and within a
/// family its series by their labels. The recorder hands families and series over in its maps' hash
/// order, which differs from boot to boot; the same metrics must scrape to the same bytes.
///
/// The kind grouping follows the `metrics-exporter-prometheus` renderer the host links:
/// `render_to_write` drains its counters map whole, then gauges, then distributions (histogram +
/// summary), in that fixed order. Family order within a kind is hash-map order there; sorting by
/// name is a deterministic pick among the orders it could produce.
fn canonical_order(families: &[MetricFamily]) -> Vec<&MetricFamily> {
    let mut out: Vec<&MetricFamily> = families.iter().collect();
    out.sort_by(|a, b| {
        kind_rank(&a.kind)
            .cmp(&kind_rank(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// The family's samples in a STABLE order: its series by their labels.
///
/// A SERIES is every sample sharing one label set once `le` / `quantile` are set aside — a
/// histogram's `_bucket` lines with its `_sum` and `_count`, a summary's quantiles with its `_sum`
/// and `_count`. A series is moved as a unit and its own lines keep the order the recorder wrote
/// them in (buckets ascending, then `_sum`, then `_count`), so every output is one of the orders
/// the recorder could already print.
fn series_order(family: &MetricFamily) -> impl Iterator<Item = &MetricSample> {
    let mut series: std::collections::BTreeMap<Vec<(&str, &str)>, Vec<&MetricSample>> =
        std::collections::BTreeMap::new();
    for sample in &family.samples {
        let key = sample
            .labels
            .iter()
            .filter(|(k, _)| k != "le" && k != "quantile")
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        series.entry(key).or_default().push(sample);
    }
    series.into_values().flatten()
}

#[cfg(test)]
#[path = "tests/lib_tests.rs"]
mod tests;
