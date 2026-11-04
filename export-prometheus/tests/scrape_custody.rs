// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! `scrape`'s private copies of the lent snapshot never reach the allocator unwiped: every block
//! freed or reallocated while the door runs is searched for a private label canary.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::mem::size_of;

use busbar_contract::abi::export::{
    self, Ops, ScrapeFamily, ScrapeIn, ScrapeLabel, ScrapeOut, ScrapeSample,
    SCRAPE_FLAG_HOOK_FAMILIES,
};
use busbar_contract::abi::mechanism::call::{AbiStr, Blob, InHead, OutHead, Outcome, BLOB_JSON};
use busbar_contract::abi::mechanism::door::Door;
use busbar_contract::abi::mechanism::lifecycle::{slot, OpenIn, OpenOut};
use busbar_contract::abi::mechanism::ticket::{HostCtx, HostTables};
use busbar_contract::abi::sdk::door::{blank_in, blank_out};

const CANARY: &[u8] = b"scrape_custody_private_canary";

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static LEAKS: Cell<usize> = const { Cell::new(0) };
}

fn armed() -> bool {
    ARMED.try_with(Cell::get).unwrap_or(false)
}

/// Count `p`'s `len` bytes as a leak when they still hold [`CANARY`].
fn retiring(p: *const u8, len: usize) {
    if !armed() {
        return;
    }
    // SAFETY: the allocator still owns this live block of `len` bytes until the caller forwards
    // it. Blocks born while armed are zero-filled whole (below); the rest were the setup's own,
    // fully written allocations.
    let block = unsafe { std::slice::from_raw_parts(p, len) };
    if block.windows(CANARY.len()).any(|w| w == CANARY) {
        let _ = LEAKS.try_with(|n| n.set(n.get() + 1));
    }
}

struct Searching;

// SAFETY: every layout and pointer is forwarded to System unchanged; while armed, new memory is
// zero-filled so the search only ever reads initialized bytes, immediately before a block is freed
// or moved.
unsafe impl GlobalAlloc for Searching {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if armed() {
            unsafe { System.alloc_zeroed(l) }
        } else {
            unsafe { System.alloc(l) }
        }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        retiring(p, l.size());
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        retiring(p, l.size());
        let moved = unsafe { System.realloc(p, l, n) };
        if armed() && !moved.is_null() && n > l.size() {
            // SAFETY: the grown block is `n` bytes; its tail past the old size is ours to write.
            unsafe { moved.add(l.size()).write_bytes(0, n - l.size()) };
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Searching = Searching;

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

fn abi(s: &[u8]) -> AbiStr {
    AbiStr {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

/// Scrape one counter whose label value is `value`, the hook layout when `hooks`, with only the
/// door's own work armed: the outcome and the blocks retired still holding the canary.
fn scrape(value: &[u8], hooks: bool) -> (Outcome, usize) {
    let host = HostTables {
        size: size_of::<HostTables>() as u32,
        _reserved: 0,
        ctx: HostCtx::NONE,
        wake: None,
        conns: std::ptr::null(),
        services: std::ptr::null(),
        io: std::ptr::null(),
    };
    let settings = br#"{"buffer_seconds":60}"#;
    let mut input: OpenIn = blank_in();
    input.head = head::<OpenIn>(slot::OPEN);
    input.host = &host;
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
    // SAFETY: the static door's OPEN slot over initialized exact-sized records; the settings and
    // host tables stay borrowed until the instance closes below.
    let outcome = unsafe {
        table.head.open.unwrap()(
            std::ptr::null_mut(),
            (&input as *const OpenIn).cast(),
            (&mut opened as *mut OpenOut).cast(),
        )
    }
    .outcome();
    assert_eq!(outcome, Outcome::Ready, "the instance opens");

    let labels = [ScrapeLabel {
        key: abi(b"owner"),
        value: abi(value),
    }];
    let samples = [ScrapeSample {
        name: abi(b"private_total"),
        labels: labels.as_ptr(),
        labels_len: labels.len(),
        value: abi(b"7"),
    }];
    let families = [ScrapeFamily {
        name: abi(b"private_total"),
        help: AbiStr::ZERO,
        unit: AbiStr::ZERO,
        kind: export::SCRAPE_KIND_COUNTER,
        _reserved: [0; 7],
        samples: samples.as_ptr(),
        samples_len: samples.len(),
    }];
    let mut buf = vec![0u8; 64 * 1024];
    let mut input: ScrapeIn = blank_in();
    input.head = head::<ScrapeIn>(export::slot::SCRAPE);
    if hooks {
        input.head.flags |= SCRAPE_FLAG_HOOK_FAMILIES;
    }
    input.families = families.as_ptr();
    input.families_len = families.len();
    input.buf = buf.as_mut_ptr();
    input.cap = buf.len();
    let mut output: ScrapeOut = blank_out();
    output.head.size = size_of::<ScrapeOut>() as u32;
    LEAKS.with(|n| n.set(0));
    ARMED.with(|a| a.set(true));
    // SAFETY: the live instance; exact-sized initialized records; the families, their samples,
    // labels and strings and the buffer all outlive the call.
    let outcome = unsafe {
        table.scrape.unwrap()(
            opened.instance,
            (&input as *const ScrapeIn).cast(),
            (&mut output as *mut ScrapeOut).cast(),
        )
    }
    .outcome();
    ARMED.with(|a| a.set(false));
    let leaks = LEAKS.with(Cell::get);

    let input = head::<InHead>(slot::CLOSE);
    let mut closed: OutHead = blank_out();
    closed.size = size_of::<OutHead>() as u32;
    // SAFETY: close the still-live instance exactly once; nothing it lent is read afterwards.
    unsafe {
        table.head.close.unwrap()(
            opened.instance,
            (&input as *const InHead).cast(),
            (&mut closed as *mut OutHead).cast(),
        );
    }
    (outcome, leaks)
}

#[test]
fn scrape_copies_of_private_labels_are_wiped_before_the_allocator_sees_them() {
    let valid = CANARY.to_vec();
    let mut invalid = CANARY.to_vec();
    invalid.extend_from_slice(&[0xff; 64]);
    for hooks in [false, true] {
        for (what, value) in [("UTF-8", &valid), ("not UTF-8", &invalid)] {
            let (outcome, leaks) = scrape(value, hooks);
            assert_eq!(outcome, Outcome::Ready, "{what}, hooks={hooks}");
            assert_eq!(
                leaks, 0,
                "{what}, hooks={hooks}: a block holding the private label was freed or moved unwiped"
            );
        }
    }
}
