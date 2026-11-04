// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! The door's size bounds and read hints, driven through the real export door: `scrape` refuses a
//! rendering past the export answer limit and a snapshot missing a required text with words;
//! `serve` keeps one read hint per snapshot scope and never allocates for a short answer no
//! renderable snapshot could need.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::size_of;

use busbar_contract::abi::export::{
    self, Ops, ScrapeFamily, ScrapeIn, ScrapeLabel, ScrapeOut, ScrapeSample, ServeIn, ServeOut,
};
use busbar_contract::abi::host::service::{
    HostSlots, ServiceOut, SnapshotReadIn, SERVICES, SNAPSHOT_SCOPE_HOOKS, SNAPSHOT_SCOPE_WHOLE,
};
use busbar_contract::abi::mechanism::call::{
    AbiStr, Blob, InHead, OutHead, Outcome, RawOutcome, BLOB_JSON,
};
use busbar_contract::abi::mechanism::door::Door;
use busbar_contract::abi::mechanism::lifecycle::{slot, OpenIn, OpenOut};
use busbar_contract::abi::mechanism::ticket::{HostCtx, HostTables};
use busbar_contract::abi::sdk::door::{blank_in, blank_out};

/// The first read's buffer, as the door sizes it.
const FIRST_READ: usize = 64 * 1024;
/// The WHOLE snapshot's layout size in [`scripted`]: past the first read's buffer.
const WHOLE_BYTES: usize = 256 * 1024;
/// The HOOKS snapshot's layout size in [`scripted`]: far below the first read's buffer.
const HOOKS_BYTES: usize = 1024;
/// A short answer no snapshot that renders within the 16 MiB answer limit could need.
const HUGE_BYTES: u64 = 1 << 30;

thread_local! {
    /// Answer every read short of [`HUGE_BYTES`].
    static HUGE: Cell<bool> = const { Cell::new(false) };
    /// Every read: its scope and the capacity it was lent.
    static READS: RefCell<Vec<(u32, usize)>> = const { RefCell::new(Vec::new()) };
}

/// `snapshot.read`: one counter family `m` laid out at the head of the buffer, reported as
/// [`WHOLE_BYTES`] / [`HOOKS_BYTES`] used, short when the buffer is smaller.
extern "C" fn scripted(_: HostCtx, raw: *const c_void, answer: *mut ServiceOut) -> RawOutcome {
    // SAFETY: the SDK lends the initialized input and output records for this callback.
    let input = unsafe { &*raw.cast::<SnapshotReadIn>() };
    let out = unsafe { &mut *answer };
    let cap = input.into.cap;
    READS.with(|r| r.borrow_mut().push((input.scope, cap)));
    let size = if input.scope == SNAPSHOT_SCOPE_HOOKS {
        HOOKS_BYTES
    } else {
        WHOLE_BYTES
    };
    if HUGE.with(Cell::get) {
        out.needed_bytes = HUGE_BYTES;
        out.outcome = RawOutcome::of(Outcome::Failed);
        return out.outcome;
    }
    if cap < size {
        out.needed_bytes = size as u64;
        out.outcome = RawOutcome::of(Outcome::Failed);
        return out.outcome;
    }
    let base = input.into.buf;
    let sample_at = size_of::<ScrapeFamily>();
    let mut at = sample_at + size_of::<ScrapeSample>();
    let mut text = |s: &str| {
        // SAFETY: the records and strings fit the lent buffer of at least `HOOKS_BYTES`.
        let ptr = unsafe { base.add(at) };
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), ptr, s.len()) };
        at += s.len();
        AbiStr { ptr, len: s.len() }
    };
    let name = text("m");
    let value = text("1");
    // SAFETY: both records are aligned, disjoint and inside the lent buffer.
    unsafe {
        base.cast::<ScrapeFamily>().write(ScrapeFamily {
            name,
            help: AbiStr::ZERO,
            unit: AbiStr::ZERO,
            kind: export::SCRAPE_KIND_COUNTER,
            _reserved: [0; 7],
            samples: base.add(sample_at).cast(),
            samples_len: 1,
        });
        base.add(sample_at)
            .cast::<ScrapeSample>()
            .write(ScrapeSample {
                name,
                labels: std::ptr::null(),
                labels_len: 0,
                value,
            });
    }
    out.len = size as u64;
    out.value = 1;
    out.outcome = RawOutcome::of(Outcome::Ready);
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

