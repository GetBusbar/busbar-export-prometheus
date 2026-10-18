// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! **ONE SCRAPE SINK, BOTH DOORS** — a second `module: prometheus` instance is refused in 1.5.5's
//! words (`BUSBAR-1.6.0.md` §2: "A port keeps 1.5.5's surface ... refusal texts"; the 1.5.5
//! shadow-oracle cell `boot.refusal__BOOT-102__validate`: "export.metrics2: a second `module:
//! prometheus` instance (already defined as 'metrics'). Prometheus serves the ONE well-known
//! /metrics route, so a second instance could only be silently ignored — keep a single
//! instance."). The Statement states the `one_instance` mark, which makes the host ask the sink's
//! LIMITS-phase `check` while the configuration is resolved, and that check answers one line per
//! instance after the first, naming the first; the INSTANCES phase answers nothing.
//!
//! RED before the fix: the Statement stated no mark and `check` answered nothing, so a second
//! instance was accepted and silently kept as a sink that is not the scrape sink.

use std::path::PathBuf;
use std::sync::Arc;

use busbar_contract::abi::export::{
    self, CheckIn, CheckInstance, CheckOut, CHECK_PHASE_INSTANCES, CHECK_PHASE_LIMITS,
};
use busbar_contract::abi::mechanism::call::{AbiStr, Blob, BLOB_JSON};
use busbar_contract::abi::mechanism::lifecycle::{slot as lc, OpenIn, OpenOut};
use busbar_contract::abi::sdk::door::{blank_in, blank_out};
use busbar_plugin_loader::dispatch::kinds::export::{Export, ExportFacts};
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, ConnTable, DispatchConfig, Dispatcher,
    Frame, LinkedRow, NoSink, Plugin, NO_BLOB,
};

/// 1.5.5's refusal of `second`, the first instance being `first` (cell BOOT-102's bytes).
fn refusal(second: &str, first: &str) -> String {
    format!(
        "export.{second}: a second `module: prometheus` instance (already defined as '{first}'). \
         Prometheus serves the ONE well-known /metrics route, so a second instance could only be \
         silently ignored \u{2014} keep a single instance."
    )
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

/// The door, linked or dropped in, opened on its settings.
fn opened(d: &Arc<Dispatcher>, dropped: bool) -> Plugin<Export> {
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
        fmt: BLOB_JSON,
        flags: 0,
    };
    input.generation = 1;
    let mut out: OpenOut = blank_out();
    out.head = out_head();
    let called = p.call(lc::OPEN, &mut Frame::new(input, out));
    assert_eq!(format!("{:?}", called.outcome), "Ready");
    p
}

/// `check` at `phase` over the instances `names` (each with the same settings): the findings.
fn check(p: &Plugin<Export>, phase: u32, names: &[&str]) -> Vec<String> {
    let settings = br#"{"buffer_seconds":60}"#;
    let instances: Vec<CheckInstance> = names
        .iter()
        .map(|n| CheckInstance {
            name: AbiStr {
                ptr: n.as_ptr(),
                len: n.len(),
            },
            settings: Blob {
                ptr: settings.as_ptr(),
                len: settings.len(),
                fmt: BLOB_JSON,
                flags: 0,
            },
        })
        .collect();
    let mut input: CheckIn = blank_in();
    input.head = in_head();
    input.phase = phase;
    input.instances = instances.as_ptr();
    input.instances_len = instances.len();
    let out = CheckOut {
        head: out_head(),
        findings: NO_BLOB,
    };
    let mut f = Frame::new(input, out);
    let called = p.call(export::slot::CHECK, &mut f);
    assert_eq!(format!("{:?}", called.outcome), "Ready");
    if f.out.findings.ptr.is_null() || f.out.findings.len == 0 {
        return Vec::new();
    }
    // SAFETY: the plugin's own findings, live under its lease until released.
    let bytes = unsafe { std::slice::from_raw_parts(f.out.findings.ptr, f.out.findings.len) };
    serde_json::from_slice(bytes).expect("findings are a JSON list of lines")
}

#[test]
fn a_second_instance_is_refused_in_1_5_5_words_through_both_doors() {
    let d = Arc::new(Dispatcher::new(DispatchConfig::default()));
    for dropped in [false, true] {
        let p = opened(&d, dropped);
        let facts = p.context::<ExportFacts>().expect("export facts");
        assert!(
            facts.one_instance,
            "the Statement states the one_instance mark"
        );
        assert!(check(&p, CHECK_PHASE_LIMITS, &[]).is_empty());
        assert!(check(&p, CHECK_PHASE_LIMITS, &["metrics"]).is_empty());
        assert_eq!(
            check(&p, CHECK_PHASE_LIMITS, &["metrics", "metrics2"]),
            vec![refusal("metrics2", "metrics")],
            "cell BOOT-102's line"
        );
        assert_eq!(
            check(&p, CHECK_PHASE_LIMITS, &["metrics", "again", "third"]),
            vec![refusal("again", "metrics"), refusal("third", "metrics")]
        );
        assert!(
            check(&p, CHECK_PHASE_INSTANCES, &["metrics", "metrics2"]).is_empty(),
            "the refusal is a limits-phase line only"
        );
    }
}
