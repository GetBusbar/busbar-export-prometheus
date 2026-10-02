// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE DOOR: the prometheus sink on the export kind's table (`busbar_contract::abi::export`), its
//! lifecycle the SDK's generic one over [`Prometheus`] (`abi::sdk::life`), every kind op a
//! [`SafeSlot`].
//!
//! * `validate` — [`crate::validate`]: the configuration's words, the zero-retention refusal included.
//! * `open` / `refresh` — nothing to hold: every scrape carries the whole snapshot it renders.
//! * `scrape` — the snapshot rendered ([`crate::render`]) into the host's buffer; a buffer too
//!   small is FAILED with the bytes `needed`, nothing written, and the host calls again once.
//! * `deliver` — READY, nothing kept: the host pulls `metrics`, it never pushes them.
//! * `status` and `check` — READY with nothing to report. `serve` — REFUSED: no route of its own
//!   (the host serves `/metrics`).
//!
//! The one `unsafe` here is [`lent`]: the export kind's `ScrapeIn` lends its families and its
//! buffer as raw pointers, and the SDK states no safe accessor for them.

use busbar_contract::abi::export::{
    cancel, CheckIn, CheckOut, DeliverIn, ExportStream, ScrapeIn, ScrapeOut, ServeIn, ServeOut,
    StatusOut, Tail,
};
use busbar_contract::abi::mechanism::call::{InHead, OutHead, Outcome};
use busbar_contract::abi::mechanism::door::{KindTailHead, Statement};
use busbar_contract::abi::sdk::door::statement;
use busbar_contract::abi::sdk::life::{Held, Life, Refreshed, Refusal};
use busbar_contract::abi::sdk::{Instance, Lent, Out, Safe, SafeSlot};

use crate::NAME;

/// The one stream the sink carries.
const STREAMS: &[u8] = &[ExportStream::Metrics as u8];

const TAIL: Tail = Tail {
    head: KindTailHead {
        size: std::mem::size_of::<Tail>() as u32,
        _reserved: 0,
    },
    streams: STREAMS.as_ptr(),
    streams_len: STREAMS.len(),
    routes: std::ptr::null(),
    routes_len: 0,
};

/// This plugin's Statement: its name and version and the `metrics` stream.
pub const STATEMENT: Statement = Statement {
    kind_tail: (&TAIL as *const Tail).cast::<KindTailHead>(),
    ..statement(NAME, env!("CARGO_PKG_VERSION"), 64)
};

/// One opened instance. It holds nothing.
#[derive(Debug)]
pub struct Prometheus;

impl Life for Prometheus {
    const CANCEL: u32 = cancel::ABORTED;

    fn validate(settings: &[u8]) -> Result<(), Refusal> {
        crate::validate(settings).map_err(Refusal::failed)
    }

    fn open(_: &[u8], _: &[&[u8]], _: u64) -> Result<Self, Refusal> {
        Ok(Self)
    }

    fn refresh(&self, _: &[u8], _: &[&[u8]], _: u64) -> Result<Refreshed, Refusal> {
        Ok(Refreshed::default())
    }
}

/// THE ONE READ AND WRITE OF HOST-LENT POINTERS the safe SDK does not cover for this kind.
#[allow(unsafe_code)]
mod lent {
    use busbar_contract::abi::export::{
        ScrapeFamily, ScrapeIn, ScrapeLabel, ScrapeSample, SCRAPE_KIND_COUNTER, SCRAPE_KIND_GAUGE,
        SCRAPE_KIND_HISTOGRAM, SCRAPE_KIND_SUMMARY,
    };
    use busbar_contract::abi::mechanism::call::AbiStr;
    use busbar_contract::abi::sdk::{MetricFamily, MetricSample};

    /// `len` values at `p`, or none for a NULL `p`.
    ///
    /// # Safety
    /// A non-NULL `p` points at `len` initialized values, lent for the call.
    unsafe fn list<'a, T>(p: *const T, len: usize) -> &'a [T] {
        if p.is_null() || len == 0 {
            return &[];
        }
        // SAFETY: the caller's contract.
        std::slice::from_raw_parts(p, len)
    }

    /// The text `s` names; `None` when it is absent.
    ///
    /// # Safety
    /// A non-NULL `s.ptr` points at `s.len` bytes, lent for the call.
    unsafe fn text(s: AbiStr) -> Option<String> {
        if s.ptr.is_null() {
            return None;
        }
        // SAFETY: the caller's contract.
        let bytes = list(s.ptr, s.len);
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    fn kind(k: u8) -> &'static str {
        match k {
            SCRAPE_KIND_COUNTER => "counter",
            SCRAPE_KIND_GAUGE => "gauge",
            SCRAPE_KIND_HISTOGRAM => "histogram",
            SCRAPE_KIND_SUMMARY => "summary",
            _ => "untyped",
        }
    }

