// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The sink's own statements: what it carries, where it serves, how it refuses settings, and that
//! it renders a snapshot back into the exposition it was taken from. The linked-vs-dropped-in proof
//! is `busbar-export-prometheus-plugin`'s conformance test.

#[path = "../../../testing/settings/integer_token.rs"]
mod settings_profile;

use super::*;

/// Renders the snapshot in its supplied order without sorting.
fn render_exposition(families: &[MetricFamily]) -> String {
    let mut out = String::new();
    for f in families {
        write_family(&mut out, f, f.samples.iter(), true);
    }
    out
}
use busbar_contract::abi::sdk::MetricSample;

#[test]
fn private_renderings_reserve_the_whole_exposition_before_copying_labels() {
    let families = [MetricFamily {
        name: "private_total".into(),
        kind: "counter".into(),
        help: Some("private HELP".repeat(32)),
        samples: vec![MetricSample {
            name: "private_total".into(),
            labels: vec![
                ("owner".into(), "private-label".repeat(128)),
                ("zone".into(), "a".into()),
            ],
            value: "1".into(),
        }],
    }];
    for hooks in [false, true] {
        let reserved = render_buffer(&families, !hooks).capacity();
        let rendered = if hooks {
            render_hooks_owned(&families)
        } else {
            render_owned(&families)
        };
        assert!(
            reserved >= rendered.len(),
            "no private prefix may be reallocated"
        );
        assert_eq!(rendered.capacity(), reserved);
        assert_eq!(
            &*rendered,
            &if hooks {
                render_hooks(&families)
            } else {
                render(&families)
            }
        );
    }
}

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

/// The sink declares no series, no code and no destination.
#[test]
fn it_declares_no_series_code_or_destination() {
    let stated: serde_json::Value = serde_json::from_str(DECLARES).expect("declares.json parses");
    for key in ["metrics", "diagnostics", "destinations"] {
        assert_eq!(stated.get(key), None, "declares states `{key}`");
    }
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
            "busbar_first_untyped",
            "untyped",
            "busbar_first_untyped",
            None,
        ),
        fam("busbar_zz_seconds", "summary", "busbar_zz_seconds", None),
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
            "# TYPE busbar_zz_seconds summary",
            "# TYPE busbar_first_untyped untyped",
        ]
    );
}

#[test]
fn distribution_label_names_remain_ordinary_labels_on_other_families() {
    for (kind, label) in [
        ("counter", "le"),
        ("gauge", "quantile"),
        ("untyped", "le"),
        ("histogram", "quantile"),
        ("summary", "le"),
    ] {
        let mut family = MetricFamily {
            name: "m".into(),
            kind: kind.into(),
            help: None,
            samples: ["z", "a"]
                .into_iter()
                .map(|value| MetricSample {
                    name: "m".into(),
                    labels: vec![(label.into(), value.into())],
                    value: "1".into(),
                })
                .collect(),
        };
        let first = render(std::slice::from_ref(&family));
        family.samples.reverse();
        assert_eq!(
            first,
            render(&[family]),
            "{kind}: {label} identifies distinct series"
        );
        assert!(first.find("=\"a\"").unwrap() < first.find("=\"z\"").unwrap());
    }
}

/// THE HOOK LAYOUT (`/metrics/hooks`, 1.5.5's bytes): families by name whatever order they come
/// in, `# HELP` before `# TYPE`, samples in the order handed (not re-sorted by label), and no blank
/// line between families — where `render` would group by kind and close each family with one.
#[test]
fn hook_families_render_in_the_1_5_5_hook_layout() {
    let sample = |name: &str, hook: &str, extra: Option<(&str, &str)>, value: &str| MetricSample {
        name: name.to_string(),
        labels: std::iter::once(("hook".to_string(), hook.to_string()))
            .chain(extra.map(|(k, v)| (k.to_string(), v.to_string())))
            .collect(),
        value: value.to_string(),
    };
    let families = vec![
        MetricFamily {
            name: "zeta_total".to_string(),
            kind: "counter".to_string(),
            help: Some("tokens \\\\ saved".to_string()),
            samples: vec![
                sample("zeta_total", "b", None, "2"),
                sample("zeta_total", "a", None, "1"),
            ],
        },
        MetricFamily {
            name: "alpha_level".to_string(),
            kind: "gauge".to_string(),
            help: None,
            samples: vec![sample("alpha_level", "a", Some(("pool", "chat")), "0.5")],
        },
    ];
    assert_eq!(
        render_hooks(&families),
        "# TYPE alpha_level gauge\n\
         alpha_level{hook=\"a\",pool=\"chat\"} 0.5\n\
         # HELP zeta_total tokens \\\\ saved\n\
         # TYPE zeta_total counter\n\
         zeta_total{hook=\"b\"} 2\n\
         zeta_total{hook=\"a\"} 1\n"
    );
}

