// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE DOOR: the prometheus sink on the export kind's table (`busbar_contract::abi::export`), its
//! lifecycle the SDK's generic one over [`Prometheus`] (`abi::sdk::life`), every kind op a
//! [`SafeSlot`].
//!
//! * `validate` — [`crate::validate`]: the configuration's words, the zero-retention refusal included.
//! * `open` / `refresh` — nothing to hold: every scrape carries the whole snapshot it renders.
//! * `scrape` — the snapshot rendered ([`crate::render`]) into the host's buffer; a buffer too
//!   small is FAILED with the bytes `needed`, nothing written, and the host calls again once. With
//!   `SCRAPE_FLAG_HOOK_FAMILIES` on the head the families are the host's hook families and render
//!   as 1.5.5's `/metrics/hooks` ([`crate::render_hooks`]).
//! * `deliver` — READY, nothing kept: the host pulls `metrics`, it never pushes them.
//! * `status` and `check` — READY with nothing to report. `serve` — REFUSED: no route of its own
//!   (the host serves `/metrics`).
//!
//! No `unsafe` here: the snapshot and the buffer `scrape` is lent are read and written through the
//! SDK's safe lend (`families()`, `samples()`, `labels()`, `buf()`).

use busbar_contract::abi::export::{
    cancel, CheckIn, CheckOut, DeliverIn, ExportStream, ScrapeIn, ScrapeOut, ServeIn, ServeOut,
    StatusOut, Tail, SCRAPE_FLAG_HOOK_FAMILIES,
};
use busbar_contract::abi::mechanism::call::{AbiStr, InHead, OutHead, Outcome};
use busbar_contract::abi::mechanism::door::{KindTailHead, Statement};
use busbar_contract::abi::sdk::door::statement;
use busbar_contract::abi::sdk::life::{Held, Life, Refreshed, Refusal};
use busbar_contract::abi::sdk::{Instance, Lent, MetricFamily, MetricSample, Out, Safe, SafeSlot};

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

/// The text `s` names, through the SDK's safe lend; `None` when it is absent (a NULL string).
fn text(s: Lent<'_, AbiStr>) -> Option<String> {
    (!s.ptr.is_null()).then(|| String::from_utf8_lossy(s.bytes()).into_owned())
}

/// The snapshot `input` lends, in the order the host gave it, read through the SDK's accessors.
fn families(input: Lent<'_, ScrapeIn>) -> Vec<MetricFamily> {
    input
        .families()
        .iter()
        .map(|f| MetricFamily {
            name: text(f.field(|f| &f.name)).unwrap_or_default(),
            kind: busbar_contract::export_calls::type_word(f.kind)
                .unwrap_or("untyped")
                .to_string(),
            help: text(f.field(|f| &f.help)),
            samples: f
                .samples()
                .iter()
                .map(|s| MetricSample {
                    name: text(s.field(|s| &s.name)).unwrap_or_default(),
                    labels: s
                        .labels()
                        .iter()
                        .map(|l| {
                            (
                                text(l.field(|l| &l.key)).unwrap_or_default(),
                                text(l.field(|l| &l.value)).unwrap_or_default(),
                            )
                        })
                        .collect(),
                    value: text(s.field(|s| &s.value)).unwrap_or_default(),
                })
                .collect(),
        })
        .collect()
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
        let families = families(input);
        let body = if input.head.flags & SCRAPE_FLAG_HOOK_FAMILIES != 0 {
            crate::render_hooks(&families)
        } else {
            crate::render(&families)
        };
        let mut buf = input.buf();
        buf.extend(body.as_bytes());
        if buf.fits() {
            out.set(|o| &o.written, buf.written());
            Outcome::Ready
        } else {
            out.set(|o| &o.needed, buf.needed());
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
