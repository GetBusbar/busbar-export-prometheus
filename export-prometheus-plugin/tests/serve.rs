// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! **THE SINK'S OWN ROUTES, BOTH DOORS** — `/metrics` and `/metrics/hooks` are the scrape sink's
//! listener needs, not the host's: the Statement declares both behind the data key, and `serve`
//! answers them from the host snapshot service (`snapshot.read`) in 1.5.5's bytes, LINKED and
//! DROPPED IN.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use busbar_contract::abi::export::{self, ServeIn, ServeOut};
use busbar_contract::abi::host::service::{SNAPSHOT_SCOPE_HOOKS, SNAPSHOT_SCOPE_WHOLE};
use busbar_contract::abi::mechanism::call::{AbiStr, Blob, Outcome};
use busbar_contract::abi::mechanism::lifecycle::{slot as lc, OpenIn, OpenOut};
use busbar_contract::abi::sdk::door::{blank_in, blank_out};
use busbar_contract::export_calls::{parse_families, ExportCalls, Family, ServeRequest};
use busbar_contract::services::HostServices;
use busbar_contract::services::{
    Caller, DiskDest, HookAsk, Later, NestAsk, Ran, Reading, RecordsList, Snapshot, Stored,
};
use busbar_plugin_loader::dispatch::kinds::export::{Export, ExportFacts};
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, Budgets, ConnTable, DispatchConfig,
    Dispatcher, Frame, LinkedRow, NoSink, Plugin,
};
use busbar_plugin_loader::export_door::ExportInstance;
use serde_json::{json, Value};

/// The recorder's exposition the whole snapshot holds.
const OWN: &str = "# TYPE busbar_requests_total counter\n\
                   busbar_requests_total{pool=\"a\\\"b\",outcome=\"ok\"} 3\n\
                   \n\
                   # HELP busbar_request_duration_seconds request latency\n\
                   # TYPE busbar_request_duration_seconds histogram\n\
                   busbar_request_duration_seconds_bucket{le=\"+Inf\"} 2\n\
                   busbar_request_duration_seconds_sum 0.75\n\
                   busbar_request_duration_seconds_count 2\n\
                   \n";

/// The hook families: one hook's counter, as the host folds it.
const HOOKS: &str = "# HELP rr_hits hits\n# TYPE rr_hits counter\nrr_hits{hook=\"rr\"} 4\n\n";

/// The host's services: a scripted snapshot (`None` = the recorder is not installed; the scope
/// picks the whole or the hook families), and nothing else served.
#[derive(Default)]
struct Host {
    ready: Mutex<bool>,
    declined: Mutex<bool>,
    /// No hook reports a metric: the hook families are empty.
    no_hooks: Mutex<bool>,
    /// When set, the exposition the whole snapshot (scope 0) holds instead of `OWN`.
    whole: Mutex<Option<String>>,
    /// How many `snapshot.read` calls the host has served, short answers included.
    reads: AtomicUsize,
    scopes: Mutex<Vec<u32>>,
    /// When set, the whole snapshot is `step` samples per read so far (the first read `step`, the second `2 * step`, ...)
    /// (a recorder that outgrows any buffer sized from the read before).
    growing: Mutex<Option<usize>>,
}

fn families(text: &str) -> Vec<Family> {
    parse_families(text).expect("an exposition")
}

