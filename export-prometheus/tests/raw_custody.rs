// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! Observe only initialized ranges lent by the real door, before their allocator retires them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::c_void;
use std::mem::size_of;

use busbar_contract::abi::export::{
    self, Ops, ScrapeFamily, ScrapeLabel, ScrapeSample, ServeIn, ServeOut,
};
use busbar_contract::abi::host::service::{HostSlots, ServiceOut, SnapshotReadIn, SERVICES};
use busbar_contract::abi::mechanism::call::{
    AbiStr, Blob, InHead, OutHead, Outcome, RawOutcome, BLOB_JSON,
};
use busbar_contract::abi::mechanism::door::Door;
use busbar_contract::abi::mechanism::lifecycle::{slot, OpenIn, OpenOut, ReleaseIn};
use busbar_contract::abi::mechanism::ticket::{HostCtx, HostTables};
use busbar_contract::abi::sdk::door::{blank_in, blank_out};

#[derive(Clone, Copy)]
struct Watched {
    at: usize,
    len: usize,
    retired: bool,
    wiped: bool,
    reallocated: bool,
}
const NONE: Watched = Watched {
    at: 0,
    len: 0,
    retired: false,
    wiped: false,
    reallocated: false,
};
thread_local! {
    static WATCH: Cell<[Watched; 4]> = const { Cell::new([NONE; 4]) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
    static MODE: Cell<u8> = const { Cell::new(0) };
    static READS: Cell<usize> = const { Cell::new(0) };
}
fn watch(ptr: *const u8, len: usize) {
    COUNT.with(|n| {
        let at = n.get();
        WATCH.with(|w| {
            let mut all = w.get();
            all[at] = Watched {
                at: ptr as usize,
                len,
                ..NONE
            };
            w.set(all);
        });
        n.set(at + 1);
    });
}
fn retiring(ptr: *mut u8, reallocated: bool) {
    let _ = WATCH.try_with(|w| {
        let mut all = w.get();
        for seen in &mut all {
            if !seen.retired && seen.at != 0 && seen.at == ptr as usize {
                // SAFETY: registration is exclusively a base initialized owned range lent by the
                // door; System still owns the allocation until this callback returns.
                seen.wiped = unsafe { std::slice::from_raw_parts(ptr, seen.len) }
                    .iter()
                    .all(|b| *b == 0);
                seen.retired = true;
                seen.reallocated = reallocated;
            }
        }
        w.set(all);
    });
}
struct Observing;
// SAFETY: every original layout and pointer is forwarded to System unchanged. Reads cover only
// registered initialized live ranges, immediately before deallocation/reallocation.
unsafe impl GlobalAlloc for Observing {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        retiring(p, false);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        retiring(p, true);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static ALLOCATOR: Observing = Observing;

extern "C" fn snapshot(_: HostCtx, raw: *const c_void, answer: *mut ServiceOut) -> RawOutcome {
    // SAFETY: the SDK lends the initialized scratch buffer and output record for this callback.
    let input = unsafe { &*raw.cast::<SnapshotReadIn>() };
    let out = unsafe { &mut *answer };
    let base = input.into.buf;
    watch(base, input.into.cap);
    let reads = READS.with(|n| {
        let next = n.get() + 1;
        n.set(next);
        next
    });
    let mode = MODE.with(Cell::get);
    let sample_at = size_of::<ScrapeFamily>();
    let label_at = sample_at + size_of::<ScrapeSample>();
    let mut at = label_at + size_of::<ScrapeLabel>();
    let mut text = |s: &str| {
        // SAFETY: fixed records and strings fit the fully initialized >=64KiB scratch buffer.
        let ptr = unsafe { base.add(at) };
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, s.len()) };
        at += s.len();
        AbiStr { ptr, len: s.len() }
    };
    let name = text("private_total");
    let key = text("owner");
    let value = text("private_metric_canary");
    let number = text("7");
    let sample = ScrapeSample {
        name,
        labels: unsafe { base.add(label_at).cast() },
        labels_len: 1,
        value: number,
    };
    let label = ScrapeLabel {
        key,
        value: if mode == 1 {
            AbiStr {
                ptr: std::ptr::null(),
                len: 1,
            }
        } else {
            value
        },
    };
    let family = ScrapeFamily {
        name,
        help: AbiStr::ZERO,
        unit: AbiStr::ZERO,
        kind: export::SCRAPE_KIND_COUNTER,
        _reserved: [0; 7],
        samples: unsafe { base.add(sample_at).cast() },
        samples_len: 1,
    };
    // SAFETY: record offsets are aligned and disjoint; all remain lent until callback return.
    unsafe {
        base.cast::<ScrapeFamily>().write(family);
        base.add(sample_at).cast::<ScrapeSample>().write(sample);
        base.add(label_at).cast::<ScrapeLabel>().write(label);
    }
    if mode == 2 || (mode == 4 && reads == 1) {
        out.needed_bytes = (input.into.cap * 2) as u64;
        out.outcome = RawOutcome::of(Outcome::Failed);
    } else if mode == 3 {
        out.outcome = RawOutcome::of(Outcome::Refused);
    } else {
        out.len = at as u64;
        out.value = 1;
        out.outcome = RawOutcome::of(Outcome::Ready);
    }
    out.outcome
}

