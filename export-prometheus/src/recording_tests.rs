// SPDX-License-Identifier: Apache-2.0
// Neutral kind recording, executed against the external recorder oracle.

fn workload() {
    let recording: Vec<serde_json::Value> =
        serde_json::from_str(busbar_contract::abi::sdk::test_support::metrics::RECORDING)
            .expect("the kind's neutral recording");
    for record in recording {
        let name = record["name"].as_str().unwrap().to_owned();
        let labels: Vec<(String, String)> = record["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|pair| {
                (
                    pair[0].as_str().unwrap().to_owned(),
                    pair[1].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        match record["op"].as_str().unwrap() {
            "describe" => {
                let help = record["help"].as_str().unwrap().to_owned();
                match record["kind"].as_str().unwrap() {
                    "counter" => metrics::describe_counter!(name, help),
                    "gauge" => metrics::describe_gauge!(name, help),
                    "histogram" => metrics::describe_histogram!(name, help),
                    other => panic!("unknown recording kind: {other}"),
                }
            }
            "counter" => {
                metrics::counter!(name, &labels).increment(record["value"].as_u64().unwrap())
            }
            "gauge" => metrics::gauge!(name, &labels).set(record["value"].as_f64().unwrap()),
            "histogram" => {
                metrics::histogram!(name, &labels).record(record["value"].as_f64().unwrap())
            }
            "register_histogram" => {
                let _registered = metrics::histogram!(name, &labels);
            }
            other => panic!("unknown recording operation: {other}"),
        }
    }
}

#[test]
fn shared_raw_kind_vector_matches_the_external_recorder_bytes() {
    use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
    use busbar_contract::export_calls::{type_word, Family};
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new()
        .set_bucket_duration(std::time::Duration::from_secs(20))
        .unwrap()
        .set_bucket_count(std::num::NonZeroU32::new(3).unwrap())
        .idle_timeout(
            metrics_util::MetricKindMask::GAUGE,
            Some(std::time::Duration::from_secs(60)),
        )
        .build_recorder();
    metrics::with_local_recorder(&recorder, workload);
    let facts: Vec<Family> =
        serde_json::from_str(busbar_contract::abi::sdk::test_support::metrics::SNAPSHOT).unwrap();
    let input: Vec<MetricFamily> = facts
        .iter()
        .map(|family| MetricFamily {
            name: family.name.clone(),
            kind: type_word(family.kind).unwrap().into(),
            help: family.help.clone(),
            samples: family
                .samples
                .iter()
                .map(|sample| MetricSample {
                    name: sample.name.clone(),
                    labels: sample.labels.clone(),
                    value: sample.value.clone(),
                })
                .collect(),
        })
        .collect();
    // The external registry iterates series in hash order. Compare the complete
    // multiset of original wire lines: no parser or local encoder touches the
    // expected bytes, and duplicates remain observable.
    fn wire_lines(text: &str) -> Vec<&str> {
        let mut lines: Vec<_> = text.lines().filter(|line| !line.is_empty()).collect();
        lines.sort_unstable();
        lines
    }
    assert_eq!(
        wire_lines(&crate::render(&input)),
        wire_lines(&recorder.handle().render()),
        "the external recorder is the byte oracle, independent of registry order"
    );
}
