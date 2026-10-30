// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! `serve`'s answers to the two host-service failures the real dispatcher never produces, driven
//! through the door's own op table over a hand-laid host table (the dispatcher serves every
//! `snapshot.read` slot and lays out every answer well): a host that serves no `snapshot.read` is
//! FAILED naming `READ_UNSERVED`, and a host whose answer breaks the service's rules (a length
//! beyond the buffer lent) is FAILED naming `READ_BROKEN`. The declined and short arms are
//! `serve.rs`'s.

#![allow(unsafe_code)] // the hand-laid host table and the door's raw ops

use std::ffi::c_void;
use std::mem::size_of;

use busbar_contract::abi::export::{self, Ops, ServeIn, ServeOut};
use busbar_contract::abi::host::service::{HostSlots, ServiceOut, SERVICES};
use busbar_contract::abi::mechanism::call::{AbiStr, Blob, Op, Outcome, RawOutcome, BLOB_JSON};
use busbar_contract::abi::mechanism::door::Door;
use busbar_contract::abi::mechanism::lifecycle::{slot as lc, OpenIn, OpenOut};
use busbar_contract::abi::mechanism::ticket::{HostCtx, HostTables};
use busbar_contract::abi::sdk::door::{blank_in, blank_out};
use busbar_plugin_loader::dispatch::{in_head, out_head};

/// A `snapshot.read` that answers READY claiming more bytes than the buffer lent holds.
extern "C" fn overlong(_: HostCtx, _: *const c_void, out: *mut ServiceOut) -> RawOutcome {
    // SAFETY: the SDK's own `ServiceOut`, writable for the call.
    let out = unsafe { &mut *out };
    out.outcome = RawOutcome::of(Outcome::Ready);
    out.len = 1 << 40;
    RawOutcome::of(Outcome::Ready)
}

/// The table: every slot absent but `snapshot_read`.
fn slots(snapshot_read: Option<busbar_contract::abi::host::service::ServiceFn>) -> HostSlots {
    // SAFETY: `HostSlots` is integers and nullable function pointers: all-zero is every slot NULL.
    let mut t: HostSlots = unsafe { std::mem::zeroed() };
    t.size = size_of::<HostSlots>() as u32;
    t.slots = SERVICES;
    t.snapshot_read = snapshot_read;
    t
}

/// The door's `open`, `serve` and `close`, read off its op table.
fn ops() -> (Op, Op, Op) {
    let door: &'static Door = busbar_export_prometheus::door::door();
    // SAFETY: the linked door is `'static` plugin data; its ops table is the export kind's `Ops`.
    let table = unsafe {
        let parts = Door::read_parts(std::ptr::from_ref(door));
        &*parts.ops.cast::<Ops>()
    };
    (
        table.head.open.expect("open"),
        table.serve.expect("serve"),
        table.head.close.expect("close"),
    )
}

/// `GET /metrics` on an instance opened over a host whose services are `slots`: the outcome and
/// the error text it named.
fn get_metrics(slots: HostSlots) -> (Outcome, String) {
    let (open, serve, close) = ops();
    let tables = HostTables {
        size: size_of::<HostTables>() as u32,
        _reserved: 0,
        ctx: HostCtx::NONE,
        wake: None,
        conns: std::ptr::null(),
        services: std::ptr::from_ref(&slots),
        io: std::ptr::null(),
    };
    let settings = br#"{"buffer_seconds":60}"#;
    let mut oin: OpenIn = blank_in();
    oin.head = in_head();
    oin.head.size = size_of::<OpenIn>() as u32;
    oin.head.op = lc::OPEN;
    oin.host = std::ptr::from_ref(&tables);
    oin.settings = Blob {
        ptr: settings.as_ptr(),
        len: settings.len(),
        fmt: BLOB_JSON,
        flags: 0,
    };
    oin.generation = 1;
    let mut oout: OpenOut = blank_out();
    oout.head = out_head();
    oout.head.size = size_of::<OpenOut>() as u32;
    let opened = open(
        std::ptr::null_mut(),
        std::ptr::from_ref(&oin).cast(),
        std::ptr::from_mut(&mut oout).cast(),
    );
    assert_eq!(opened.outcome(), Outcome::Ready, "the instance opens");

    let path = "/metrics";
    let mut sin: ServeIn = blank_in();
    sin.head = in_head();
    sin.head.size = size_of::<ServeIn>() as u32;
    sin.head.op = export::slot::SERVE;
    sin.method = AbiStr {
        ptr: b"GET".as_ptr(),
        len: 3,
    };
    sin.path = AbiStr {
        ptr: path.as_ptr(),
        len: path.len(),
    };
    let mut sout: ServeOut = blank_out();
    sout.head = out_head();
    sout.head.size = size_of::<ServeOut>() as u32;
    let served = serve(
        oout.instance,
        std::ptr::from_ref(&sin).cast(),
        std::ptr::from_mut(&mut sout).cast(),
    );
    let error = if sout.head.error.ptr.is_null() {
        String::new()
    } else {
        // SAFETY: the plugin's own error text, static for the process.
        String::from_utf8_lossy(unsafe {
            std::slice::from_raw_parts(sout.head.error.ptr, sout.head.error.len)
        })
        .into_owned()
    };
    let outcome = served.outcome();

    let mut cin = in_head();
    cin.op = lc::CLOSE;
    let mut cout = out_head();
    close(
        oout.instance,
        std::ptr::from_ref(&cin).cast(),
        std::ptr::from_mut(&mut cout).cast(),
    );
    (outcome, error)
}

#[test]
fn a_host_that_serves_no_snapshot_read_is_failed_naming_the_cause() {
    let (outcome, error) = get_metrics(slots(None));
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(
        error,
        "the host serves no snapshot.read: the exposition cannot read the snapshot"
    );
}

#[test]
fn a_host_answer_that_breaks_the_service_rules_is_failed_naming_the_cause() {
    let (outcome, error) = get_metrics(slots(Some(overlong)));
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(error, "the host's snapshot broke the service's rules");
}
