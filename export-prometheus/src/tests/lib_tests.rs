// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The sink's own statements: what it carries, where it serves, how it refuses settings, and that
//! it renders a snapshot back into the exposition it was taken from. The linked-vs-dropped-in proof
//! is the composition root's (`root::linked::tests`), where the linked tables live.

use super::*;
use busbar_contract::abi::sdk::MetricSample;

fn sink() -> Box<dyn ExportHandler> {
    open("{}").expect("the sink opens")
}

#[test]
fn it_carries_the_metrics_stream_and_declares_no_route_of_its_own() {
    let sink = sink();
    assert_eq!(sink.streams(), vec![ExportStream::Metrics]);
    assert!(sink.routes().is_empty(), "the host serves /metrics");
}

/// The refusals read as the configuration's own settings errors always have — one line each,
/// `export.<instance>.settings: <serde's words>`.
#[test]
fn its_settings_refusals_are_the_configurations_words() {
    let sink = sink();
    let check = |v: serde_json::Value| sink.validate("m", &v);
    assert!(check(serde_json::json!({ "buffer_seconds": 60 })).is_empty());
    assert!(check(serde_json::json!({ "buffer_seconds": 60, "key_gauge_limit": 5 })).is_empty());
    assert_eq!(
        check(serde_json::json!({})),
        vec!["export.m.settings: missing field `buffer_seconds`".to_string()]
    );
    assert_eq!(
        check(serde_json::json!({ "buffer_seconds": 60, "buffer": 1 })),
        vec![
            "export.m.settings: unknown field `buffer`, expected `buffer_seconds` or \
             `key_gauge_limit`"
                .to_string()
        ]
    );
    assert_eq!(
        check(serde_json::json!({ "buffer_seconds": 0 })),
        vec![
            "the `module: prometheus` export instance sets settings.buffer_seconds: 0, which \
             retains no observations — every scrape would report empty quantiles while still paying \
             the recording cost. Name a positive retention window in seconds, or remove the \
             instance to turn metrics off"
                .to_string()
        ]
    );
    assert_eq!(
        check(serde_json::json!({ "buffer_seconds": "60" })),
        vec!["export.m.settings: invalid type: string \"60\", expected u64".to_string()]
    );
}

/// Every family type, in the stable order — HELP optional, labels escaped as carried, numbers as spelled —
/// renders back to the exposition the snapshot was read from, byte for byte.
#[test]
fn it_renders_the_snapshot_back_into_the_exposition() {
    let sample = |name: &str, labels: &[(&str, &str)], value: &str| MetricSample {
        name: name.to_string(),
        labels: labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        value: value.to_string(),
    };
    let families = vec![
        MetricFamily {
            name: "busbar_request_duration_seconds".to_string(),
            kind: "summary".to_string(),
            help: None,
            samples: vec![
                sample(
                    "busbar_request_duration_seconds",
                    &[("quantile", "0.5")],
                    "0.0125",
                ),
                sample("busbar_request_duration_seconds_sum", &[], "0.05"),
                sample("busbar_request_duration_seconds_count", &[], "4"),
            ],
        },
        MetricFamily {
            name: "busbar_requests_total".to_string(),
            kind: "counter".to_string(),
            help: Some("requests served".to_string()),
            samples: vec![sample(
                "busbar_requests_total",
                &[("pool", "a\\\"b"), ("outcome", "ok")],
                "3",
            )],
        },
    ];
    let (content_type, body) = sink().render(&families);
    assert_eq!(content_type, "text/plain; version=0.0.4");
    assert_eq!(
        body,
        "# TYPE busbar_request_duration_seconds summary\n\
         busbar_request_duration_seconds{quantile=\"0.5\"} 0.0125\n\
         busbar_request_duration_seconds_sum 0.05\n\
         busbar_request_duration_seconds_count 4\n\
         \n\
         # HELP busbar_requests_total requests served\n\
         # TYPE busbar_requests_total counter\n\
         busbar_requests_total{pool=\"a\\\"b\",outcome=\"ok\"} 3\n\
         \n"
    );
    assert_eq!(
        sink().render(&[]).1,
        "",
        "an empty recorder renders nothing"
    );
}

#[test]
fn the_linked_entry_states_the_row_and_this_crates_boundary() {
    let (name, alias, declares, entry) = linked::EXPORT;
    assert_eq!(
        (name, alias, declares),
        ("busbar-export-prometheus", "prometheus", DECLARES)
    );
    let stated: serde_json::Value = serde_json::from_str(declares).expect("declares.json parses");
    assert_eq!(
        stated,
        serde_json::json!({"contract_abi": {"min": 3, "max": 3}}),
        "the sink declares its contract-ABI range and nothing else"
    );
    assert!(std::ptr::eq(entry, &BUSBAR_COLD_ENTRY));
}