/// THE ONE-INSTANCE REFUSAL, in the words busbar 1.5.5 refused a second scrape-sink instance with:
#[test]
fn a_second_instance_is_refused_in_1_5_5_words() {
    assert!(check_limits(&[]).is_empty());
    assert!(check_limits(&["metrics".to_string()]).is_empty());
    let names = ["metrics", "again", "third"].map(String::from);
    assert_eq!(
        check_limits(&names),
        vec![
            "export.again: a second `module: prometheus` instance (already defined as 'metrics'). \
             Prometheus serves the ONE well-known /metrics route, so a second instance could only \
             be silently ignored — keep a single instance."
                .to_string(),
            "export.third: a second `module: prometheus` instance (already defined as 'metrics'). \
             Prometheus serves the ONE well-known /metrics route, so a second instance could only \
             be silently ignored — keep a single instance."
                .to_string(),
        ]
    );
}

/// The sink states the `one_instance` mark, which is what makes the host ask its limits check while
/// the configuration is resolved.
#[test]
fn the_statement_marks_one_instance() {
    assert_eq!(
        crate::door::STATEMENT.marks & busbar_contract::abi::mechanism::door::MARK_ONE_INSTANCE,
        busbar_contract::abi::mechanism::door::MARK_ONE_INSTANCE
    );
}

#[test]
fn empty_settings_are_refused_for_the_missing_buffer_seconds() {
    assert_eq!(
        validate(b"").expect_err("refused"),
        "settings: missing field `buffer_seconds`"
    );
}

#[test]
fn open_and_refresh_refuse_what_validate_refuses() {
    use crate::door::Prometheus;
    use busbar_contract::abi::sdk::life::Life;
    for bad in [
        &br#"{"buffer_seconds":0}"#[..],
        br#"{"buffer_seconds":60,"x":1}"#,
        b"not json",
    ] {
        let want = validate(bad).expect_err("validate refuses");
        let opened = Prometheus::open(bad, &[], 1).expect_err("open refuses");
        assert_eq!(opened.text(), Some(want.as_str()));
        let p = Prometheus::open(br#"{"buffer_seconds":60}"#, &[], 1).expect("opens");
        let refreshed = p.refresh(bad, &[], 2).expect_err("refresh refuses");
        assert_eq!(refreshed.text(), Some(want.as_str()));
    }
}

#[test]
fn the_statement_states_the_alias_rewrite() {
    use busbar_contract::abi::mechanism::door::REWRITE_ALIAS;
    assert_eq!(crate::door::STATEMENT.rewrites_len, 1);
    assert_eq!(crate::door::REWRITES[0].class, REWRITE_ALIAS);

    let from = crate::door::REWRITES[0].from;
    assert_eq!(from.len, crate::ALIAS.len());
    assert!(std::ptr::eq(from.ptr, crate::ALIAS.as_ptr()));
    assert!(crate::door::REWRITES[0].to.is_absent());
}

#[test]
fn published_settings_schema_matches_the_operator_parser() {
    let schema: serde_json::Value = serde_json::from_str(crate::SETTINGS_SCHEMA).unwrap();
    let blob = crate::door::STATEMENT.settings_schema;
    assert!(std::ptr::eq(blob.ptr, crate::SETTINGS_SCHEMA.as_ptr()));
    assert_eq!(blob.len, crate::SETTINGS_SCHEMA.len());
    assert_eq!(blob.fmt, busbar_contract::abi::mechanism::call::BLOB_JSON);
    assert_eq!(blob.flags, 0);
    assert_eq!(schema["required"], serde_json::json!(["buffer_seconds"]));
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["buffer_seconds"]["minimum"], 1);
    assert_eq!(schema["properties"]["key_gauge_limit"]["default"], 2000);
    for (value, accepted) in [
        (serde_json::json!({"buffer_seconds": 1}), true),
        (
            serde_json::json!({"buffer_seconds": 60, "key_gauge_limit": 2000}),
            true,
        ),
        (
            serde_json::json!({"buffer_seconds": 60, "key_gauge_limit": 0}),
            true,
        ),
        (serde_json::json!({}), false),
        (serde_json::json!({"buffer_seconds": 0}), false),
        (serde_json::json!({"buffer_seconds": "60"}), false),
        (
            serde_json::json!({"buffer_seconds": 60, "key_gauge_limit": -1}),
            false,
        ),
        (
            serde_json::json!({"buffer_seconds": 60, "unknown": 1}),
            false,
        ),
    ] {
        assert_eq!(
            crate::validate(&serde_json::to_vec(&value).unwrap()).is_ok(),
            accepted,
            "{value}"
        );
    }
}