fn abi(s: &[u8]) -> AbiStr {
    AbiStr {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

/// The answer's error text; empty when it names none.
fn error_of(error: AbiStr) -> String {
    if error.ptr.is_null() {
        return String::new();
    }
    // SAFETY: the door's error text lives until its instance closes, after this read.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(error.ptr, error.len) })
        .into_owned()
}

/// Open an instance over a host whose `snapshot.read` is [`scripted`], run `body` on it, close it.
fn with_instance<R>(body: impl FnOnce(&Ops, *mut c_void) -> R) -> R {
    // SAFETY: HostSlots consists of scalar fields and nullable function pointers.
    let mut services: HostSlots = unsafe { std::mem::zeroed() };
    services.size = size_of::<HostSlots>() as u32;
    services.slots = SERVICES;
    services.snapshot_read = Some(scripted);
    let host = HostTables {
        size: size_of::<HostTables>() as u32,
        _reserved: 0,
        ctx: HostCtx::NONE,
        wake: None,
        conns: std::ptr::null(),
        services: &services,
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
    // SAFETY: the static door's OPEN slot over initialized exact-sized records; settings and host
    // tables stay borrowed until the instance closes below.
    let outcome = unsafe {
        table.head.open.unwrap()(
            std::ptr::null_mut(),
            (&input as *const OpenIn).cast(),
            (&mut opened as *mut OpenOut).cast(),
        )
    }
    .outcome();
    assert_eq!(outcome, Outcome::Ready, "the instance opens");
    let answer = body(table, opened.instance);
    let input = head::<InHead>(slot::CLOSE);
    let mut output: OutHead = blank_out();
    output.size = size_of::<OutHead>() as u32;
    // SAFETY: close the still-live instance exactly once; nothing it lent is read afterwards.
    unsafe {
        table.head.close.unwrap()(
            opened.instance,
            (&input as *const InHead).cast(),
            (&mut output as *mut OutHead).cast(),
        );
    }
    answer
}

/// `GET path` on `instance`: the outcome, the status and the error text.
fn get(table: &Ops, instance: *mut c_void, path: &'static str) -> (Outcome, u16, String) {
    let mut input: ServeIn = blank_in();
    input.head = head::<ServeIn>(export::slot::SERVE);
    input.method = abi(b"GET");
    input.path = abi(path.as_bytes());
    let mut output: ServeOut = blank_out();
    output.head.size = size_of::<ServeOut>() as u32;
    // SAFETY: the live instance; exact-sized initialized records; method/path are static.
    let outcome = unsafe {
        table.serve.unwrap()(
            instance,
            (&input as *const ServeIn).cast(),
            (&mut output as *mut ServeOut).cast(),
        )
    }
    .outcome();
    (outcome, output.status_code, error_of(output.head.error))
}

/// `scrape` of `families` into a `cap`-byte buffer: the outcome, `written`, `needed` and the
/// error text.
fn scrape(
    table: &Ops,
    instance: *mut c_void,
    families: &[ScrapeFamily],
    cap: usize,
) -> (Outcome, usize, usize, String) {
    let mut buf = vec![0u8; cap];
    let mut input: ScrapeIn = blank_in();
    input.head = head::<ScrapeIn>(export::slot::SCRAPE);
    input.families = families.as_ptr();
    input.families_len = families.len();
    input.buf = buf.as_mut_ptr();
    input.cap = buf.len();
    let mut output: ScrapeOut = blank_out();
    output.head.size = size_of::<ScrapeOut>() as u32;
    // SAFETY: the live instance; exact-sized initialized records; the families, their samples,
    // labels and strings and the buffer all outlive the call.
    let outcome = unsafe {
        table.scrape.unwrap()(
            instance,
            (&input as *const ScrapeIn).cast(),
            (&mut output as *mut ScrapeOut).cast(),
        )
    }
    .outcome();
    (
        outcome,
        output.written,
        output.needed,
        error_of(output.head.error),
    )
}

fn family(name: AbiStr, samples: &[ScrapeSample]) -> ScrapeFamily {
    ScrapeFamily {
        name,
        help: AbiStr::ZERO,
        unit: AbiStr::ZERO,
        kind: export::SCRAPE_KIND_COUNTER,
        _reserved: [0; 7],
        samples: samples.as_ptr(),
        samples_len: samples.len(),
    }
}

#[test]
fn each_snapshot_scope_keeps_its_own_read_hint() {
    HUGE.with(|h| h.set(false));
    READS.with(|r| r.borrow_mut().clear());
    let answers = with_instance(|table, instance| {
        ["/metrics", "/metrics/hooks", "/metrics"].map(|path| get(table, instance, path))
    });
    for (outcome, status, error) in &answers {
        assert_eq!((*outcome, *status), (Outcome::Ready, 200), "{error}");
    }
    let reads = READS.with(|r| r.borrow().clone());
    assert_eq!(
        reads.len(),
        4,
        "one short re-call on the first WHOLE read, then one read each: {reads:?}"
    );
    assert_eq!(reads[0], (SNAPSHOT_SCOPE_WHOLE, FIRST_READ));
    assert_eq!(
        reads[2],
        (SNAPSHOT_SCOPE_HOOKS, FIRST_READ),
        "the HOOKS read is not sized by the WHOLE snapshot: {reads:?}"
    );
    assert_eq!(reads[3].0, SNAPSHOT_SCOPE_WHOLE);
    assert!(
        reads[3].1 >= WHOLE_BYTES,
        "the second WHOLE read is sized by the first, not by HOOKS: {reads:?}"
    );
}

#[test]
fn a_short_answer_past_any_renderable_snapshot_is_refused_before_allocating() {
    HUGE.with(|h| h.set(true));
    READS.with(|r| r.borrow_mut().clear());
    let answer = with_instance(|table, instance| get(table, instance, "/metrics"));
    HUGE.with(|h| h.set(false));
    assert_eq!(answer.0, Outcome::Failed);
    assert_eq!(
        answer.2,
        "the rendered snapshot exceeds the export answer limit (scope WHOLE)"
    );
    assert_eq!(
        READS.with(|r| r.borrow().len()),
        1,
        "no re-call buffer is allocated for the short answer"
    );
}

#[test]
fn a_scrape_past_the_export_answer_limit_is_failed_with_words_not_answered_short() {
    let value = vec![b'1'; 17 * 1024 * 1024];
    let samples = [ScrapeSample {
        name: abi(b"big_total"),
        labels: std::ptr::null(),
        labels_len: 0,
        value: abi(&value),
    }];
    let families = [family(abi(b"big_total"), &samples)];
    let (outcome, written, needed, error) =
        with_instance(|table, instance| scrape(table, instance, &families, 64 * 1024));
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(
        (written, needed),
        (0, 0),
        "never a short answer past the limit"
    );
    assert_eq!(
        error,
        "the rendered snapshot exceeds the export answer limit"
    );
}

#[test]
fn a_scrape_snapshot_missing_a_required_text_is_failed_with_words() {
    let unnamed =
        "the host's scrape snapshot lacks a family name, sample name, label or sample value";
    let label = [ScrapeLabel {
        key: abi(b"pool"),
        value: AbiStr::ZERO,
    }];
    let keyless = [ScrapeLabel {
        key: AbiStr::ZERO,
        value: abi(b"a"),
    }];
    let named = [ScrapeSample {
        name: abi(b"m"),
        labels: std::ptr::null(),
        labels_len: 0,
        value: abi(b"1"),
    }];
    let no_name = [ScrapeSample {
        name: AbiStr::ZERO,
        ..named[0]
    }];
    let no_value = [ScrapeSample {
        value: AbiStr::ZERO,
        ..named[0]
    }];
    let no_label_value = [ScrapeSample {
        labels: label.as_ptr(),
        labels_len: 1,
        ..named[0]
    }];
    let no_label_key = [ScrapeSample {
        labels: keyless.as_ptr(),
        labels_len: 1,
        ..named[0]
    }];
    let cases = [
        family(AbiStr::ZERO, &named),
        family(abi(b"m"), &no_name),
        family(abi(b"m"), &no_value),
        family(abi(b"m"), &no_label_value),
        family(abi(b"m"), &no_label_key),
    ];
    for (index, case) in cases.iter().enumerate() {
        let (outcome, written, needed, error) = with_instance(|table, instance| {
            scrape(table, instance, std::slice::from_ref(case), 64 * 1024)
        });
        assert_eq!(outcome, Outcome::Failed, "case {index}");
        assert_eq!((written, needed), (0, 0), "case {index}");
        assert_eq!(error, unnamed, "case {index}");
    }
    let (outcome, written, _, error) =
        with_instance(|table, instance| scrape(table, instance, &[family(abi(b"m"), &named)], 64));
    assert_eq!(outcome, Outcome::Ready, "{error}");
    assert_eq!(written, "# TYPE m counter\nm 1\n\n".len());
}
