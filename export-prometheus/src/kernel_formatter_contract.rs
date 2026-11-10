// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Formatter predicates over the exact Family data asserted by the kernel's conversion tests.
//! Filtering, escaping and conversion remain kernel data oracles; these tests call the real sink.

use crate::{render, render_hooks};
use busbar_contract::abi::sdk::{MetricFamily, MetricSample};

fn sample(name: &str, labels: &[(&str, &str)], value: &str) -> MetricSample {
    MetricSample {
        name: name.to_owned(),
        labels: labels
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        value: value.to_owned(),
    }
}

fn family(name: &str, kind: &str, help: Option<&str>, samples: Vec<MetricSample>) -> MetricFamily {
    MetricFamily {
        name: name.to_owned(),
        kind: kind.to_owned(),
        help: help.map(str::to_owned),
        samples,
    }
}

#[test]
fn counter_renders_with_hook_label_and_verbatim_name() {
    let input = [family(
        "tokens_saved_total",
        "counter",
        Some("tokens the hook saved"),
        vec![sample(
            "tokens_saved_total",
            &[("hook", "compress"), ("pool", "chat")],
            "42",
        )],
    )];
    let text = render_hooks(&input);
    assert!(text.contains("# TYPE tokens_saved_total counter"), "{text}");
    assert!(
        text.contains("# HELP tokens_saved_total tokens the hook saved"),
        "{text}"
    );
    assert!(
        text.contains("tokens_saved_total{hook=\"compress\",pool=\"chat\"} 42\n"),
        "{text}"
    );
    assert_eq!(text, "# HELP tokens_saved_total tokens the hook saved\n# TYPE tokens_saved_total counter\ntokens_saved_total{hook=\"compress\",pool=\"chat\"} 42\n");
}