    /// The snapshot `input` lends, in the order the host gave it.
    pub(super) fn families(input: &ScrapeIn) -> Vec<MetricFamily> {
        // SAFETY (every read below): the export kind's ABI lends `families` and, inside each, its
        // `samples`, their `labels` and every string as lists and bytes valid for the call.
        let families: &[ScrapeFamily] = unsafe { list(input.families, input.families_len) };
        families
            .iter()
            .map(|f| {
                let samples: &[ScrapeSample] = unsafe { list(f.samples, f.samples_len) };
                MetricFamily {
                    name: unsafe { text(f.name) }.unwrap_or_default(),
                    kind: kind(f.kind).to_string(),
                    help: unsafe { text(f.help) },
                    samples: samples
                        .iter()
                        .map(|s| {
                            let labels: &[ScrapeLabel] = unsafe { list(s.labels, s.labels_len) };
                            MetricSample {
                                name: unsafe { text(s.name) }.unwrap_or_default(),
                                labels: labels
                                    .iter()
                                    .map(|l| unsafe {
                                        (
                                            text(l.key).unwrap_or_default(),
                                            text(l.value).unwrap_or_default(),
                                        )
                                    })
                                    .collect(),
                                value: unsafe { text(s.value) }.unwrap_or_default(),
                            }
                        })
                        .collect(),
                }
            })
            .collect()
    }

    /// Copy `bytes` into the host's buffer when they fit: `true` when written.
    pub(super) fn write(input: &ScrapeIn, bytes: &[u8]) -> bool {
        if bytes.len() > input.cap || (input.buf.is_null() && !bytes.is_empty()) {
            return false;
        }
        if !bytes.is_empty() {
            // SAFETY: the export kind's ABI lends `buf` as `cap` writable bytes for the call,
            // overlapping nothing else it lends; `bytes.len() <= cap`.
            unsafe { std::slice::from_raw_parts_mut(input.buf, bytes.len()) }
                .copy_from_slice(bytes);
        }
        true
    }
}

/// `scrape`: the snapshot rendered into the host's buffer.
pub struct Scrape;

impl SafeSlot for Scrape {
    type In = ScrapeIn;
    type Out = ScrapeOut;
    type State = Held<Prometheus>;
    fn call(
        _: Instance<'_, Held<Prometheus>>,
        input: Lent<'_, ScrapeIn>,
        mut out: Out<'_, ScrapeOut>,
    ) -> Outcome {
        let body = crate::render(&lent::families(input.get()));
        if lent::write(input.get(), body.as_bytes()) {
            out.set(|o| &o.written, body.len());
            Outcome::Ready
        } else {
            out.set(|o| &o.needed, body.len());
            Outcome::Failed
        }
    }
}

/// `deliver`: nothing kept — the host pulls `metrics`.
pub struct Deliver;

impl SafeSlot for Deliver {
    type In = DeliverIn;
    type Out = OutHead;
    type State = Held<Prometheus>;
    fn call(
        _: Instance<'_, Held<Prometheus>>,
        _: Lent<'_, DeliverIn>,
        _: Out<'_, OutHead>,
    ) -> Outcome {
        Outcome::Ready
    }
}

/// `status`: nothing to report.
pub struct Status;

impl SafeSlot for Status {
    type In = InHead;
    type Out = StatusOut;
    type State = Held<Prometheus>;
    fn call(
        _: Instance<'_, Held<Prometheus>>,
        _: Lent<'_, InHead>,
        _: Out<'_, StatusOut>,
    ) -> Outcome {
        Outcome::Ready
    }
}

/// `check`: the sink has no checks of its own.
pub struct Check;

impl SafeSlot for Check {
    type In = CheckIn;
    type Out = CheckOut;
    type State = Held<Prometheus>;
    fn call(
        _: Instance<'_, Held<Prometheus>>,
        _: Lent<'_, CheckIn>,
        _: Out<'_, CheckOut>,
    ) -> Outcome {
        Outcome::Ready
    }
}

/// `serve`: REFUSED — the host serves `/metrics`; the sink claims no route.
pub struct Serve;

impl SafeSlot for Serve {
    type In = ServeIn;
    type Out = ServeOut;
    type State = Held<Prometheus>;
    fn call(
        _: Instance<'_, Held<Prometheus>>,
        _: Lent<'_, ServeIn>,
        _: Out<'_, ServeOut>,
    ) -> Outcome {
        Outcome::Refused
    }
}

mod table {
    use super::{Check, Deliver, Prometheus, Safe, Scrape, Serve, Status};

    busbar_contract::plugin_door! {
        ops: busbar_contract::abi::export::Ops,
        statement: super::STATEMENT,
        lifecycle: life(Prometheus),
        kind_ops: {
            deliver: Safe<Deliver>, scrape: Safe<Scrape>, status: Safe<Status>,
            check: Safe<Check>, serve: Safe<Serve>,
        },
    }
}

/// This plugin's door: the one a compiled-in build links and the dropped-in image exports.
pub use table::door;