/// `le` / `quantile` are set aside only for the family kind that owns them: a gauge's `le`-labelled
/// series are distinct series, ordered by their labels whatever order the recorder handed them in.
#[test]
fn a_gauge_with_le_labelled_series_renders_identically_from_both_input_orders() {
    let fam = |order: [&str; 2]| MetricFamily {
        name: "x".to_string(),
        kind: "gauge".to_string(),
        help: None,
        samples: order
            .iter()
            .map(|v| MetricSample {
                name: "x".to_string(),
                labels: vec![("le".to_string(), (*v).to_string())],
                value: "1".to_string(),
            })
            .collect(),
    };
    let (ab, ba) = (render(&[fam(["a", "b"])]), render(&[fam(["b", "a"])]));
    assert_eq!(ab, ba, "gauge series collapsed to one key keep input order");
}

#[test]
fn settings_schema_integer_tokens_match_native_parser() {
    let schema: serde_json::Value = serde_json::from_str(crate::SETTINGS_SCHEMA).unwrap();
    let profile = settings_profile::validator(&schema).unwrap();
    let standard = jsonschema::validator_for(&schema).unwrap();
    assert_eq!(schema["x-busbar-settings-profile"], "integer-token-v1");
    for (pointer, template, integer, kind) in [
        (
            r#"/properties/buffer_seconds"#,
            r#"{"buffer_seconds":TOKEN}"#,
            "1",
            "unsigned",
        ),
        (
            r#"/properties/key_gauge_limit"#,
            r#"{"buffer_seconds":60,"key_gauge_limit":TOKEN}"#,
            "1",
            "unsigned",
        ),
    ] {
        assert_eq!(
            schema.pointer(pointer).unwrap()["x-busbar-integer-token"],
            kind
        );
        for (token, accepted) in [
            (integer.to_string(), true),
            (format!("{integer}.0"), false),
            (format!("{integer}e0"), false),
            (format!("{integer}E+0"), false),
        ] {
            let text = template.replace("TOKEN", &token);
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert!(
                standard.is_valid(&value),
                "standard mathematical integer: {text}"
            );
            assert_eq!(profile.is_valid(&value), accepted, "profile: {text}");
            assert_eq!(
                crate::validate(text.as_bytes()).is_ok(),
                accepted,
                "native: {text}"
            );
        }
        // Do not infer signed negative-zero behavior from a regular expression:
        // the profile must agree with the actual lifecycle parser.
        for token in [
            "null",
            "0",
            "-0",
            "-0.0",
            "0e0",
            "-1",
            "18446744073709551616",
        ] {
            let text = template.replace("TOKEN", token);
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                profile.is_valid(&value),
                crate::validate(text.as_bytes()).is_ok(),
                "native sign/range: {text}"
            );
        }
    }
}
