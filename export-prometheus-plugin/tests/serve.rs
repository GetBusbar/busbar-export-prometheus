// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! **THE SINK'S OWN ROUTES, BOTH DOORS** — `/metrics` and `/metrics/hooks` are the scrape sink's
//! listener needs, not the host's (`BUSBAR-1.6.0.md` Appendix A B.5, l.3829: "declared in its
//! `routes` list, served on the data listener behind the data key, and answered by its `serve`";
//! §5 l.626-628: "The path, the 503 boot window, the first-party gate, the content-type and the
//! series stay 1.5.5 bytes"): the Statement declares both behind the data key, and `serve` answers
//! them from the host snapshot service (`snapshot.read`) in 1.5.5's bytes. The 1.5.5 shadow-oracle
//! cells are `ops.scrape__metrics__key` (`200`, `content-type: text/plain; version=0.0.4`) and
//! `ops.scrape__metricshooks__key` (`200`, `content-type: text/plain; version=0.0.4;
//! charset=utf-8`, an empty body with no hook reporting). Driven through a real dispatcher whose
//! host services answer a scripted snapshot, LINKED and DROPPED IN; the two transcripts must be
//! equal.
//!
//! RED before the routes were declared and `serve` answered them: the sink declared no route and
//! refused every serve, so a build configuring it mounted neither path (busbar Q128 HIGH,
//! crates/busbar-export-prometheus/src door.rs:36-45, :189-203).
//!
//! THE RED ARMS, same file: before the recorder is installed the answer is `503` with
//! `Retry-After: 1` and no body (never an empty success); a read the host declines is FAILED (the
//! host answers `502`); a path the sink does not own is REFUSED.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use busbar_contract::abi::export::{self, ServeIn, ServeOut};
use busbar_contract::abi::mechanism::call::{AbiStr, Blob};
use busbar_contract::abi::mechanism::lifecycle::{slot as lc, OpenIn, OpenOut};
use busbar_contract::abi::sdk::door::{blank_in, blank_out};
use busbar_contract::export_calls::{parse_families, Family};
use busbar_contract::services::{
    Caller, DiskDest, HookAsk, HostServices, Later, NestAsk, Ran, Reading, RecordsList, Snapshot,
    Stored,
};
use busbar_plugin_loader::dispatch::kinds::export::{Export, ExportFacts};
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, ConnTable, DispatchConfig, Dispatcher,
    Frame, LinkedRow, NoSink, Plugin,
};
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
}

fn families(text: &str) -> Vec<Family> {
    parse_families(text).expect("an exposition")
}

impl HostServices for Host {
    fn snapshot_read(&self, _: &Caller, scope: u32) -> Snapshot {
        if *self.declined.lock().unwrap() {
            return Snapshot::Refused("not granted");
        }
        if !*self.ready.lock().unwrap() {
            return Snapshot::NotReady;
        }
        if scope != 0 && *self.no_hooks.lock().unwrap() {
            return Snapshot::Families(Vec::new());
        }
        Snapshot::Families(families(if scope == 0 { OWN } else { HOOKS }))
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

fn row() -> LinkedRow {
    LinkedRow::of(busbar_export_prometheus::door::door).expect("the linked door renders")
}

/// The door, linked or dropped in, opened on a dispatcher serving `host`.
fn opened(host: Arc<Host>, dropped: bool) -> (Arc<Dispatcher>, Plugin<Export>) {
    let d = Arc::new(Dispatcher::with_services(DispatchConfig::default(), host));
    let bind = Bind {
        instance: Arc::from("export.metrics"),
        max_inflight_cap: 64,
        sink: Arc::new(NoSink),
        dispatcher: d.adopter(),
        conns: ConnTable::NoNeeds,
    };
    let p = if dropped {
        load_dropped::<Export>(&cdylib(), &row().statement, bind).expect("the dropped door loads")
    } else {
        load_linked::<Export>(&row(), bind).expect("the linked door loads")
    };
    let settings = br#"{"buffer_seconds":60}"#;
    let mut input: OpenIn = blank_in();
    input.head = in_head();
    input.settings = Blob {
        ptr: settings.as_ptr(),
        len: settings.len(),
        fmt: busbar_contract::abi::mechanism::call::BLOB_JSON,
        flags: 0,
        ..Blob::ZERO
    };
    input.generation = 1;
    let mut out: OpenOut = blank_out();
    out.head = out_head();
    let called = p.call(lc::OPEN, &mut Frame::new(input, out));
    assert_eq!(format!("{:?}", called.outcome), "Ready");
    (d, p)
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
    let mut input: ServeIn = blank_in();
    input.head = in_head();
    input.method = AbiStr {
        ptr: b"GET".as_ptr(),
        len: 3,
        ..AbiStr::ZERO
    };
    input.path = AbiStr {
        ptr: path.as_ptr(),
        len: path.len(),
        ..AbiStr::ZERO
    };
    let mut out: ServeOut = blank_out();
    out.head = out_head();
    let mut f = Frame::new(input, out);
    let called = p.call(export::slot::SERVE, &mut f);
    let headers: Vec<String> = if f.out.headers_out.is_null() {
        Vec::new()
    } else {
        // SAFETY: the plugin's own header list, live under its lease.
        unsafe { std::slice::from_raw_parts(f.out.headers_out, f.out.headers_out_len) }
            .iter()
            .map(|s| copy(s.ptr, s.len))
            .collect()
    };
    json!({
        "outcome": format!("{:?}", called.outcome),
        "status": f.out.status_code,
        "headers": headers,
        "body": copy(f.out.body.ptr, f.out.body.len),
    })
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
    let not_ready = get(&p, "/metrics");
    *host.ready.lock().unwrap() = true;
    let metrics = get(&p, "/metrics");
    let hooks = get(&p, "/metrics/hooks");
    *host.no_hooks.lock().unwrap() = true;
    let hooks_empty = get(&p, "/metrics/hooks");
    let elsewhere = get(&p, "/exports/metrics/x");
    *host.declined.lock().unwrap() = true;
    let declined = get(&p, "/metrics");
    json!({
        "routes": routes,
        "not_ready": not_ready,
        "metrics": metrics,
        "hooks": hooks,
        "hooks_empty": hooks_empty,
        "elsewhere": elsewhere["outcome"],
        "declined": declined["outcome"],
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
    assert_eq!(
        linked["elsewhere"], "Refused",
        "a path the sink does not own"
    );
    assert_eq!(
        linked["declined"], "Failed",
        "a declined read: the host answers 502"
    );
}
