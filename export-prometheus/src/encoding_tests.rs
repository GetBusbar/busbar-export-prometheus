// SPDX-License-Identifier: Apache-2.0
// Protocol tests transferred from the core recorder to the owning exporter.
use super::encoding::{escaped, sanitized};
use metrics_exporter_prometheus::formatting as reference;

// Decode the protocol grammar independently of either exporter sanitizer. Family label values
// are already escaped; a Prometheus text consumer must recover the original recorder identity.
fn decode_escaped(value: &str, quotes: bool) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => decoded.push(match chars.next().expect("complete escape") {
                '\\' => '\\',
                'n' => '\n',
                '"' if quotes => '"',
                other => panic!("invalid escape: {other:?}"),
            }),
            '\n' => panic!("unescaped newline"),
            '"' if quotes => panic!("unescaped label quote"),
            other => decoded.push(other),
        }
    }
    decoded
}

#[test]
fn raw_values_escape_once_and_roundtrip_through_the_protocol() {
    // Explicit wire expectations cover slash runs, slash-before-quote/newline, and literal \\n.
    for (raw, label, help) in [
        ("", "", ""),
        ("plain", "plain", "plain"),
        ("a\"b", "a\\\"b", "a\"b"),
        ("a\\b", "a\\\\b", "a\\\\b"),
        ("a\\\\b", "a\\\\\\\\b", "a\\\\\\\\b"),
        ("a\\\"b", "a\\\\\\\"b", "a\\\\\"b"),
        ("a\\nb", "a\\\\nb", "a\\\\nb"),
        ("line\nfeed", "line\\nfeed", "line\\nfeed"),
        ("trailing\\", "trailing\\\\", "trailing\\\\"),
        ("\\\\\\", "\\\\\\\\\\\\", "\\\\\\\\\\\\"),
        ("\\\n", "\\\\\\n", "\\\\\\n"),
        ("ünï\"cødé\\", "ünï\\\"cødé\\\\", "ünï\"cødé\\\\"),
    ] {
        assert_eq!(escaped(raw, true), label, "label: {raw:?}");
        assert_eq!(escaped(raw, false), help, "help: {raw:?}");
        assert_eq!(decode_escaped(label, true), raw);
        assert_eq!(decode_escaped(help, false), raw);
    }
}

#[test]
fn metric_and_label_names_keep_existing_sanitization() {
    for s in [
        "busbar_ok",
        "busbar:ok",
        "9lead",
        "a.b-c",
        ":colon",
        "ü",
        "",
    ] {
        assert_eq!(
            sanitized(s, true),
            reference::sanitize_metric_name(s),
            "{s:?}"
        );
        assert_eq!(
            sanitized(s, false),
            reference::sanitize_label_key(s),
            "{s:?}"
        );
    }
}

#[test]
fn raw_metric_text_is_encoded_once_by_the_owner() {
    use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
    let families = [MetricFamily {
        name: "work.items".into(),
        kind: "counter".into(),
        help: Some("raw\nhelp\\text".into()),
        samples: vec![MetricSample {
            name: "work.items".into(),
            labels: vec![("9key".into(), "a\"b\\c\nd".into())],
            value: "3".into(),
        }],
    }];
    assert_eq!(crate::render(&families), "# HELP work_items raw\\nhelp\\\\text\n# TYPE work_items counter\nwork_items{_key=\"a\\\"b\\\\c\\nd\"} 3\n\n");
    assert_eq!(
        families[0].samples[0].labels[0].1, "a\"b\\c\nd",
        "the lent raw input is unchanged"
    );
}

#[test]
fn wire_series_order_is_owned_by_the_exporter() {
    use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
    let family = MetricFamily {
        name: "order".into(),
        kind: "counter".into(),
        help: None,
        samples: [("\n", "19"), ("\"", "17"), ("\\", "31"), ("\\\\", "23")]
            .into_iter()
            .map(|(label, value)| MetricSample {
                name: "order".into(),
                labels: vec![("identity".into(), label.into())],
                value: value.into(),
            })
            .collect(),
    };
    let text = crate::render(&[family]);
    let values: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("order{"))
        .map(|line| line.rsplit_once(' ').unwrap().1)
        .collect();
    assert_eq!(values, ["17", "31", "23", "19"]);
}

#[test]
fn normalized_family_names_share_one_header_without_losing_raw_series() {
    use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
    let families: Vec<_> = [("work.items", "a", "2"), ("work-items", "b", "3")]
        .into_iter()
        .map(|(name, label, value)| MetricFamily {
            name: name.into(),
            kind: "counter".into(),
            help: None,
            samples: vec![MetricSample {
                name: name.into(),
                labels: vec![("source".into(), label.into())],
                value: value.into(),
            }],
        })
        .collect();
    assert_eq!(
        crate::render(&families),
        "# TYPE work_items counter\nwork_items{source=\"a\"} 2\nwork_items{source=\"b\"} 3\n\n"
    );
}