impl HostServices for Host {
    fn snapshot_read(&self, _: &Caller, scope: u32, _later: Later) -> Ran {
        let snapshot = (|| {
            self.scopes.lock().unwrap().push(scope);
            let read = self.reads.fetch_add(1, Ordering::SeqCst);
            if let Some(step) = *self.growing.lock().unwrap() {
                return Snapshot::Families(families(&big_exposition(step * (read + 1))));
            }
            if *self.declined.lock().unwrap() {
                return Snapshot::Refused("not granted");
            }
            if !*self.ready.lock().unwrap() {
                return Snapshot::NotReady;
            }
            if scope != 0 && *self.no_hooks.lock().unwrap() {
                return Snapshot::Families(Vec::new());
            }
            if scope == 0 {
                if let Some(text) = self.whole.lock().unwrap().as_deref() {
                    return Snapshot::Families(families(text));
                }
            }
            Snapshot::Families(families(if scope == 0 { OWN } else { HOOKS }))
        })();
        Ran::Now(snapshot.stored())
    }
    fn now(&self) -> Reading {
        Reading {
            wall_ns: 0,
            mono_ns: 0,
        }
    }
    fn dest_judge(&self, _: &str, _: u32, _: u32, _: Option<Later>) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn records_get(&self, _: &Caller, _: &str, _: &[u8], _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn records_list(&self, _: &Caller, _: RecordsList, _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn records_claim(&self, _: &Caller, _: &str, _: &[u8], _: u64, _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn sign(&self, _: &Caller, _: &[u8]) -> Stored {
        Stored::refused("no")
    }
    fn trust_sight(&self, _: &Caller, _: &str, _: &str, _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn trust_unreached(&self, _: &Caller, _: &str) -> Stored {
        Stored::refused("unimplemented")
    }
    fn trust_sight_item(&self, _: &Caller, _: &str, _: &str, _: &str, _later: Later) -> Ran {
        Ran::Now(Stored::refused("unimplemented"))
    }
    fn trust_decide(
        &self,
        _: &Caller,
        _ticket: busbar_contract::abi::mechanism::ticket::Ticket,
        _: busbar_contract::services::TrustKeyRef<'_>,
        _: Option<&str>,
        _: bool,
        _later: Later,
    ) -> Ran {
        Ran::Now(Stored::refused("unimplemented"))
    }
    fn trust_state(&self, _: &Caller, _: &str) -> Stored {
        Stored::refused("unimplemented")
    }
    fn trust_serves(&self, _: &Caller, _: &str, _: Option<&str>, _: Option<&str>) -> Stored {
        Stored::refused("unimplemented")
    }
    fn session_emit(&self, _: &Caller, _: u64, _: &[u8]) -> Stored {
        Stored::refused("unimplemented")
    }
    fn ticket_mint(&self, _: &Caller, _: Option<u64>, _: u64, _: &[u8], _later: Later) -> Ran {
        Ran::Now(Stored::refused("unimplemented"))
    }
    fn ticket_take(&self, _: &Caller, _: Option<u64>) -> Stored {
        Stored::refused("unimplemented")
    }
    fn trust_due(&self, _: &Caller) -> Stored {
        Stored::refused("no")
    }
    fn trust_verify(&self, _: &Caller, _: &str, _: &[u8], _: &[u8]) -> Stored {
        Stored::refused("no")
    }
    fn entitlement_check(&self, _: &Caller, _: Option<u64>, _: &str) -> Stored {
        Stored::refused("no")
    }
    fn random_fill(&self, _: u64) -> Stored {
        Stored::refused("no")
    }
    fn records_secret(&self, _: &str, _: &str, _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn unit_nest(&self, _: &Caller, _: Option<u64>, _: NestAsk, _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn work_open(&self, _: &Caller, _: Option<u64>, _: &str, _: &[u8], _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn work_find(&self, _: &Caller, _: Option<u64>, _: &[u8], _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn work_settle(&self, _: &Caller, _: Option<u64>, _: u64, _: &[u8], _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn work_resume(&self, _: &Caller, _: Option<u64>, _: u64, _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn verify_lookup(&self, _: &Caller, _: &[u8], _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn verify_store(&self, _: &Caller, _: &[u8], _: &[u8], _: u64) -> Stored {
        Stored::refused("no")
    }
    fn content_scan(&self, _: &Caller, _: Option<u64>, _: &[u8], _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn hook_call(&self, _: &Caller, _: Option<u64>, _: HookAsk, _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
    fn disk_append(&self, _: &DiskDest, _: Vec<u8>, _: Later) -> Ran {
        Ran::Now(Stored::refused("no"))
    }
}

#[path = "support/cdylib.rs"]
mod artifact;
use artifact::cdylib;

#[path = "support/inbound.rs"]
mod inbound;

#[test]
fn the_listener_need_names_the_existing_data_binding() {
    let rendered = busbar_contract::abi::mechanism::rendering::read(&row().statement)
        .expect("the Statement reads");
    assert_eq!(rendered.needs.len(), 1);
    let need = &rendered.needs[0];
    assert_eq!(
        need.direction,
        busbar_contract::abi::host::conn::connector::DIRECTION_INBOUND
    );
    assert_eq!(need.transport, "http");
    assert_eq!(need.target_from, "listen");
    assert!(need.auth.is_empty());
}

fn row() -> LinkedRow {
    LinkedRow::of(busbar_export_prometheus::door).expect("the linked door renders")
}

fn loaded(host: Arc<Host>, dropped: bool) -> (Arc<Dispatcher>, Plugin<Export>) {
    loaded_with_config(host, dropped, DispatchConfig::default())
}

fn loaded_with_config(
    host: Arc<Host>,
    dropped: bool,
    config: DispatchConfig,
) -> (Arc<Dispatcher>, Plugin<Export>) {
    let d = Arc::new(Dispatcher::with_services(
        config,
        host,
        busbar_contract::services::test_caller_binding(),
    ));
    let bind = Bind {
        instance: Arc::from("export.metrics"),
        max_inflight_cap: 64,
        sink: Arc::new(NoSink),
        dispatcher: d.adopter(),
        conns: ConnTable::Host(inbound::host(d.conn_waker(), None)),
    };
    let p = if dropped {
        load_dropped::<Export>(&cdylib(), &row().statement, bind).expect("the dropped door loads")
    } else {
        load_linked::<Export>(&row(), bind).expect("the linked door loads")
    };
    (d, p)
}

/// The door, linked or dropped in, opened on a dispatcher serving `host`.
fn opened(host: Arc<Host>, dropped: bool) -> (Arc<Dispatcher>, Plugin<Export>) {
    opened_with_config(host, dropped, DispatchConfig::default())
}

fn opened_with_config(
    host: Arc<Host>,
    dropped: bool,
    config: DispatchConfig,
) -> (Arc<Dispatcher>, Plugin<Export>) {
    let (d, p) = loaded_with_config(host, dropped, config);
    let settings = br#"{"buffer_seconds":60}"#;
    let mut input: OpenIn = blank_in();
    input.head = in_head();
    input.settings = Blob {
        ptr: settings.as_ptr(),
        len: settings.len(),
        fmt: busbar_contract::abi::mechanism::call::BLOB_JSON,
        flags: 0,
    };
    input.generation = 1;
    let mut out: OpenOut = blank_out();
    out.head = out_head();
    let called = p.call(lc::OPEN, &mut Frame::new(input, out));
    assert_eq!(format!("{:?}", called.outcome), "Ready");
    (d, p)
}

#[test]
fn the_host_calls_keep_the_declared_route_policy_and_serve_both_scopes() {
    for dropped in [false, true] {
        let host = Arc::new(Host::default());
        let (d, p) = loaded(host.clone(), dropped);
        let instance = ExportInstance::open(p, d, br#"{"buffer_seconds":60}"#)
            .expect("the export instance opens");
        assert_eq!(instance.public_routes(), ["/metrics", "/metrics/hooks"]);
        assert_eq!(instance.persistent_routes(), ["/metrics/hooks"]);
        for (path, content_type, body) in [
            ("/metrics", "text/plain; version=0.0.4", OWN),
            (
                "/metrics/hooks",
                "text/plain; version=0.0.4; charset=utf-8",
                "# HELP rr_hits hits\n# TYPE rr_hits counter\nrr_hits{hook=\"rr\"} 4\n",
            ),
        ] {
            let request = ServeRequest {
                method: "GET",
                path,
                query: None,
                headers: &[],
                body: &[],
            };
            *host.ready.lock().unwrap() = false;
            let not_ready = instance.serve(&request).expect("the boot answer");
            assert_eq!(not_ready.status, 503, "dropped={dropped}, {path}");
            assert_eq!(
                not_ready.headers,
                [(String::from("retry-after"), String::from("1"))]
            );
            assert!(not_ready.body.is_empty());
            *host.ready.lock().unwrap() = true;
            let ready = instance.serve(&request).expect("the recorder answer");
            assert_eq!(ready.status, 200, "dropped={dropped}, {path}");
            assert_eq!(
                ready.headers,
                [(String::from("content-type"), content_type.to_string())]
            );
            assert_eq!(ready.body, body.as_bytes(), "dropped={dropped}, {path}");
        }
        assert_eq!(
            *host.scopes.lock().unwrap(),
            [
                SNAPSHOT_SCOPE_WHOLE,
                SNAPSHOT_SCOPE_WHOLE,
                SNAPSHOT_SCOPE_HOOKS,
                SNAPSHOT_SCOPE_HOOKS,
            ]
        );
    }
}

/// `bytes` a plugin answered under its lease, copied.
fn copy(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    // SAFETY: the plugin's own answer, live under its lease until released.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(ptr, len) }).into_owned()
}

/// `GET path` through `serve`: the outcome, status, headers and body.
fn get(p: &Plugin<Export>, path: &str) -> Value {
    serve(p, path).0
}

/// `GET path` through `serve`: the transcript, and the error text the answer named, if any.
fn serve(p: &Plugin<Export>, path: &str) -> (Value, Option<String>) {
    let mut input: ServeIn = blank_in();
    input.head = in_head();
    input.method = AbiStr {
        ptr: b"GET".as_ptr(),
        len: 3,
    };
    input.path = AbiStr {
        ptr: path.as_ptr(),
        len: path.len(),
    };
    let mut out: ServeOut = blank_out();
    out.head = out_head();
    let mut f = Frame::new(input, out);
    let called = p.call(export::slot::SERVE, &mut f);
    let headers: Vec<String> = if called.outcome != Outcome::Ready || f.out.headers_out.is_null() {
        Vec::new()
    } else {
        // SAFETY: the plugin's own header list, live under its lease.
        unsafe { std::slice::from_raw_parts(f.out.headers_out, f.out.headers_out_len) }
            .iter()
            .map(|s| copy(s.ptr, s.len))
            .collect()
    };
    let error = called
        .error
        .as_ref()
        .map(|e| String::from_utf8_lossy(e).into_owned());
    (
        json!({
            "outcome": format!("{:?}", called.outcome),
            "status": f.out.status_code,
            "headers": headers,
            "body": if called.outcome == Outcome::Ready {
                copy(f.out.body.ptr, f.out.body.len)
            } else {
                String::new()
            },
        }),
        error,
    )
}

/// One door's script.
fn transcript(dropped: bool) -> Value {
    let host = Arc::new(Host::default());
    let (_d, p) = opened(host.clone(), dropped);
    let routes: Vec<String> = p
        .context::<ExportFacts>()
        .expect("export facts")
        .routes
        .iter()
        .map(|r| format!("{} {} {:?}", r.method.as_str(), r.path, r.auth))
        .collect();
    let policy = p.context::<ExportFacts>().expect("export facts");
    let public_routes = policy.public_routes.clone();
    let persistent_routes = policy.persistent_routes.clone();
    let not_ready = get(&p, "/metrics");
    let hooks_not_ready = get(&p, "/metrics/hooks");
    *host.ready.lock().unwrap() = true;
    let metrics = get(&p, "/metrics");
    let hooks = get(&p, "/metrics/hooks");
    *host.no_hooks.lock().unwrap() = true;
    let hooks_empty = get(&p, "/metrics/hooks");
    let elsewhere = serve(&p, "/exports/metrics/x");
    *host.declined.lock().unwrap() = true;
    let declined = serve(&p, "/metrics");
    let snapshot_scopes = host.scopes.lock().unwrap().clone();
    json!({
        "routes": routes,
        "public_routes": public_routes,
        "persistent_routes": persistent_routes,
        "not_ready": not_ready,
        "hooks_not_ready": hooks_not_ready,
        "metrics": metrics,
        "hooks": hooks,
        "hooks_empty": hooks_empty,
        "elsewhere": elsewhere.0["outcome"],
        "elsewhere_error": elsewhere.1,
        "declined": declined.0["outcome"],
        "declined_error": declined.1,
        "snapshot_scopes": snapshot_scopes,
    })
}

#[test]
fn the_sink_serves_its_own_routes_from_the_host_snapshot_through_both_doors() {
    let linked = transcript(false);
    assert_eq!(linked, transcript(true), "the two doors are not one sink");
    assert_eq!(
        linked["routes"],
        json!(["GET /metrics Key", "GET /metrics/hooks Key"])
    );
    assert_eq!(
        linked["public_routes"],
        json!(["/metrics", "/metrics/hooks"])
    );
    assert_eq!(linked["persistent_routes"], json!(["/metrics/hooks"]));
    assert_eq!(
        linked["metrics"],
        json!({"outcome": "Ready", "status": 200,
               "headers": ["content-type", "text/plain; version=0.0.4"], "body": OWN}),
        "the whole snapshot, 1.5.5's bytes and content type"
    );
    assert_eq!(
        linked["hooks"],
        json!({"outcome": "Ready", "status": 200,
               "headers": ["content-type", "text/plain; version=0.0.4; charset=utf-8"],
               "body": "# HELP rr_hits hits\n# TYPE rr_hits counter\nrr_hits{hook=\"rr\"} 4\n"}),
        "the hook families, 1.5.5's hook exposition and its own content type"
    );
    assert_eq!(
        linked["hooks_empty"],
        json!({"outcome": "Ready", "status": 200,
               "headers": ["content-type", "text/plain; version=0.0.4; charset=utf-8"],
               "body": ""}),
        "no hook reporting: 1.5.5's oracle cell ops.scrape__metricshooks__key, an empty 200"
    );
    assert_eq!(
        linked["not_ready"],
        json!({"outcome": "Ready", "status": 503, "headers": ["retry-after", "1"], "body": ""}),
        "not ready: refused with a retry, never an empty success"
    );
    assert_eq!(linked["hooks_not_ready"], linked["not_ready"]);
    assert_eq!(
        linked["snapshot_scopes"],
        json!([
            SNAPSHOT_SCOPE_WHOLE,
            SNAPSHOT_SCOPE_HOOKS,
            SNAPSHOT_SCOPE_WHOLE,
            SNAPSHOT_SCOPE_HOOKS,
            SNAPSHOT_SCOPE_HOOKS,
            SNAPSHOT_SCOPE_WHOLE,
        ]),
        "each admitted route reads only its own snapshot scope"
    );
    assert_eq!(
        linked["elsewhere"], "Refused",
        "a path the sink does not own"
    );
    assert_eq!(
        linked["declined"], "Failed",
        "a declined read: the host answers 502"
    );
    assert_eq!(
        linked["elsewhere_error"],
        "not a route this sink serves (only GET /metrics and /metrics/hooks)",
        "a refusal names its cause"
    );
    assert_eq!(
        linked["declined_error"], "the host refused the snapshot read (scope WHOLE)",
        "a failure names its cause, so the operator's 502 is not bare"
    );
}

/// One counter family of `n` labelled samples, in the sink's stable order.
fn big_exposition(n: usize) -> String {
    let mut text = String::from("# TYPE busbar_big_total counter\n");
    for i in 0..n {
        text.push_str(&format!(
            "busbar_big_total{{pool=\"pool-{i:06}\",outcome=\"ok\"}} {i}\n"
        ));
    }
    text.push('\n');
    text
}

/// A snapshot larger than the 64 KiB first read buffer: the host answers `Short` with its size,
/// the door re-calls once with headroom and serves the whole body; the second read starts from the
/// stored hint. RED when the `Short` arm is gone: a real recorder snapshot over 64 KiB made
/// `GET /metrics` FAILED (the host answers `502`).
#[test]
fn a_snapshot_larger_than_the_first_read_buffer_is_served_whole_through_both_doors() {
    let big = big_exposition(3000);
    assert!(big.len() > 100 * 1024, "the fixture must exceed 64 KiB");
    for dropped in [false, true] {
        let host = Arc::new(Host::default());
        *host.ready.lock().unwrap() = true;
        *host.whole.lock().unwrap() = Some(big.clone());
        let (_d, p) = opened(host.clone(), dropped);
        let mut calls = Vec::new();
        for read in ["first", "second, from the stored hint"] {
            let before = host.reads.load(Ordering::SeqCst);
            assert_eq!(
                get(&p, "/metrics"),
                json!({"outcome": "Ready", "status": 200,
                       "headers": ["content-type", "text/plain; version=0.0.4"], "body": big}),
                "dropped={dropped}, {read} read"
            );
            calls.push(host.reads.load(Ordering::SeqCst) - before);
        }

        assert_eq!(calls, [2, 1], "dropped={dropped}: snapshot reads per GET");
    }
}

/// A snapshot that outgrows the buffer even on the one re-call: the host answers `Short` twice, and
/// the door FAILS the serve naming the cause (the host answers `502`), reading exactly twice.
#[test]
fn a_snapshot_that_outgrows_the_one_re_call_is_failed_through_both_doors() {
    for dropped in [false, true] {
        let host = Arc::new(Host::default());
        *host.ready.lock().unwrap() = true;

        *host.growing.lock().unwrap() = Some(1500);
        let (_d, p) = opened(host.clone(), dropped);
        let (answer, error) = serve(&p, "/metrics");
        assert_eq!(answer["outcome"], "Failed", "dropped={dropped}");
        assert_eq!(
            error.as_deref(),
            Some("the snapshot outgrew the buffer on the one re-call (scope WHOLE)"),
            "dropped={dropped}"
        );
        assert_eq!(host.reads.load(Ordering::SeqCst), 2, "one re-call, no more");
    }
}

#[test]
fn route_prefixes_do_not_admit_near_misses_through_either_door() {
    for dropped in [false, true] {
        let host = Arc::new(Host::default());
        *host.ready.lock().unwrap() = true;
        let (_d, p) = opened(host.clone(), dropped);
        for path in ["/metrics/", "/metricsx", "/metrics/hooks/x"] {
            assert_eq!(
                serve(&p, path).0["outcome"],
                "Refused",
                "{path}, dropped={dropped}"
            );
        }
        assert_eq!(host.reads.load(Ordering::SeqCst), 0);
        assert_eq!(get(&p, "/metrics")["status"], 200);
    }
}

#[test]
fn refused_snapshot_reads_name_the_scope_through_either_door() {
    for dropped in [false, true] {
        let host = Arc::new(Host::default());
        *host.declined.lock().unwrap() = true;
        let (_d, p) = opened(host, dropped);
        for (path, scope) in [("/metrics", "WHOLE"), ("/metrics/hooks", "HOOKS")] {
            let (answer, error) = serve(&p, path);
            assert_eq!(answer["outcome"], "Failed");
            assert_eq!(
                error,
                Some(format!(
                    "the host refused the snapshot read (scope {scope})"
                ))
            );
        }
    }
}

#[test]
fn an_exposition_above_the_existing_answer_limit_fails_without_exposing_its_labels() {
    use busbar_contract::abi::mechanism::check::HARD_MAX_BYTES;
    const PREFIX: &str = "# TYPE big counter\nbig{label=\"";
    const SUFFIX: &str = "\"} 1\n\n";
    const LABEL: &str = "private-metric-label-canary";
    let limit = usize::try_from(HARD_MAX_BYTES).expect("the ABI limit fits this target");
    for size in [limit - 1, limit, limit + 1] {
        let mut text = String::from(PREFIX);
        text.push_str(LABEL);
        text.extend(std::iter::repeat_n(
            'x',
            size - PREFIX.len() - LABEL.len() - SUFFIX.len(),
        ));
        text.push_str(SUFFIX);
        assert_eq!(text.len(), size);
        for dropped in [false, true] {
            let host = Arc::new(Host::default());
            *host.ready.lock().unwrap() = true;
            *host.whole.lock().unwrap() = Some(text.clone());

            let config = DispatchConfig {
                budgets: Budgets {
                    call: Duration::from_secs(30),
                    ..Budgets::default()
                },
                ..DispatchConfig::default()
            };
            let (_d, p) = opened_with_config(host, dropped, config);
            let (answer, error) = serve(&p, "/metrics");
            assert_eq!(
                _d.stats().quarantined,
                0,
                "the byte-limit crossing completes"
            );
            if size <= limit {
                assert_eq!(
                    answer["outcome"],
                    "Ready",
                    "size={size}, dropped={dropped}, dispatcher={:?}",
                    _d.stats()
                );
                assert_eq!(answer["status"], 200);
                assert!(
                    answer["body"].as_str() == Some(text.as_str()),
                    "size={size}, dropped={dropped}"
                );
                assert!(error.is_none());
            } else {
                assert_eq!(answer["outcome"], "Failed", "dropped={dropped}");
                assert_eq!(answer["body"], "");
                let error = error.expect("the bounded failure names its cause");
                assert!(error.contains("answer limit"), "{error}");
                assert!(
                    !error.contains(LABEL),
                    "the metric value is not diagnostic text"
                );
            }
        }
    }
}
