// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The sink's own statements: what it carries, where it serves, how it refuses settings, and that
//! it renders a snapshot back into the exposition it was taken from. The linked-vs-dropped-in proof
//! is `busbar-export-prometheus-plugin`'s conformance test.

use super::*;
use busbar_contract::abi::sdk::MetricSample;

/// The refusals read as the configuration's own settings errors always have — one line each,
/// `settings: <serde's words>` (the host names the instance).
#[test]
fn its_settings_refusals_are_the_configurations_words() {
    let check = |v: serde_json::Value| {
        validate(v.to_string().as_bytes())
            .err()
            .into_iter()
            .collect::<Vec<_>>()
    };
    assert!(check(serde_json::json!({ "buffer_seconds": 60 })).is_empty());
    assert!(check(serde_json::json!({ "buffer_seconds": 60, "key_gauge_limit": 5 })).is_empty());
    assert_eq!(
        check(serde_json::json!({})),
        vec!["settings: missing field `buffer_seconds`".to_string()]
    );
    assert_eq!(
        check(serde_json::json!({ "buffer_seconds": 60, "buffer": 1 })),
        vec![
            "settings: unknown field `buffer`, expected `buffer_seconds` or \
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
        vec!["settings: invalid type: string \"60\", expected u64".to_string()]
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
    let body = render(&families);
    // The counter renders BEFORE the summary even though the snapshot handed it over after —
    // v1.5.5's own renderer always drains its counters map whole before its distributions map,
    // and this sink's stable order groups by kind first (counter, gauge, histogram/summary) so
    // it never lands a counter after a summary the way an un-grouped name sort would (KP-C0).
    assert_eq!(
        body,
        "# HELP busbar_requests_total requests served\n\
         # TYPE busbar_requests_total counter\n\
         busbar_requests_total{pool=\"a\\\"b\",outcome=\"ok\"} 3\n\
         \n\
         # TYPE busbar_request_duration_seconds summary\n\
         busbar_request_duration_seconds{quantile=\"0.5\"} 0.0125\n\
         busbar_request_duration_seconds_sum 0.05\n\
         busbar_request_duration_seconds_count 4\n\
         \n"
    );
    assert_eq!(render(&[]), "", "an empty recorder renders nothing");
}

/// The sink declares its contract-ABI range and nothing else.
#[test]
fn it_declares_its_contract_abi_range_and_nothing_else() {
    let stated: serde_json::Value = serde_json::from_str(DECLARES).expect("declares.json parses");
    assert_eq!(
        stated,
        serde_json::json!({"contract_abi": {"min": 3, "max": 3}})
    );
    assert_eq!((NAME, ALIAS), ("busbar-export-prometheus", "prometheus"));
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
    // A counter whose NAME sorts after the histogram's ("z" > "l"): under a plain name sort it
    // would land after `busbar_latency_seconds`, but v1.5.5's own renderer drains its whole
    // counters map before its distributions map, so this must still render before the histogram
    // (KP-C0: kind beats name).
    let zulu = "busbar_zulu_total";
    families.insert(
        zulu.to_string(),
        vec![vec![sample(zulu, &[("pool", "alpha")], "1")]],
    );
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
    let (a, b) = (render(&one), render(&two));
    assert_eq!(
        a, b,
        "the rendered exposition depends on the recorder's hash order"
    );

    // Families by KIND then name (every counter, including `busbar_zulu_total`, before the one
    // histogram), series by labels, and each series whole in the recorder's own order. A plain
    // name sort would put `busbar_zulu_total` (a counter) after `busbar_latency_seconds` (the
    // histogram); kind grouping — v1.5.5's own renderer drains counters whole before
    // distributions — must not.
    let lines: Vec<&str> = a.lines().filter(|l| l.starts_with("# TYPE ")).collect();
    let mut expected: Vec<String> = (0..12)
        .map(|n| format!("# TYPE busbar_family_{n:02}_total counter"))
        .collect();
    expected.push("# TYPE busbar_zulu_total counter".to_string());
    expected.sort();
    expected.push("# TYPE busbar_latency_seconds histogram".to_string());
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    assert_eq!(lines, expected, "families are not kind-then-name ordered");
    assert!(a.starts_with(
        "# TYPE busbar_family_00_total counter\n\
         busbar_family_00_total{pool=\"alpha\"} 1\n\
         busbar_family_00_total{pool=\"bravo\"} 1\n"
    ));
    assert!(a.contains("# TYPE busbar_zulu_total counter\nbusbar_zulu_total{pool=\"alpha\"} 1\n"));
    let zulu_pos = a.find("# TYPE busbar_zulu_total").unwrap();
    let hist_pos = a.find("# TYPE busbar_latency_seconds").unwrap();
    assert!(
        zulu_pos < hist_pos,
        "a counter must render before the histogram even though its name sorts after it"
    );
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

/// A gauge renders after every counter and before every histogram, whatever the names sort to
/// (v1.5.5 drains counters, then gauges, then distributions). Fed in reverse order, with a gauge
/// whose name sorts after the histogram's, so a rank that moved the gauge would show.
#[test]
fn a_gauge_renders_between_the_counters_and_the_histograms_whatever_its_name() {
    let fam = |name: &str, kind: &str, sample_name: &str, le: Option<&str>| MetricFamily {
        name: name.to_string(),
        kind: kind.to_string(),
        help: None,
        samples: vec![MetricSample {
            name: sample_name.to_string(),
            labels: le
                .map(|v| vec![("le".to_string(), v.to_string())])
                .unwrap_or_default(),
            value: "1".to_string(),
        }],
    };
    let families = vec![
        fam(
            "busbar_aa_seconds",
            "histogram",
            "busbar_aa_seconds_bucket",
            Some("+Inf"),
        ),
        fam("busbar_zz_gauge", "gauge", "busbar_zz_gauge", None),
        fam("busbar_m_total", "counter", "busbar_m_total", None),
    ];
    let out = render(&families);
    let types: Vec<&str> = out.lines().filter(|l| l.starts_with("# TYPE ")).collect();
    assert_eq!(
        types,
        [
            "# TYPE busbar_m_total counter",
            "# TYPE busbar_zz_gauge gauge",
            "# TYPE busbar_aa_seconds histogram",
        ]
    );
}