fn ops() -> &'static Ops {
    // SAFETY: the linked door and export table are static program data.
    unsafe {
        &*Door::read_parts(busbar_export_prometheus::door::door())
            .ops
            .cast::<Ops>()
    }
}
fn head<T>(op: u32) -> InHead {
    let mut h: InHead = blank_in();
    h.size = size_of::<T>() as u32;
    h.op = op;
    h
}
fn abi(s: &str) -> AbiStr {
    AbiStr {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

#[test]
fn snapshot_and_successful_body_owners_wipe_at_release_close_and_error() {
    for mode in 0..=4 {
        for release in [true, false] {
            WATCH.with(|w| w.set([NONE; 4]));
            COUNT.with(|n| n.set(0));
            MODE.with(|n| n.set(mode));
            READS.with(|n| n.set(0));
            // SAFETY: HostSlots consists of scalar fields and nullable function pointers.
            let mut services: HostSlots = unsafe { std::mem::zeroed() };
            services.size = size_of::<HostSlots>() as u32;
            services.slots = SERVICES;
            services.snapshot_read = Some(snapshot);
            let host = HostTables {
                size: size_of::<HostTables>() as u32,
                _reserved: 0,
                ctx: HostCtx::NONE,
                wake: None,
                conns: std::ptr::null(),
                services: &services,
                io: std::ptr::null(),
            };
            let mut input: OpenIn = blank_in();
            input.head = head::<OpenIn>(slot::OPEN);
            input.host = &host;
            let settings = br#"{"buffer_seconds":60}"#;
            input.settings = Blob {
                ptr: settings.as_ptr(),
                len: settings.len(),
                fmt: BLOB_JSON,
                flags: 0,
            };
            input.generation = 1;
            let mut opened: OpenOut = blank_out();
            opened.head.size = size_of::<OpenOut>() as u32;
            let table = ops();
            assert_eq!(
                // SAFETY: this is the static export door's OPEN slot. The initialized OpenIn
                // and writable OpenOut have their exact sizes; settings, host tables and every
                // non-null host pointer remain borrowed until the instance closes below.
                unsafe {
                    table.head.open.unwrap()(
                        std::ptr::null_mut(),
                        (&input as *const OpenIn).cast(),
                        (&mut opened as *mut OpenOut).cast(),
                    )
                }
                .outcome(),
                Outcome::Ready
            );
            let mut input: ServeIn = blank_in();
            input.head = head::<ServeIn>(export::slot::SERVE);
            input.method = abi("GET");
            input.path = abi("/metrics");
            let mut output: ServeOut = blank_out();
            output.head.size = size_of::<ServeOut>() as u32;
            // SAFETY: OPEN returned this live instance. Both exact-sized records are initialized,
            // method/path are static borrows, and the output lease stays live until release/close.
            let outcome = unsafe {
                table.serve.unwrap()(
                    opened.instance,
                    (&input as *const ServeIn).cast(),
                    (&mut output as *mut ServeOut).cast(),
                )
            }
            .outcome();
            if mode == 0 || mode == 4 {
                assert_eq!(outcome, Outcome::Ready);
                let expected = b"# TYPE private_total counter\nprivate_total{owner=\"private_metric_canary\"} 7\n\n";
                // SAFETY: the successful body is leased until release/close below.
                assert_eq!(
                    unsafe { std::slice::from_raw_parts(output.body.ptr, output.body.len) },
                    expected
                );
                watch(output.body.ptr, output.body.len);
                if release {
                    let mut input: ReleaseIn = blank_in();
                    input.head = head::<ReleaseIn>(slot::RELEASE);
                    input.lease = output.head.lease;
                    let mut released: OutHead = blank_out();
                    released.size = size_of::<OutHead>() as u32;
                    assert_eq!(
                        // SAFETY: the instance and its returned lease are still live. The exact-
                        // sized release records are initialized; the body is not read afterward.
                        unsafe {
                            table.head.release.unwrap()(
                                opened.instance,
                                (&input as *const ReleaseIn).cast(),
                                (&mut released as *mut OutHead).cast(),
                            )
                        }
                        .outcome(),
                        Outcome::Ready
                    );
                }
            } else {
                assert_eq!(outcome, Outcome::Failed, "mode={mode}");
                // SAFETY: the failure text lives in the instance's reply storage until close below.
                let error = unsafe {
                    std::slice::from_raw_parts(output.head.error.ptr, output.head.error.len)
                };
                let cause: &[u8] = match mode {
                    1 => b"the host's snapshot broke the service's rules (scope WHOLE)",
                    2 => b"the snapshot outgrew the buffer on the one re-call (scope WHOLE)",
                    _ => b"the host refused the snapshot read (scope WHOLE)",
                };
                assert_eq!(
                    error,
                    cause,
                    "mode={mode}: {}",
                    String::from_utf8_lossy(error)
                );
            }
            let input = head::<InHead>(slot::CLOSE);
            let mut output: OutHead = blank_out();
            output.size = size_of::<OutHead>() as u32;
            // SAFETY: this closes the still-live instance exactly once. Exact-sized input/output
            // records and host tables remain live through the call; no instance/lease is used later.
            unsafe {
                table.head.close.unwrap()(
                    opened.instance,
                    (&input as *const InHead).cast(),
                    (&mut output as *mut OutHead).cast(),
                );
            }
            let count = COUNT.with(Cell::get);
            assert_eq!(
                count,
                if mode == 4 {
                    3
                } else if mode == 0 || mode == 2 {
                    2
                } else {
                    1
                }
            );
            for seen in WATCH.with(Cell::get).into_iter().take(count) {
                assert!(seen.retired && seen.wiped && !seen.reallocated, "mode={mode}, release={release}: initialized owner retired unwiped or was reallocated");
            }
        }
    }
}
