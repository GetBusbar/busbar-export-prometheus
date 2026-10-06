// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! **ONE SCRAPE SINK, BOTH DOORS, ONE TABLE** — the prometheus sink's linked + dropped-in
//! conformance on the export kind's memory ABI (THE DESIGN §11.4), run against the busbar rev this
//! repo pins (`.busbar-ref`).
//!
//! The sink is held two ways at once: LINKED (the logic crate's `door::door`, through the loader's
//! `load_linked`) and DROPPED IN (this crate's built cdylib, `dlopen`ed by the loader's
//! `load_dropped`, which resolves `busbar_plugin_door` and compares its Statement with the linked
//! row's, byte for byte). Each is bound to a real dispatcher and driven over one script through the
//! export kind's table: its settings refusals (the configuration's own words, the zero-retention
//! refusal included), open, the kind's other answers, and scrapes — the recorder snapshot of an
//! exposition carrying every family type the recorder writes, lent as `ScrapeIn::families` in the
//! recorder's order and rendered into the host's buffer; the same snapshot in another order; and a
//! buffer too small, answered with the bytes needed and then, on the ONE re-call, rendered. The two
//! transcripts must be equal, and the bytes rendered must be the recorder's own.
//!
//! THE RED ARMS, same file: the door asked for as another kind is refused, by either origin; a
//! manifest stating 1.5.5's export ABI (2) is refused before `dlopen`. A missing cdylib PANICS —
//! this test IS the dropped-in door's proof, and never skips.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use busbar_contract::abi::export::{
    self, CheckIn, CheckOut, DeliverIn, ScrapeFamily, ScrapeIn, ScrapeLabel, ScrapeOut,
    ScrapeSample, ServeIn, ServeOut, StatusOut,
};
use busbar_contract::abi::mechanism::call::{AbiStr, Blob, InHead, OutHead, BLOB_JSON};
use busbar_contract::abi::mechanism::lifecycle::{slot as lc, OpenIn, OpenOut, ValidateIn};
use busbar_contract::abi::mechanism::rendering::RENDERING_MAGIC;
use busbar_contract::abi::sdk::door::{blank_in, blank_out};
use busbar_contract::abi::sdk::{MetricFamily, MetricSample};
use busbar_plugin_loader::dispatch::kinds::export::Export;
use busbar_plugin_loader::dispatch::kinds::secret::Secret;
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, Called, DispatchConfig, Dispatcher, Frame,
    LinkedRow, LoadError, NoSink, Plugin, NO_BLOB,
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

/// This crate's built cdylib (uplifted or under `deps`, newest wins). A missing artifact is a
/// failure, never a skip.
fn cdylib() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary has a path");
    let profile = exe
        .parent()
        .and_then(|d| d.parent())
        .expect("target/<profile>");
    let file = busbar_plugin_loader::plugin_library_filename("busbar_export_prometheus_plugin");
    [profile.join(&file), profile.join("deps").join(&file)]
        .into_iter()
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max()
        .map(|(_, p)| p)
        .unwrap_or_else(|| {
            panic!("the busbar-export-prometheus-plugin cdylib ({file}) is not built")
        })
}

/// The row a busbar build that links the sink states.
fn row() -> LinkedRow {
    LinkedRow::of(busbar_export_prometheus::door::door).expect("the linked door renders")
}

fn dispatcher() -> Arc<Dispatcher> {
    Arc::new(Dispatcher::new(DispatchConfig {
        workers: 2,
        watchdog_period: Duration::from_millis(20),
        ..DispatchConfig::default()
    }))
}

fn bind(d: &Dispatcher) -> Bind {
    Bind {
        instance: Arc::from("metrics"),
        max_inflight_cap: 64,
        sink: Arc::new(NoSink),
        dispatcher: d.adopter(),
        conns: None,
    }
}

/// The door, linked or dropped in.
fn load(d: &Dispatcher, dropped: bool) -> Plugin<Export> {
    if dropped {
        load_dropped::<Export>(&cdylib(), &row().statement, bind(d))
            .expect("the dropped-in door loads")
    } else {
        load_linked::<Export>(&row(), bind(d)).expect("the linked door loads")
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

/// The recorder snapshot of `exposition`, as the host's snapshot reader takes it.
fn to_contract(exposition: &str) -> Vec<MetricFamily> {
    let families = busbar_contract::export_calls::parse_families(exposition)
        .expect("the exposition snapshots");
    families
        .iter()
        .map(|f| MetricFamily {
            name: f.name.clone(),
            kind: busbar_contract::export_calls::type_word(f.kind)
                .unwrap_or("untyped")
                .to_string(),
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
        json!({"deliver": "Ready ", "status": "Ready ", "check": "Ready ", "serve": "Refused "})
    );
    assert_eq!(
        linked["scrape"],
        json!({"answer": "Ready ", "written": OWN.len(), "needed": 0, "text": OWN, "recall": null}),
        "the scrape is the recorder's bytes, back"
    );
    assert_eq!(
        linked["scrape_reordered"]["text"], OWN,
        "the same snapshot in another order renders the same bytes"
    );
    assert_eq!(
        linked["scrape_short"],
        json!({
            "answer": "Failed ", "written": 0, "needed": OWN.len(), "text": "",
            "recall": ["Ready ", OWN],
        }),
        "a short buffer is answered with the bytes needed, then rendered on the one re-call"
    );
    assert_eq!(
        linked["scrape_empty"],
        json!({"answer": "Ready ", "written": 0, "needed": 0, "text": "", "recall": null}),
        "an empty recorder renders nothing"
    );
    assert_eq!(linked["close"], "Ready ");
}

/// RED: the door asked for as another kind is refused, by either origin, before any slot runs.
#[test]
fn a_wrong_kind_is_refused() {
    let d = dispatcher();
    let err =
        load_linked::<Secret>(&row(), bind(&d)).expect_err("an export door is not a secret door");
    assert!(
        matches!(
            err,
            LoadError::WrongKind { .. } | LoadError::StatementMismatch
        ),
        "{err:?}"
    );
    let err = load_dropped::<Secret>(&cdylib(), &row().statement, bind(&d))
        .expect_err("the dropped-in export door is not a secret door");
    assert!(matches!(err, LoadError::ManifestKind { .. }), "{err:?}");
}

/// RED: a manifest stating 1.5.5's export ABI (2) is refused before `dlopen` (THE DESIGN §11.8).
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

// THE PUBLISHED CONFORMANCE SUITE, RUN BY THIS PLUGIN (busbar's loader, at the commit this repo pins):
// the sink driven two ways through the one loader over the export kind's script with the inputs in
// `conformance.json`, every step's crossings exactly at the script's pin, the two folds equal, and
// the suite's RED arms kept.
busbar_plugin_loader::conformance_suite! {
    door: busbar_export_prometheus::door::door,
    cdylib: "busbar_export_prometheus_plugin",
    inputs: include_str!("conformance.json"),
}
