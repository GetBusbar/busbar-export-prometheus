// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! **ONE SCRAPE SINK, BOTH DOORS, ONE TABLE** — the prometheus sink's linked + dropped-in
//! conformance on the export kind's memory ABI (THE DESIGN §11.4), run against the busbar rev this
//! repo pins (`.busbar-ref`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use busbar_contract::abi::export::{
    self, CheckIn, CheckOut, DeliverIn, ScrapeFamily, ScrapeIn, ScrapeLabel, ScrapeOut,
    ScrapeSample, ServeIn, ServeOut, StatusOut,
};
use busbar_contract::abi::mechanism::call::{
    AbiStr, Blob, InHead, OutHead, Outcome, BLOB_JSON, BLOB_JSONL,
};
use busbar_contract::abi::mechanism::lifecycle::{slot as lc, OpenIn, OpenOut, ValidateIn};
use busbar_contract::abi::mechanism::rendering::RENDERING_MAGIC;
use busbar_contract::abi::sdk::door::{blank_in, blank_out};
use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
use busbar_contract::export_calls::type_word;
use busbar_plugin_loader::dispatch::kinds::export::Export;
use busbar_plugin_loader::dispatch::kinds::secret::Secret;
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, Called, ConnTable, Diagnostic,
    DispatchConfig, Dispatcher, Dropped, EnvelopeSink, Frame, LinkedRow, LoadError, Metric, NoSink,
    Plugin, NO_BLOB,
};
use serde_json::{json, Value};

/// The settings every opened sink is handed.
const SETTINGS: &str = r#"{"buffer_seconds":60}"#;

/// The recorder exposition the scrape renders: every family type the recorder writes — a HELP-less
/// counter with escaped label values, a histogram, a gauge, a quantile summary — never empty, so the
/// proof is not vacuous. The families are in the sink's stable order — every counter, then every
/// gauge, then every histogram/summary (v1.5.5's own renderer drains its maps in that fixed order),
/// name-sorted within a kind — so the bytes come back unchanged.
const OWN: &str = "# TYPE busbar_requests_total counter\n\
                   busbar_requests_total{pool=\"a\\\"b\\\\c\\nd\",outcome=\"ok\"} 3\n\
                   \n\
                   # TYPE busbar_conformance_gauge gauge\n\
                   busbar_conformance_gauge 1.5\n\
                   \n\
                   # TYPE busbar_plane_request_duration_seconds summary\n\
                   busbar_plane_request_duration_seconds{quantile=\"0.99\"} 0.0125\n\
                   busbar_plane_request_duration_seconds_sum 1e-3\n\
                   busbar_plane_request_duration_seconds_count 4\n\
                   \n\
                   # HELP busbar_request_duration_seconds request latency\n\
                   # TYPE busbar_request_duration_seconds histogram\n\
                   busbar_request_duration_seconds_bucket{le=\"0.5\"} 1\n\
                   busbar_request_duration_seconds_bucket{le=\"+Inf\"} 2\n\
                   busbar_request_duration_seconds_sum 0.75\n\
                   busbar_request_duration_seconds_count 2\n\
                   \n";

#[path = "support/cdylib.rs"]
mod artifact;
use artifact::cdylib;

#[path = "support/inbound.rs"]
mod inbound;

/// The row a busbar build that links the sink states.
fn row() -> LinkedRow {
    LinkedRow::of(busbar_export_prometheus::door).expect("the linked door renders")
}

fn dispatcher() -> Arc<Dispatcher> {
    Arc::new(Dispatcher::new(
        DispatchConfig {
            workers: 2,
            watchdog_period: Duration::from_millis(20),
            ..DispatchConfig::default()
        },
        busbar_contract::services::test_caller_binding(),
    ))
}

fn bind(d: &Dispatcher) -> Bind {
    Bind {
        instance: Arc::from("metrics"),
        max_inflight_cap: 64,
        sink: Arc::new(NoSink),
        dispatcher: d.adopter(),
        conns: ConnTable::Host(inbound::host(d.conn_waker(), None)),
    }
}

/// The door, linked or dropped in.
fn load(d: &Dispatcher, dropped: bool) -> Plugin<Export> {
    load_with_sink(d, dropped, Arc::new(NoSink))
}