#[test]
fn histogram_renders_as_summary() {
    let labels = [("hook", "compress"), ("pool", "chat")];
    let input = [family(
        "compress_latency_us",
        "summary",
        None,
        vec![
            sample(
                "compress_latency_us",
                &[("hook", "compress"), ("pool", "chat"), ("quantile", "0.5")],
                "18",
            ),
            sample(
                "compress_latency_us",
                &[("hook", "compress"), ("pool", "chat"), ("quantile", "0.95")],
                "54",
            ),
            sample(
                "compress_latency_us",
                &[("hook", "compress"), ("pool", "chat"), ("quantile", "0.99")],
                "91",
            ),
            sample("compress_latency_us_count", &labels, "1000"),
        ],
    )];
    let text = render_hooks(&input);
    assert!(
        text.contains("# TYPE compress_latency_us summary"),
        "{text}"
    );
    assert!(
        text.contains("compress_latency_us{hook=\"compress\",pool=\"chat\",quantile=\"0.5\"} 18\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "compress_latency_us{hook=\"compress\",pool=\"chat\",quantile=\"0.99\"} 91\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("compress_latency_us_count{hook=\"compress\",pool=\"chat\"} 1000\n"),
        "{text}"
    );
}

#[test]
fn hook_histogram_keeps_its_kind_and_bucket_order_as_handed() {
    let input = [family(
        "rewriter_compression_ratio",
        "histogram",
        None,
        vec![
            sample(
                "rewriter_compression_ratio_bucket",
                &[("hook", "compress"), ("pool", "chat"), ("le", "0.5")],
                "3",
            ),
            sample(
                "rewriter_compression_ratio_bucket",
                &[("hook", "compress"), ("pool", "chat"), ("le", "1")],
                "7",
            ),
            sample(
                "rewriter_compression_ratio_bucket",
                &[("hook", "compress"), ("pool", "chat"), ("le", "+Inf")],
                "7",
            ),
            sample(
                "rewriter_compression_ratio_count",
                &[("hook", "compress"), ("pool", "chat")],
                "7",
            ),
        ],
    )];
    assert_eq!(
        render_hooks(&input),
        "# TYPE rewriter_compression_ratio histogram\n\
         rewriter_compression_ratio_bucket{hook=\"compress\",pool=\"chat\",le=\"0.5\"} 3\n\
         rewriter_compression_ratio_bucket{hook=\"compress\",pool=\"chat\",le=\"1\"} 7\n\
         rewriter_compression_ratio_bucket{hook=\"compress\",pool=\"chat\",le=\"+Inf\"} 7\n\
         rewriter_compression_ratio_count{hook=\"compress\",pool=\"chat\"} 7\n",
        "the hook exposition keeps the kind and the bucket order as handed"
    );
}

#[test]
fn two_hooks_same_name_share_header_split_by_label() {
    let input = [family(
        "proxy_compression_ratio_by_strategy",
        "gauge",
        None,
        vec![
            sample(
                "proxy_compression_ratio_by_strategy",
                &[("hook", "hook_a")],
                "0.4",
            ),
            sample(
                "proxy_compression_ratio_by_strategy",
                &[("hook", "hook_b")],
                "0.6",
            ),
        ],
    )];
    let text = render_hooks(&input);
    assert_eq!(
        text.matches("# TYPE proxy_compression_ratio_by_strategy")
            .count(),
        1,
        "one TYPE header per name: {text}"
    );
    assert!(
        text.contains("proxy_compression_ratio_by_strategy{hook=\"hook_a\"} 0.4\n"),
        "{text}"
    );
    assert!(
        text.contains("proxy_compression_ratio_by_strategy{hook=\"hook_b\"} 0.6\n"),
        "{text}"
    );
}

#[test]
fn label_values_are_escaped() {
    let input = [family(
        "x_total",
        "counter",
        None,
        vec![sample("x_total", &[("hook", "h"), ("k", r#"a"b\c"#)], "1")],
    )];
    let text = render_hooks(&input);
    assert!(text.contains(r#"k="a\"b\\c""#), "{text}");
}

#[test]
fn hook_labels_are_written_once_as_handed() {
    let input = [
        family(
            "chars_saved_total",
            "histogram",
            None,
            vec![
                sample(
                    "chars_saved_total_bucket",
                    &[("hook", "compress"), ("quantile", "0.5"), ("le", "1")],
                    "1",
                ),
                sample(
                    "chars_saved_total_bucket",
                    &[("hook", "compress"), ("quantile", "0.5"), ("le", "+Inf")],
                    "3",
                ),
                sample(
                    "chars_saved_total_count",
                    &[("hook", "compress"), ("le", "oops"), ("quantile", "0.5")],
                    "3",
                ),
            ],
        ),
        family(
            "latency_seconds",
            "untyped",
            None,
            vec![sample(
                "latency_seconds",
                &[("hook", "compress"), ("le", "oops"), ("quantile", "0.5")],
                "2",
            )],
        ),
    ];
    assert_eq!(
        render_hooks(&input),
        "# TYPE chars_saved_total histogram\n\
         chars_saved_total_bucket{hook=\"compress\",quantile=\"0.5\",le=\"1\"} 1\n\
         chars_saved_total_bucket{hook=\"compress\",quantile=\"0.5\",le=\"+Inf\"} 3\n\
         chars_saved_total_count{hook=\"compress\",le=\"oops\",quantile=\"0.5\"} 3\n\
         # TYPE latency_seconds untyped\n\
         latency_seconds{hook=\"compress\",le=\"oops\",quantile=\"0.5\"} 2\n",
        "each label is written once, in the order the host handed it"
    );
}

#[test]
fn test_request_finished_renders_premigration_names_and_labels() {
    for ingress in ["acme", "openai"] {
        let labels = [("ingress_protocol", ingress), ("pool", "tel-parity-pool")];
        let input = [
            family(
                "busbar_requests_total",
                "counter",
                None,
                vec![sample(
                    "busbar_requests_total",
                    &[
                        ("ingress_protocol", ingress),
                        ("pool", "tel-parity-pool"),
                        ("outcome", "ok"),
                    ],
                    "1",
                )],
            ),
            family(
                "busbar_request_duration_seconds",
                "summary",
                None,
                vec![
                    sample(
                        "busbar_request_duration_seconds",
                        &[
                            ("ingress_protocol", ingress),
                            ("pool", "tel-parity-pool"),
                            ("quantile", "0.99"),
                        ],
                        "0.005",
                    ),
                    sample("busbar_request_duration_seconds_count", &labels, "1"),
                ],
            ),
        ];
        let out = render(&input);
        let ingress_label = format!("ingress_protocol=\"{ingress}\"");
        let counter_line = out.lines().find(|line| {
            !line.starts_with('#')
                && line.starts_with("busbar_requests_total")
                && line.contains(&ingress_label)
                && line.contains("pool=\"tel-parity-pool\"")
                && line.contains("outcome=\"ok\"")
        });
        assert!(
            counter_line.is_some(),
            "banked requests_total must render with the exact pre-migration labels; got:\n{out}"
        );
        assert!(
            out.lines().any(|line| !line.starts_with('#')
                && line.starts_with("busbar_request_duration_seconds")
                && line.contains("pool=\"tel-parity-pool\"")
                && line.contains("quantile=")),
            "duration summary quantile lines must render for the banked pool; got:\n{out}"
        );
    }
}
