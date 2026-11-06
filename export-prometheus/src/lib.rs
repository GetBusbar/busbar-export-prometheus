// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE PROMETHEUS EXPORT SINK — `export.<name>.module: prometheus`.

#![forbid(unsafe_code)]

pub mod door;

use busbar_contract::abi::sdk::export::second_instances;
use busbar_contract::abi::sdk::life::settings_as;
use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
use std::fmt::Write as _;
use zeroize::Zeroizing;

/// Reserve the complete rendering before copying any private label. String growth must never
/// release a prior unwiped allocation. All additions are checked before an output owner exists.
fn render_buffer(families: &[MetricFamily], blank_line: bool) -> Zeroizing<String> {
    let mut bytes = 0usize;
    let mut add = |n: usize| {
        bytes = bytes
            .checked_add(n)
            .expect("metric rendering size overflow")
    };
    for f in families {
        add(9);
        add(f.name.len());
        add(f.kind.len());
        if let Some(help) = &f.help {
            add(9);
            add(f.name.len());
            add(help.len());
        }
        for s in &f.samples {
            add(s.name.len());
            add(2);
            add(s.value.len());
            if !s.labels.is_empty() {
                add(2);
                add(s.labels.len() - 1);
                for (k, v) in &s.labels {
                    add(k.len());
                    add(v.len());
                    add(3);
                }
            }
        }
        if blank_line {
            add(1);
        }
    }
    Zeroizing::new(String::with_capacity(bytes))
}

/// One family in the Prometheus TEXT exposition format: `# HELP` (when present), `# TYPE`, the
/// `samples` as handed, and a blank line when `blank_line` (the host exposition closes each family
/// with one; the hook exposition does not). Every label value and number is written as the snapshot
/// carries it.
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
    let mut out = render_hooks_owned(families);
    std::mem::take(&mut *out)
}

pub(crate) fn render_hooks_owned(families: &[MetricFamily]) -> Zeroizing<String> {
    let mut ordered: Vec<&MetricFamily> = families.iter().collect();
    ordered.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = render_buffer(families, false);
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
/// `busbar-plugin-pack --declares-file` signs into the tarball).
pub const DECLARES: &str = include_str!("../declares.json");

/// The operator settings schema published in the plugin's signed Statement.
pub const SETTINGS_SCHEMA: &str = include_str!("../settings.schema.json");

/// The `settings:` an operator writes for this sink, field for field the configuration grammar's
/// shape. The VALUES are the host's to act on (retention window, scrape-time gauge bound); this
/// sink only says whether they are well formed.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PrometheusSettings {
    /// REQUIRED: the retention window of the rolling quantile summary, in seconds — positive.
    buffer_seconds: u64,
    /// The bound on per-key scrape-time gauges: validated, never read here (the host applies it,
    /// 2000 when omitted).
    #[serde(default)]
    #[allow(dead_code)]
    key_gauge_limit: u64,
}

/// The zero-retention refusal, word for word as the configuration has always reported it.
const ZERO_RETENTION: &str =
    "the `module: prometheus` export instance sets settings.buffer_seconds: 0, \
     which retains no observations — every scrape would report empty quantiles while still paying \
     the recording cost. Name a positive retention window in seconds, or remove the instance to \
     turn metrics off";

/// The settings refusal, as one complete line: a malformed bag is `settings: <serde's words>`; a
/// zero retention window is refused rather than served inert (omitting the instance is how
/// collection is turned off).
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
/// family-closing blank line, every label value and number exactly as the recorder wrote it, in a
/// stable order: families by kind (counter, gauge, histogram and summary, then any other) and name,
/// and each family's series by their labels.
pub fn render(families: &[MetricFamily]) -> String {
    let mut out = render_owned(families);
    std::mem::take(&mut *out)
}

pub(crate) fn render_owned(families: &[MetricFamily]) -> Zeroizing<String> {
    let mut out = render_buffer(families, true);
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
        _ => 3,
    }
}

/// The snapshot in a STABLE order: families grouped by KIND — every counter, then every gauge,
/// then every histogram/summary (a distribution) — and by name within a kind's group.
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
fn series_order(family: &MetricFamily) -> impl Iterator<Item = &MetricSample> {
    let mut series: std::collections::BTreeMap<Vec<(&str, &str)>, Vec<&MetricSample>> =
        std::collections::BTreeMap::new();
    // Only the kind that owns a label sets it aside: other kinds (and a user's own `quantile` on a
    // histogram, `le` on a summary) key on the full label set.
    let aside = match family.kind.as_str() {
        "histogram" => Some("le"),
        "summary" => Some("quantile"),
        _ => None,
    };
    for sample in &family.samples {
        let key = sample
            .labels
            .iter()
            .filter(|(k, _)| Some(k.as_str()) != aside)
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        series.entry(key).or_default().push(sample);
    }
    series.into_values().flatten()
}

#[cfg(test)]
#[path = "tests/lib_tests.rs"]
mod tests;
