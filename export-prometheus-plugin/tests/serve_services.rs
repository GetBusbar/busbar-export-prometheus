// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! `serve`'s answers to the two host-service failures the real dispatcher never produces, driven
//! through the door's own op table over a hand-laid host table (the dispatcher serves every
//! `snapshot.read` slot and lays out every answer well): a host that serves no `snapshot.read` is
//! FAILED naming `READ_UNSERVED`, and a host whose answer breaks the service's rules (a length
//! beyond the buffer lent) is FAILED naming `READ_BROKEN`. The declined and short arms are
//! `serve.rs`'s.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
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

thread_local! {
    static FAIL_NEXT_ALLOCATION: Cell<bool> = const { Cell::new(false) };
    static REJECTED_ALLOCATION: Cell<usize> = const { Cell::new(0) };
}

struct AllocationFailure;

fn reject_allocation(layout: Layout) -> bool {
    if FAIL_NEXT_ALLOCATION
        .try_with(|armed| armed.replace(false))
        .unwrap_or(false)
    {
        let _ = REJECTED_ALLOCATION.try_with(|rejected| rejected.set(layout.size()));
        true
    } else {
        false
    }
}

// SAFETY: all successful allocations and deallocations use System unchanged. The one armed
// thread-local allocation returns null, as GlobalAlloc permits, without touching any allocation.
unsafe impl GlobalAlloc for AllocationFailure {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if reject_allocation(layout) {
            std::ptr::null_mut()
        } else {
            // SAFETY: forward the allocator's original valid layout.
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if reject_allocation(layout) {
            std::ptr::null_mut()
        } else {
            // SAFETY: forward the allocator's original valid layout.
            unsafe { System.alloc_zeroed(layout) }
        }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: every non-null allocation above came from System with this layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: AllocationFailure = AllocationFailure;

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
    get_metrics_with_services(Some(slots))
}

fn get_metrics_with_services(slots: Option<HostSlots>) -> (Outcome, String) {
    get_metrics_sequence(slots, 1).pop().unwrap()
}

fn get_metrics_sequence(slots: Option<HostSlots>, count: usize) -> Vec<(Outcome, String)> {
    let (open, serve, close) = ops();
    let tables = HostTables {
        size: size_of::<HostTables>() as u32,
        _reserved: 0,
        ctx: HostCtx::NONE,
        wake: None,
        conns: std::ptr::null(),
        services: slots.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
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
    // SAFETY: this static door's OPEN slot receives initialized exact-sized records. Settings and
    // all non-null host-table pointers remain live until CLOSE; the output is exclusively writable.
    let opened = unsafe {
        open(
            std::ptr::null_mut(),
            std::ptr::from_ref(&oin).cast(),
            std::ptr::from_mut(&mut oout).cast(),
        )
    };
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
    let answers = (0..count)
        .map(|_| {
            let mut sout: ServeOut = blank_out();
            sout.head = out_head();
            sout.head.size = size_of::<ServeOut>() as u32;
            // SAFETY: OPEN returned this live instance; method/path and host tables still live.
            // Exact-sized input/output records are initialized, and answer leases stay live below.
            let served = unsafe {
                serve(
                    oout.instance,
                    std::ptr::from_ref(&sin).cast(),
                    std::ptr::from_mut(&mut sout).cast(),
                )
            };
            let error = if sout.head.error.ptr.is_null() {
                String::new()
            } else {
                // SAFETY: the plugin's error lease is alive until this instance closes.
                String::from_utf8_lossy(unsafe {
                    std::slice::from_raw_parts(sout.head.error.ptr, sout.head.error.len)
                })
                .into_owned()
            };
            (served.outcome(), error)
        })
        .collect();

    let mut cin = in_head();
    cin.op = lc::CLOSE;
    let mut cout = out_head();
    // SAFETY: close the live instance exactly once while the initialized records and host-table
    // borrows remain valid. No instance or leased output is accessed after this call.
    unsafe {
        close(
            oout.instance,
            std::ptr::from_ref(&cin).cast(),
            std::ptr::from_mut(&mut cout).cast(),
        );
    }
    answers
}

#[test]
fn a_host_that_serves_no_snapshot_read_is_failed_naming_the_cause() {
    let (outcome, error) = get_metrics(slots(None));
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(
        error,
        "the host serves no snapshot.read: the exposition cannot read the snapshot (scope WHOLE)"
    );
}

#[test]
fn a_host_answer_that_breaks_the_service_rules_is_failed_naming_the_cause() {
    let (outcome, error) = get_metrics(slots(Some(overlong)));
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(
        error,
        "the host's snapshot broke the service's rules (scope WHOLE)"
    );
}

extern "C" fn impossible_size(_: HostCtx, _: *const c_void, out: *mut ServiceOut) -> RawOutcome {
    // SAFETY: the service caller lends a writable ServiceOut for this crossing.
    let out = unsafe { &mut *out };
    out.outcome = RawOutcome::of(Outcome::Failed);
    out.needed_bytes = u64::MAX;
    RawOutcome::of(Outcome::Failed)
}

#[test]
fn an_unrepresentable_snapshot_size_is_a_named_failure() {
    let (outcome, error) = get_metrics(slots(Some(impossible_size)));
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(
        error,
        "the host's snapshot broke the service's rules (scope WHOLE)"
    );
}

static SNAPSHOT_CAPACITIES: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

extern "C" fn spike_then_empty(
    _: HostCtx,
    input: *const c_void,
    out: *mut ServiceOut,
) -> RawOutcome {
    // SAFETY: snapshot.read lends these valid input and output records for this call.
    let input = unsafe { &*input.cast::<busbar_contract::abi::host::service::SnapshotReadIn>() };
    let out = unsafe { &mut *out };
    let mut capacities = SNAPSHOT_CAPACITIES.lock().unwrap();
    let call = capacities.len();
    capacities.push(input.into.cap);
    if call == 0 {
        out.needed_bytes = 128 * 1024;
        out.outcome = RawOutcome::of(Outcome::Failed);
    } else {
        out.len = if call == 1 { 128 * 1024 } else { 0 };
        out.value = 0;
        out.outcome = RawOutcome::of(Outcome::Ready);
    }
    out.outcome
}

#[test]
fn a_snapshot_spike_does_not_keep_the_peak_allocation_after_a_small_read() {
    SNAPSHOT_CAPACITIES.lock().unwrap().clear();
    let answers = get_metrics_sequence(Some(slots(Some(spike_then_empty))), 3);
    assert!(answers
        .iter()
        .all(|(outcome, why)| *outcome == Outcome::Ready && why.is_empty()));
    let capacities = SNAPSHOT_CAPACITIES.lock().unwrap();
    assert_eq!(
        capacities.len(),
        4,
        "one short recall, then two successful reads"
    );
    assert!(
        capacities[1] >= 128 * 1024,
        "the successful recall fits the spike"
    );
    assert!(
        capacities[2] > capacities[0],
        "the next read can use the successful large hint"
    );
    assert!(
        capacities[3] <= capacities[0],
        "after an empty successful read, a later scrape must stop allocating the historical peak: {capacities:?}"
    );
}

extern "C" fn short_with_an_unallocatable_retry(
    _: HostCtx,
    _: *const c_void,
    out: *mut ServiceOut,
) -> RawOutcome {
    // SAFETY: the SDK lends the reply for this synchronous service call.
    let out = unsafe { &mut *out };
    if REJECTED_ALLOCATION.with(Cell::get) == 0 {
        out.needed_bytes = 128 * 1024;
        out.outcome = RawOutcome::of(Outcome::Failed);
        FAIL_NEXT_ALLOCATION.with(|armed| armed.set(true));
    } else {
        out.value = 0;
        out.len = 0;
        out.outcome = RawOutcome::of(Outcome::Ready);
    }
    out.outcome
}

#[test]
fn an_unallocatable_retry_fails_with_words_and_does_not_poison_the_next_read() {
    REJECTED_ALLOCATION.with(|rejected| rejected.set(0));
    let answers = get_metrics_sequence(Some(slots(Some(short_with_an_unallocatable_retry))), 2);

    FAIL_NEXT_ALLOCATION.with(|armed| armed.set(false));
    let rejected = REJECTED_ALLOCATION.with(|rejected| rejected.replace(0));
    assert!(
        rejected >= 128 * 1024,
        "the actual retry buffer allocation must be refused"
    );
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0].0, Outcome::Failed);
    assert_eq!(
        answers[0].1, "the sink could not allocate the snapshot read buffer (scope WHOLE)",
        "the sink's own allocation failure is not blamed on the host"
    );
    assert_eq!(
        answers[1],
        (Outcome::Ready, String::new()),
        "the next read can recover"
    );
}

#[test]
fn a_host_that_offers_no_services_table_is_failed_naming_the_cause() {
    let (outcome, error) = get_metrics_with_services(None);
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(
        error,
        "the host offers this sink no services: the exposition cannot read the snapshot"
    );
}