fn load_with_sink(d: &Dispatcher, dropped: bool, sink: Arc<dyn EnvelopeSink>) -> Plugin<Export> {
    let binding = Bind { sink, ..bind(d) };
    if dropped {
        load_dropped::<Export>(&cdylib(), &row().statement, binding)
            .expect("the dropped-in door loads")
    } else {
        load_linked::<Export>(&row(), binding).expect("the linked door loads")
    }
}

fn blob(bytes: &[u8]) -> Blob {
    Blob {
        ptr: bytes.as_ptr(),
        len: bytes.len(),
        fmt: BLOB_JSON,
        flags: 0,
    }
}

const ABSENT: AbiStr = AbiStr {
    ptr: std::ptr::null(),
    len: 0,
};

fn abi(s: &str) -> AbiStr {
    AbiStr {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

fn spelled(c: &Called) -> String {
    let text = c
        .error
        .as_deref()
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    format!("{:?} {text}", c.outcome)
}

fn validate(p: &Plugin<Export>, settings: &str) -> String {
    let mut input: ValidateIn = blank_in();
    input.head = in_head();
    input.settings = blob(settings.as_bytes());
    spelled(&p.call(lc::VALIDATE, &mut Frame::new(input, out_head())))
}

fn open(p: &Plugin<Export>) -> String {
    let mut input: OpenIn = blank_in();
    input.head = in_head();
    input.settings = blob(SETTINGS.as_bytes());
    input.generation = 1;
    let mut out: OpenOut = blank_out();
    out.head = out_head();
    spelled(&p.call(lc::OPEN, &mut Frame::new(input, out)))
}

/// The kind's four non-scrape answers.
fn kind_answers(p: &Plugin<Export>) -> Value {
    let mut deliver: DeliverIn = blank_in();
    deliver.head = in_head();
    let status = StatusOut {
        head: out_head(),
        status: NO_BLOB,
    };
    let mut check: CheckIn = blank_in();
    check.head = in_head();
    let check_out = CheckOut {
        head: out_head(),
        findings: NO_BLOB,
    };
    let mut serve: ServeIn = blank_in();
    serve.head = in_head();
    let mut serve_out: ServeOut = blank_out();
    serve_out.head = out_head();
    json!({
        "deliver": spelled(&p.call(export::slot::DELIVER, &mut Frame::new(deliver, out_head()))),
        "status": spelled(&p.call(export::slot::STATUS, &mut Frame::new(in_head(), status))),
        "check": spelled(&p.call(export::slot::CHECK, &mut Frame::new(check, check_out))),
        "serve": spelled(&p.call(export::slot::SERVE, &mut Frame::new(serve, serve_out))),
    })
}

fn kind_code(kind: &str) -> u8 {
    match kind {
        "counter" => export::SCRAPE_KIND_COUNTER,
        "gauge" => export::SCRAPE_KIND_GAUGE,
        "histogram" => export::SCRAPE_KIND_HISTOGRAM,
        "summary" => export::SCRAPE_KIND_SUMMARY,
        _ => export::SCRAPE_KIND_UNTYPED,
    }
}

/// `scrape` of `families` (lent as the host lends them) into a buffer of `cap` bytes: the answer,
/// the bytes written or needed, and the text — and, for a short answer, the ONE re-call with a
/// buffer as large as needed.
fn scrape(p: &Plugin<Export>, families: &[MetricFamily], cap: usize) -> Value {
    scrape_flagged(p, families, cap, 0)
}

/// [`scrape`] with the `ScrapeIn` head's `flags` set — `SCRAPE_FLAG_HOOK_FAMILIES` asks for the
/// host's hook families in 1.5.5's `/metrics/hooks` layout.
fn scrape_flagged(p: &Plugin<Export>, families: &[MetricFamily], cap: usize, flags: u32) -> Value {
    let labels: Vec<Vec<Vec<ScrapeLabel>>> = families
        .iter()
        .map(|f| {
            f.samples
                .iter()
                .map(|s| {
                    s.labels
                        .iter()
                        .map(|(k, v)| ScrapeLabel {
                            key: abi(k),
                            value: abi(v),
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let samples: Vec<Vec<ScrapeSample>> = families
        .iter()
        .zip(&labels)
        .map(|(f, ls)| {
            f.samples
                .iter()
                .zip(ls)
                .map(|(s, l)| ScrapeSample {
                    name: abi(&s.name),
                    labels: l.as_ptr(),
                    labels_len: l.len(),
                    value: abi(&s.value),
                })
                .collect()
        })
        .collect();
    let lent: Vec<ScrapeFamily> = families
        .iter()
        .zip(&samples)
        .map(|(f, ss)| ScrapeFamily {
            name: abi(&f.name),
            help: f.help.as_deref().map_or(ABSENT, abi),
            unit: ABSENT,
            kind: kind_code(&f.kind),
            _reserved: [0; 7],
            samples: ss.as_ptr(),
            samples_len: ss.len(),
        })
        .collect();
    let call = |buf: &mut Vec<u8>| {
        let mut input: ScrapeIn = blank_in();
        input.head = in_head();
        input.head.flags = flags;
        input.families = lent.as_ptr();
        input.families_len = lent.len();
        input.buf = buf.as_mut_ptr();
        input.cap = buf.len();
        let mut out: ScrapeOut = blank_out();
        out.head = out_head();
        Frame::new(input, out)
    };
    let mut buf = vec![0u8; cap];
    let mut frame = call(&mut buf);
    let first = p.call(export::slot::SCRAPE, &mut frame);
    let (written, needed) = (frame.out.written, frame.out.needed);
    let text = String::from_utf8_lossy(&buf[..written.min(cap)]).into_owned();

    let untouched = buf.iter().all(|b| *b == 0);
    let answer = spelled(&first);
    let recalled = first.recall.map(|token| {
        let mut bigger = vec![0u8; needed];
        let mut frame = call(&mut bigger);
        let again = p.recall(token, export::slot::SCRAPE, &mut frame);
        let n = frame.out.written.min(bigger.len());
        json!([spelled(&again), String::from_utf8_lossy(&bigger[..n])])
    });
    json!({
        "answer": answer,
        "written": written,
        "needed": needed,
        "text": text,
        "untouched": untouched,
        "recall": recalled,
    })
}

fn close(p: &Plugin<Export>) -> String {
    let mut f: Frame<InHead, OutHead> = Frame::new(in_head(), out_head());
    spelled(&p.call(lc::CLOSE, &mut f))
}

/// One door's whole script, as one comparable transcript.
fn transcript(dropped: bool) -> Value {
    let d = dispatcher();
    let p = load(&d, dropped);
    let validation: Vec<String> = [
        json!({ "buffer_seconds": 60 }),
        json!({ "buffer_seconds": 60, "key_gauge_limit": 5 }),
        json!({}),
        json!({ "buffer_seconds": 60, "buffer": 1 }),
        json!({ "buffer_seconds": 0 }),
        json!({ "buffer_seconds": "sixty" }),
    ]
    .iter()
    .map(|s| validate(&p, &s.to_string()))
    .collect();
    let opened = open(&p);
    let answers = kind_answers(&p);
    let families = to_contract(OWN);
    let mut reversed = families.clone();
    reversed.reverse();
    json!({
        "name": p.name(),
        "kind": format!("{:?}", p.kind()),
        "validate": validation,
        "open": opened,
        "answers": answers,
        "scrape": scrape(&p, &families, 64 * 1024),
        "scrape_reordered": scrape(&p, &reversed, 64 * 1024),
        "scrape_short": scrape(&p, &families, 8),
        "scrape_empty": scrape(&p, &[], 64),
        "close": close(&p),
    })
}

/// The recorder snapshot of `exposition`, as the host's snapshot reader takes it
/// (`busbar_contract::export_calls::parse_families`), in the SDK's family shape: each family's
/// `# TYPE` word, its HELP and its samples exactly as read. Contract types only: the plugin talks to
/// busbar over the ABI and names no loader internals (Part 2 #2).
fn to_contract(exposition: &str) -> Vec<MetricFamily> {
    let families = busbar_contract::export_calls::parse_families(exposition)
        .expect("the exposition snapshots");
    families
        .iter()
        .map(|f| MetricFamily {
            name: f.name.clone(),
            kind: type_word(f.kind).unwrap_or("untyped").to_string(),
            help: f.help.clone(),
            samples: f
                .samples
                .iter()
                .map(|s| MetricSample {
                    name: s.name.clone(),
                    labels: s.labels.clone(),
                    value: s.value.clone(),
                })
                .collect(),
        })
        .collect()
}

/// The prometheus sink is ONE plugin through either door, and the bytes it renders are the
/// recorder's own.
#[test]
fn the_linked_and_the_dropped_in_prometheus_sink_are_one_sink() {
    let linked = transcript(false);
    let dropped_in = transcript(true);
    assert_eq!(linked, dropped_in, "the two doors are not one sink");

    assert_eq!(linked["name"], "busbar-export-prometheus");
    assert_eq!(linked["kind"], "Export");
    assert_eq!(
        linked["validate"],
        json!([
            "Ready ",
            "Ready ",
            "Failed settings: missing field `buffer_seconds`",
            "Failed settings: unknown field `buffer`, expected `buffer_seconds` or `key_gauge_limit`",
            "Failed the `module: prometheus` export instance sets settings.buffer_seconds: 0, which \
             retains no observations — every scrape would report empty quantiles while still \
             paying the recording cost. Name a positive retention window in seconds, or remove \
             the instance to turn metrics off",
            "Failed settings: invalid type: string \"sixty\", expected u64",
        ])
    );
    assert_eq!(linked["open"], "Ready ");
    assert_eq!(
        linked["answers"],
        json!({"deliver": "Ready ", "status": "Ready ", "check": "Ready ",
               "serve": "Refused not a route this sink serves (only GET /metrics and /metrics/hooks)"})
    );
    assert_eq!(
        linked["scrape"],
        json!({"answer": "Ready ", "written": OWN.len(), "needed": 0, "text": OWN, "untouched": false, "recall": null}),
        "the scrape is the recorder's bytes, back"
    );
    assert_eq!(
        linked["scrape_reordered"]["text"], OWN,
        "the same snapshot in another order renders the same bytes"
    );
    assert_eq!(
        linked["scrape_short"],
        json!({
            "answer": "Failed ", "written": 0, "needed": OWN.len(), "text": "", "untouched": true,
            "recall": ["Ready ", OWN],
        }),
        "a short buffer is answered with the bytes needed, then rendered on the one re-call"
    );
    assert_eq!(
        linked["scrape_empty"],
        json!({"answer": "Ready ", "written": 0, "needed": 0, "text": "", "untouched": true, "recall": null}),
        "an empty recorder renders nothing"
    );
    assert_eq!(linked["close"], "Ready ");
}

/// The hook-families scrape (`SCRAPE_FLAG_HOOK_FAMILIES`, 1.5.5's `/metrics/hooks`) is rendered in
/// the hook layout through BOTH doors: families by name whatever order they are handed in, HELP
/// when present, and NO blank line between families — unlike the recorder layout.
#[test]
fn the_hook_families_scrape_is_the_hook_layout_through_both_doors() {
    const HOOKS: &str = "# TYPE busbar_hook_calls_total counter\n\
                         busbar_hook_calls_total{hook=\"a\"} 2\n\
                         busbar_hook_calls_total{hook=\"b\"} 5\n\
                         # HELP busbar_hook_duration_seconds hook latency\n\
                         # TYPE busbar_hook_duration_seconds gauge\n\
                         busbar_hook_duration_seconds{hook=\"a\"} 0.5\n";
    const RECORDER: &str = "# TYPE busbar_hook_calls_total counter\n\
                            busbar_hook_calls_total{hook=\"a\"} 2\n\
                            busbar_hook_calls_total{hook=\"b\"} 5\n\
                            \n\
                            # HELP busbar_hook_duration_seconds hook latency\n\
                            # TYPE busbar_hook_duration_seconds gauge\n\
                            busbar_hook_duration_seconds{hook=\"a\"} 0.5\n\
                            \n";
    let sample = |name: &str, hook: &str, value: &str| MetricSample {
        name: name.to_string(),
        labels: vec![("hook".to_string(), hook.to_string())],
        value: value.to_string(),
    };

    let handed = vec![
        MetricFamily {
            name: "busbar_hook_duration_seconds".to_string(),
            kind: "gauge".to_string(),
            help: Some("hook latency".to_string()),
            samples: vec![sample("busbar_hook_duration_seconds", "a", "0.5")],
        },
        MetricFamily {
            name: "busbar_hook_calls_total".to_string(),
            kind: "counter".to_string(),
            help: None,
            samples: vec![
                sample("busbar_hook_calls_total", "a", "2"),
                sample("busbar_hook_calls_total", "b", "5"),
            ],
        },
    ];
    for dropped in [false, true] {
        let d = dispatcher();
        let p = load(&d, dropped);
        assert_eq!(validate(&p, SETTINGS), "Ready ");
        assert_eq!(open(&p), "Ready ");
        let got = scrape_flagged(&p, &handed, 64 * 1024, export::SCRAPE_FLAG_HOOK_FAMILIES);
        assert_eq!(
            got,
            json!({"answer": "Ready ", "written": HOOKS.len(), "needed": 0, "text": HOOKS, "untouched": false, "recall": null}),
            "dropped_in={dropped}: the hook scrape is the hook layout"
        );

        let plain = scrape(&p, &handed, 64 * 1024);
        assert_eq!(
            plain,
            json!({"answer": "Ready ", "written": RECORDER.len(), "needed": 0, "text": RECORDER, "untouched": false, "recall": null}),
            "dropped_in={dropped}: the unflagged scrape keeps the recorder layout"
        );
        close(&p);
    }
}

#[derive(Default)]
struct Observed(AtomicUsize);

impl EnvelopeSink for Observed {
    fn metric(&self, _: Metric<'_>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn diag(&self, _: Diagnostic<'_>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn dropped(&self, _: Dropped) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn a_delivered_metric_batch_produces_no_host_observations_through_either_door() {
    for dropped in [false, true] {
        let d = dispatcher();
        let observed = Arc::new(Observed::default());
        let p = load_with_sink(&d, dropped, observed.clone());
        assert_eq!(open(&p), "Ready ");
        observed.0.store(0, Ordering::SeqCst);
        let mut input: DeliverIn = blank_in();
        input.head = in_head();
        input.op_id = [1; 16];
        input.stream = export::ExportStream::Metrics as u8;
        input.batch = Blob {
            fmt: BLOB_JSONL,
            ..blob(b"{\"ts\":1}\n")
        };
        let called = p.call(export::slot::DELIVER, &mut Frame::new(input, out_head()));
        assert_eq!(called.outcome, Outcome::Ready, "dropped={dropped}");
        assert_eq!(
            observed.0.load(Ordering::SeqCst),
            0,
            "a pull sink does not emit a delivered batch"
        );
        close(&p);
    }
}

#[test]
fn a_wrong_kind_is_refused() {
    let d = dispatcher();
    let err =
        load_linked::<Secret>(&row(), bind(&d)).expect_err("an export door is not a secret door");
    assert_eq!(
        err,
        LoadError::ManifestKind {
            stated: busbar_contract::abi::mechanism::KindCode::Export,
            want: busbar_contract::abi::mechanism::KindCode::Secret,
        },
        "the linked row is rejected before its wrong-kind table is read"
    );
    let err = load_dropped::<Secret>(&cdylib(), &row().statement, bind(&d))
        .expect_err("the dropped-in export door is not a secret door");
    assert_eq!(
        err,
        LoadError::ManifestKind {
            stated: busbar_contract::abi::mechanism::KindCode::Export,
            want: busbar_contract::abi::mechanism::KindCode::Secret,
        },
        "the dropped manifest is rejected before its wrong-kind image is loaded"
    );
}

#[test]
fn a_manifest_stating_the_1_5_5_export_abi_is_refused() {
    let mut stated = row().statement;
    let at = RENDERING_MAGIC.len() + 8;
    assert_eq!(
        u32::from_le_bytes(stated[at..at + 4].try_into().unwrap()),
        export::ABI_VERSION
    );
    stated[at..at + 4].copy_from_slice(&(export::ABI_VERSION - 1).to_le_bytes());
    let err = load_dropped::<Export>(&cdylib(), &stated, bind(&dispatcher()))
        .expect_err("1.5.5's export ABI is refused");
    assert!(matches!(err, LoadError::ManifestKindAbi { .. }), "{err:?}");
}

#[test]
fn lifecycle_and_configured_examples_cross_both_real_doors() {
    use busbar_contract::abi::export::{CheckInstance, CHECK_PHASE_INSTANCES, CHECK_PHASE_LIMITS};
    use busbar_contract::abi::mechanism::lifecycle::{RefreshIn, ReleaseIn, TickIn, TickOut};

    let cases: Value = serde_json::from_str(include_str!("conformance.json")).expect("test cases");
    for dropped in [false, true] {
        let d = dispatcher();
        let p = load(&d, dropped);
        let count = || p.test_invocations().0;
        let initial = count();
        assert_eq!(p.name(), "busbar-export-prometheus");
        assert_eq!(p.kind(), busbar_contract::abi::mechanism::KindCode::Export);
        assert_eq!(p.max_inflight(), 64);
        assert_eq!(count(), initial, "reading facts does not cross the door");
        for bad in cases["export"]["bad_settings"]
            .as_array()
            .expect("bad settings")
        {
            let before = count();
            assert_eq!(
                validate(&p, &bad["settings"].to_string()),
                format!("Failed {}", bad["refusal"].as_str().unwrap())
            );
            assert_eq!(count() - before, 1);
        }
        let deliver = || {
            let mut input: DeliverIn = blank_in();
            input.head = in_head();
            input.op_id = [1; 16];
            input.stream = export::ExportStream::Metrics as u8;
            input.batch = Blob {
                fmt: BLOB_JSONL,
                ..blob(b"{\"ts\":1}\n")
            };
            p.call(export::slot::DELIVER, &mut Frame::new(input, out_head()))
        };
        let before = count();
        assert_ne!(deliver().outcome, Outcome::Ready);
        assert_eq!(count(), before, "unopened delivery is refused by the host");
        let before = count();
        assert_eq!(validate(&p, SETTINGS), "Ready ");
        assert_eq!(count() - before, 1);
        let before = count();
        assert_eq!(open(&p), "Ready ");
        assert_eq!(count() - before, 1);
        let before = count();
        p.ready(&d, Duration::from_secs(30))
            .expect("the actual sink is ready");
        assert_eq!(count() - before, u64::from(p.has_ready()));
        let before = count();
        let called = deliver();
        assert_eq!(called.outcome, Outcome::Ready);
        assert_eq!(called.lease, 0);
        assert_eq!(count() - before, 1);
        for example in cases["export"]["scrape"].as_array().unwrap() {
            let families: Vec<MetricFamily> = example["families"]
                .as_array()
                .unwrap()
                .iter()
                .map(|family| MetricFamily {
                    name: family["name"].as_str().unwrap().to_owned(),
                    kind: family["kind"].as_str().unwrap().to_owned(),
                    help: family["help"].as_str().map(str::to_owned),
                    samples: family["samples"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|sample| MetricSample {
                            name: sample["name"].as_str().unwrap().to_owned(),
                            labels: sample["labels"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|pair| {
                                    (
                                        pair[0].as_str().unwrap().to_owned(),
                                        pair[1].as_str().unwrap().to_owned(),
                                    )
                                })
                                .collect(),
                            value: sample["value"].as_str().unwrap().to_owned(),
                        })
                        .collect(),
                })
                .collect();
            let before = count();
            let got = scrape(&p, &families, example["cap"].as_u64().unwrap() as usize);
            let expected = example["exposition"].as_str().unwrap();
            if expected.len() > example["cap"].as_u64().unwrap() as usize {
                assert_eq!(got["answer"], "Failed ");
                assert_eq!(got["recall"][0], "Ready ");
                assert_eq!(got["recall"][1], expected);
                assert_eq!(count() - before, 2);
            } else {
                assert_eq!(got["answer"], "Ready ");
                assert_eq!(got["text"], expected);
                assert_eq!(count() - before, 1);
            }
        }
        let mut leases = Vec::new();
        let before = count();
        let called = p.call(
            export::slot::STATUS,
            &mut Frame::new(
                in_head(),
                StatusOut {
                    head: out_head(),
                    status: NO_BLOB,
                },
            ),
        );
        assert_eq!(called.outcome, Outcome::Ready);
        assert_eq!(count() - before, 1);
        if called.lease != 0 {
            leases.push(called.lease);
        }
        let instances = ["conformance", "again"].map(|name| CheckInstance {
            name: abi(name),
            settings: blob(SETTINGS.as_bytes()),
        });
        for phase in [CHECK_PHASE_LIMITS, CHECK_PHASE_INSTANCES] {
            let mut input: CheckIn = blank_in();
            input.head = in_head();
            input.phase = phase;
            input.instances = instances.as_ptr();
            input.instances_len = instances.len();
            let before = count();
            let called = p.call(
                export::slot::CHECK,
                &mut Frame::new(
                    input,
                    CheckOut {
                        head: out_head(),
                        findings: NO_BLOB,
                    },
                ),
            );
            assert_eq!(called.outcome, Outcome::Ready);
            assert_eq!(count() - before, 1);
            if called.lease != 0 {
                leases.push(called.lease);
            }
        }
        let before = count();
        let mut input: ServeIn = blank_in();
        input.head = in_head();
        input.method = abi("GET");
        input.path = abi("/exports/metrics/x");
        let mut output: ServeOut = blank_out();
        output.head = out_head();
        let called = p.call(export::slot::SERVE, &mut Frame::new(input, output));
        assert_eq!(called.outcome, Outcome::Refused);
        assert_eq!(count() - before, 1);
        if called.lease != 0 {
            leases.push(called.lease);
        }
        assert!(
            !leases.is_empty(),
            "the second instance's limits finding is leased"
        );
        let mut unique = leases.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), leases.len());
        for (index, lease) in leases.iter().chain(leases.first()).enumerate() {
            let mut input: ReleaseIn = blank_in();
            input.head = in_head();
            input.lease = *lease;
            let before = count();
            let answer = p.call(lc::RELEASE, &mut Frame::new(input, out_head()));
            assert_eq!(
                answer.outcome,
                if index < leases.len() {
                    Outcome::Ready
                } else {
                    Outcome::Refused
                }
            );
            assert_eq!(count() - before, 1);
        }
        let mut input: TickIn = blank_in();
        input.head = in_head();
        input.now_ns = 1;
        let mut output: TickOut = blank_out();
        output.head = out_head();
        let before = count();
        assert_eq!(
            p.call(lc::TICK, &mut Frame::new(input, output)).outcome,
            Outcome::Ready
        );
        assert_eq!(count() - before, 1);
        for settings in ["{}", SETTINGS] {
            let mut input: RefreshIn = blank_in();
            input.head = in_head();
            input.generation = 2;
            input.settings = blob(settings.as_bytes());
            let before = count();
            let answer = p.call(lc::REFRESH, &mut Frame::new(input, out_head()));
            assert_eq!(
                answer.outcome,
                if settings == SETTINGS {
                    Outcome::Ready
                } else {
                    Outcome::Failed
                }
            );
            assert_eq!(count() - before, 1);
        }
        let before = count();
        assert_eq!(deliver().outcome, Outcome::Ready);
        assert_eq!(count() - before, 1);
        let before = count();
        assert_eq!(close(&p), "Ready ");
        assert_eq!(count() - before, 1);
        let before = count();
        assert_ne!(deliver().outcome, Outcome::Ready);
        assert_eq!(count(), before, "closed delivery never crosses the door");
    }
}

#[test]
fn the_real_scrape_instance_preserves_policy_refresh_recall_and_drop() {
    use busbar_contract::export_calls::{parse_families, ExportCalls, ServeRequest};
    use busbar_plugin_loader::dispatch::kinds::export::ExportFacts;
    use busbar_plugin_loader::export_door::ExportInstance;

    for dropped in [false, true] {
        let d = dispatcher();
        let p = load(&d, dropped);
        let facts = p
            .context::<ExportFacts>()
            .expect("the export tail is read at bind");
        assert_eq!(facts.streams, [export::ExportStream::Metrics as u8]);
        assert_eq!(
            facts
                .routes
                .iter()
                .map(|r| format!("{} {} {:?}", r.method.as_str(), r.path, r.auth))
                .collect::<Vec<_>>(),
            ["GET /metrics Key", "GET /metrics/hooks Key"]
        );
        assert_eq!(facts.public_routes, ["/metrics", "/metrics/hooks"]);
        assert_eq!(facts.persistent_routes, ["/metrics/hooks"]);
        let sink = ExportInstance::open(p, d, SETTINGS.as_bytes()).expect("the actual sink opens");
        assert_eq!(sink.public_routes(), ["/metrics", "/metrics/hooks"]);
        assert_eq!(sink.persistent_routes(), ["/metrics/hooks"]);
        for settings in [
            json!({"bogus": 1}),
            json!({"buffer_seconds": 0}),
            json!({"buffer_seconds": "thirty"}),
        ] {
            assert!(sink.refresh(&settings, 2).is_err(), "{settings}");
            assert!(sink.plugin().is_open());
        }
        sink.refresh(&json!({"buffer_seconds": 30}), 3)
            .expect("valid refresh");
        let text = "# TYPE private_counter counter\nprivate_counter{pool=\"private\"} 1\n\n";
        assert_eq!(
            sink.scrape(&parse_families(text).unwrap()).unwrap(),
            text.as_bytes()
        );
        assert!(sink.plugin().is_open());
        assert_eq!(sink.status(), None);
        let unserved = sink
            .serve(&ServeRequest {
                method: "GET",
                path: "/metrics",
                query: None,
                headers: &[],
                body: &[],
            })
            .expect_err("an unserved host snapshot never yields bytes");
        assert!(
            unserved.contains("snapshot"),
            "the failure names the snapshot read: {unserved}"
        );
        let mut big = String::from("# TYPE big_total counter\n");
        for i in 0..4000 {
            big.push_str(&format!("big_total{{hook=\"h\",n=\"{i:020}\"}} {i}\n"));
        }
        let small = "# TYPE small gauge\nsmall{hook=\"h\"} 1\n";
        let hooks = format!("{big}{small}");
        assert!(hooks.len() > 64 * 1024);
        let mut families = parse_families(&format!("{big}\n{small}\n")).expect("snapshot");
        families.reverse();
        let before = sink.plugin().test_invocations().0;
        assert_eq!(sink.scrape_hooks(&families).unwrap(), hooks.as_bytes());
        assert_eq!(
            sink.plugin().test_invocations().0 - before,
            2,
            "one exact short-buffer recall"
        );
        let before = sink.plugin().test_invocations().0;
        let plain = sink.scrape(&families).unwrap();
        assert_eq!(sink.plugin().test_invocations().0 - before, 2);
        assert_eq!(
            String::from_utf8(plain).unwrap(),
            format!("{big}\n{small}\n")
        );
        assert_eq!(sink.scrape(&[]).unwrap(), b"");
        assert!(sink.plugin().is_open());
        let counts = sink.plugin().test_invocation_probe();
        let before_close = counts().0;
        drop(sink);
        assert_eq!(
            counts().0 - before_close,
            1,
            "dropping the actual sink crosses its CLOSE exactly once"
        );
    }
}

#[test]
fn declared_needs_and_requested_profile_match_the_actual_plugin() {
    let rendered = busbar_contract::abi::mechanism::rendering::read(&row().statement)
        .expect("the actual Statement renders");
    let declares: Value =
        serde_json::from_str(busbar_export_prometheus::DECLARES).expect("declares JSON");
    let mut actual: Vec<_> = rendered
        .needs
        .iter()
        .map(|need| need.transport.as_str())
        .collect();
    actual.sort_unstable();
    actual.dedup();
    let mut declared: Vec<_> = declares["needs"]
        .as_array()
        .expect("declared transport names")
        .iter()
        .map(|name| name.as_str().expect("transport name"))
        .collect();
    declared.sort_unstable();
    declared.dedup();
    assert_eq!(actual, declared);
    assert!(
        !(cfg!(debug_assertions) && std::env::var_os("BUSBAR_EXPECT_RELEASE").is_some()),
        "BUSBAR_EXPECT_RELEASE requires --release"
    );
}
