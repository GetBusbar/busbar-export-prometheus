// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

use std::sync::Arc;

use busbar_contract::abi::export::{CheckIn, CheckOut, CHECK_PHASE_INSTANCES, CHECK_PHASE_LIMITS};
use busbar_contract::abi::mechanism::call::{Blob, Outcome, BLOB_JSON};
use busbar_contract::abi::mechanism::lifecycle::{slot, OpenIn, OpenOut};
use busbar_contract::abi::sdk::door::{blank_in, blank_out};
use busbar_plugin_loader::dispatch::kinds::export::Export;
use busbar_plugin_loader::dispatch::{
    in_head, load_dropped, load_linked, out_head, Bind, ConnTable, DispatchConfig, Dispatcher,
    Frame, LinkedRow, NoSink, NO_BLOB,
};

#[path = "support/cdylib.rs"]
mod artifact;
#[path = "support/inbound.rs"]
mod inbound;

#[test]
fn check_refuses_a_phase_it_does_not_know_through_each_real_door() {
    for dropped in [false, true] {
        let d = Arc::new(Dispatcher::new(DispatchConfig::default()));
        let row = LinkedRow::of(busbar_export_prometheus::door::door).expect("rendered door");
        let bind = Bind {
            instance: Arc::from("metrics"),
            max_inflight_cap: 64,
            sink: Arc::new(NoSink),
            dispatcher: d.adopter(),
            conns: ConnTable::Host(inbound::host(d.conn_waker(), None)),
        };
        let p = if dropped {
            load_dropped::<Export>(&artifact::cdylib(), &row.statement, bind)
        } else {
            load_linked::<Export>(&row, bind)
        }
        .expect("the real sink loads");
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
        let mut output: OpenOut = blank_out();
        output.head = out_head();
        assert_eq!(
            p.call(slot::OPEN, &mut Frame::new(input, output)).outcome,
            Outcome::Ready
        );
        for phase in [CHECK_PHASE_LIMITS, CHECK_PHASE_INSTANCES, 7] {
            let mut input: CheckIn = blank_in();
            input.head = in_head();
            input.phase = phase;
            let mut frame = Frame::new(
                input,
                CheckOut {
                    head: out_head(),
                    findings: NO_BLOB,
                },
            );
            let called = p.call(busbar_contract::abi::export::slot::CHECK, &mut frame);
            if phase == 7 {
                assert_eq!(called.outcome, Outcome::Refused);
                assert_eq!(
                    called.error.as_deref(),
                    Some(busbar_export_prometheus::door::UNKNOWN_PHASE.as_bytes())
                );
            } else {
                assert_eq!(called.outcome, Outcome::Ready);
                assert_eq!(frame.out.findings.len, 0);
            }
        }
        assert_eq!(
            p.call(slot::CLOSE, &mut Frame::new(in_head(), out_head()))
                .outcome,
            Outcome::Ready
        );
    }
}