/// A `BuildHasher` whose seed is the test's to choose, so "a different hash seed" is a fact of the
/// test and not a coin the process flips: FNV-1a over the bytes, the seed folded into the basis.
#[derive(Clone, Copy)]
struct Seeded(u64);

struct SeededHasher(u64);

impl std::hash::Hasher for SeededHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

impl std::hash::BuildHasher for Seeded {
    type Hasher = SeededHasher;
    fn build_hasher(&self) -> SeededHasher {
        SeededHasher(0xcbf2_9ce4_8422_2325 ^ self.0.wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }
}

/// The recorder's snapshot as a hash-ordered recorder hands it over under `seed`: families in its
/// map's order, and each family's series in its map's order — a histogram series (buckets
/// ascending, `_sum`, `_count`) and a summary series each staying whole, as the recorder writes
/// them.
fn snapshot_under(seed: u64) -> Vec<MetricFamily> {
    let sample = |name: &str, labels: &[(&str, &str)], value: &str| MetricSample {
        name: name.to_string(),
        labels: labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        value: value.to_string(),
    };
    let pools = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
    ];
    let mut families: std::collections::HashMap<String, Vec<Vec<MetricSample>>, Seeded> =
        std::collections::HashMap::with_hasher(Seeded(seed));
    for i in 0..12 {
        let name = format!("busbar_family_{i:02}_total");
        let mut series: std::collections::HashMap<&str, Vec<MetricSample>, Seeded> =
            std::collections::HashMap::with_hasher(Seeded(seed ^ i));
        for pool in pools {
            series.insert(pool, vec![sample(&name, &[("pool", pool)], "1")]);
        }
        families.insert(name, series.into_values().collect());
    }
    let hist = "busbar_latency_seconds";
    let mut series: std::collections::HashMap<&str, Vec<MetricSample>, Seeded> =
        std::collections::HashMap::with_hasher(Seeded(seed.rotate_left(7)));
    for pool in pools {
        series.insert(
            pool,
            vec![
                sample(
                    &format!("{hist}_bucket"),
                    &[("pool", pool), ("le", "0.5")],
                    "1",
                ),
                sample(
                    &format!("{hist}_bucket"),
                    &[("pool", pool), ("le", "1")],
                    "2",
                ),
                sample(
                    &format!("{hist}_bucket"),
                    &[("pool", pool), ("le", "+Inf")],
                    "3",
                ),
                sample(&format!("{hist}_sum"), &[("pool", pool)], "1.75"),
                sample(&format!("{hist}_count"), &[("pool", pool)], "3"),
            ],
        );
    }
    families.insert(hist.to_string(), series.into_values().collect());
    families
        .into_iter()
        .map(|(name, series)| MetricFamily {
            kind: if name == hist { "histogram" } else { "counter" }.to_string(),
            help: None,
            samples: series.into_iter().flatten().collect(),
            name,
        })
        .collect()
}

/// THE SAME METRICS SCRAPE TO THE SAME BYTES: the one snapshot handed over in two hash orders
/// renders identically. The RED arm is the unsorted render, which differs between the two — so
/// the seeds really did reorder the input, and the equality is the sink's doing.
#[test]
fn the_same_snapshot_under_two_hash_seeds_renders_identical_bytes() {
    let (one, two) = (snapshot_under(1), snapshot_under(2));
    assert_ne!(
        render_exposition(&one),
        render_exposition(&two),
        "RED arm: the two seeds must hand the snapshot over in different orders"
    );
    let (a, b) = (sink().render(&one).1, sink().render(&two).1);
    assert_eq!(
        a, b,
        "the rendered exposition depends on the recorder's hash order"
    );

    // Families by name, series by labels, and each series whole in the recorder's own order.
    let lines: Vec<&str> = a.lines().filter(|l| l.starts_with("# TYPE ")).collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted, "families are not in name order");
    assert!(a.starts_with(
        "# TYPE busbar_family_00_total counter\n\
         busbar_family_00_total{pool=\"alpha\"} 1\n\
         busbar_family_00_total{pool=\"bravo\"} 1\n"
    ));
    assert!(a.contains(
        "# TYPE busbar_latency_seconds histogram\n\
         busbar_latency_seconds_bucket{pool=\"alpha\",le=\"0.5\"} 1\n\
         busbar_latency_seconds_bucket{pool=\"alpha\",le=\"1\"} 2\n\
         busbar_latency_seconds_bucket{pool=\"alpha\",le=\"+Inf\"} 3\n\
         busbar_latency_seconds_sum{pool=\"alpha\"} 1.75\n\
         busbar_latency_seconds_count{pool=\"alpha\"} 3\n\
         busbar_latency_seconds_bucket{pool=\"bravo\",le=\"0.5\"} 1\n"
    ));
    assert_eq!(a.lines().count(), render_exposition(&one).lines().count());
}
