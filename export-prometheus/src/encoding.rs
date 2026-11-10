// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Prometheus owns metric identifier normalization and text escaping.
use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
use zeroize::Zeroize;

pub(super) fn sanitized(s: &str, colon: bool) -> String {
    let ok = |i: usize, c: char| {
        c.is_ascii_alphabetic() || c == '_' || (colon && c == ':') || (i > 0 && c.is_ascii_digit())
    };

    let mut out = zeroize::Zeroizing::new(String::with_capacity(s.len()));
    for (i, c) in s.chars().enumerate() {
        out.push(if ok(i, c) { c } else { '_' });
    }
    std::mem::take(&mut *out)
}

pub(super) fn escaped(s: &str, quotes: bool) -> String {
    let capacity = s.len().checked_mul(2).expect("metric escape size overflow");
    let mut out = zeroize::Zeroizing::new(String::with_capacity(capacity));
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '"' if quotes => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    std::mem::take(&mut *out)
}

pub(super) struct Encoded(pub(super) Vec<MetricFamily>);

impl Drop for Encoded {
    fn drop(&mut self) {
        for family in &mut self.0 {
            family.name.zeroize();
            family.kind.zeroize();
            family.help.zeroize();
            for sample in &mut family.samples {
                sample.name.zeroize();
                sample.labels.zeroize();
                sample.value.zeroize();
            }
        }
    }
}

pub(super) fn encode(families: &[MetricFamily], names: bool) -> Encoded {
    let name = |s: &str, colon| {
        if names {
            sanitized(s, colon)
        } else {
            s.to_owned()
        }
    };
    Encoded(
        families
            .iter()
            .map(|family| MetricFamily {
                name: name(&family.name, true),
                kind: family.kind.clone(),
                help: family.help.as_deref().map(|s| escaped(s, false)),
                samples: family
                    .samples
                    .iter()
                    .map(|sample| MetricSample {
                        name: name(&sample.name, true),
                        labels: sample
                            .labels
                            .iter()
                            .map(|(key, value)| (name(key, false), escaped(value, true)))
                            .collect(),
                        value: sample.value.clone(),
                    })
                    .collect(),
            })
            .collect(),
    )
}
