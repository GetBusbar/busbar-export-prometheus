// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE DOOR: the prometheus sink on the export kind's table (`busbar_contract::abi::export`), its
//! lifecycle the SDK's generic one over [`Prometheus`] (`abi::sdk::life`), every kind op a
//! [`SafeSlot`].
//!
//! * `validate` — [`crate::validate`]: the configuration's words, the zero-retention refusal included.
//! * `open` / `refresh` — nothing to hold but the size the last snapshot took (the buffer the next
//!   read starts with).
//! * `serve` — THE SINK'S OWN ROUTES (owner law 2026-09-27: "/metrics and /metrics/hooks leave
//!   core"; busbar ARCHITECT Q-U2-4). The Statement declares `GET /metrics` and `GET /metrics/hooks`
//!   behind the data key; `serve` reads the host's families through the host snapshot service
//!   (`snapshot.read`, scope WHOLE or HOOKS) and answers 1.5.5's bytes: `200` with the exposition
//!   ([`crate::render`]) under `text/plain; version=0.0.4`, or the hook exposition
//!   ([`crate::render_hooks`]) under `text/plain; version=0.0.4; charset=utf-8`; NOT READY (the
//!   host's recorder is not installed yet) is `503` with `Retry-After: 1` and no body. A read the
//!   host declines is FAILED (the host answers `502`).
//! * `scrape` — a snapshot the host hands in, rendered ([`crate::render`]) into the host's buffer; a
//!   buffer too small is FAILED with the bytes `needed`, nothing written, and the host calls again
//!   once. With `SCRAPE_FLAG_HOOK_FAMILIES` on the head the families are the host's hook families
//!   and render as 1.5.5's `/metrics/hooks` ([`crate::render_hooks`]).
//! * `deliver` — READY, nothing kept: the host pulls `metrics`, it never pushes them.
//! * `status` — READY with nothing to report. `check` — at the limits phase, 1.5.5's refusal of
//!   every instance after the first ([`crate::check_limits`]): the sink states the `one_instance`
//!   mark, so the host asks it while the configuration is resolved; READY with nothing otherwise.
//!
//! The one `unsafe` here is [`lent`]: the export kind's `ScrapeIn` lends its families and its
//! buffer as raw pointers, and the SDK states no safe accessor for them.

use std::sync::atomic::{AtomicUsize, Ordering};

use busbar_contract::abi::export::{
    cancel, CheckIn, CheckOut, DeliverIn, ExportStream, Route, ScrapeIn, ScrapeOut, ServeIn,
    ServeOut, StatusOut, Tail, CHECK_PHASE_LIMITS, ROUTE_AUTH_KEY, SCRAPE_FLAG_HOOK_FAMILIES,
};
use busbar_contract::abi::host::service::{SNAPSHOT_SCOPE_HOOKS, SNAPSHOT_SCOPE_WHOLE};
use busbar_contract::abi::mechanism::call::{
    AbiStr, InHead, OutHead, Outcome, BLOB_JSON, BLOB_OCTETS,
};
use busbar_contract::abi::mechanism::door::{KindTailHead, Statement, MARK_ONE_INSTANCE};
use busbar_contract::abi::mechanism::ticket::{CompletionHandle, Ticket};
use busbar_contract::abi::sdk::door::statement;
use busbar_contract::abi::sdk::life::{Held, Life, Refreshed, Refusal};
use busbar_contract::abi::sdk::{Instance, Lent, Out, Safe, SafeSlot};
use busbar_contract::abi::sdk::{MetricFamily, MetricSample, ServiceError, Services};

use crate::NAME;

/// The one stream the sink carries.
const STREAMS: &[u8] = &[ExportStream::Metrics as u8];

/// A `'static` string as the ABI names it.
const fn text(s: &'static str) -> AbiStr {
    AbiStr {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

/// The well-known exposition path.
const METRICS: &str = "/metrics";
/// The well-known hook exposition path.
const METRICS_HOOKS: &str = "/metrics/hooks";

/// `GET path` behind the data plane's key.
const fn get(path: &'static str) -> Route {
    Route {
        path: text(path),
        method: text("GET"),
        auth: ROUTE_AUTH_KEY,
        _reserved: 0,
    }
}

/// The sink's own routes: the two well-known expositions, 1.5.5's paths behind the data key.
const ROUTES: &[Route] = &[get(METRICS), get(METRICS_HOOKS)];

const TAIL: Tail = Tail {
    head: KindTailHead {
        size: std::mem::size_of::<Tail>() as u32,
        _reserved: 0,
    },
    streams: STREAMS.as_ptr(),
    streams_len: STREAMS.len(),
    routes: ROUTES.as_ptr(),
    routes_len: ROUTES.len(),
};

/// This plugin's Statement: its name and version, the `metrics` stream and its two routes.
pub const STATEMENT: Statement = Statement {
    kind_tail: (&TAIL as *const Tail).cast::<KindTailHead>(),
    // One scrape sink renders the ONE well-known `/metrics`: at most one instance.
    marks: MARK_ONE_INSTANCE,
    ..statement(NAME, env!("CARGO_PKG_VERSION"), 64)
};

/// The buffer the first snapshot read starts with.
const FIRST_READ: usize = 64 * 1024;

/// One opened instance: it holds only the size the last snapshot took, the buffer the next read
/// starts with.
#[derive(Debug)]
pub struct Prometheus {
    read_hint: AtomicUsize,
}

impl Prometheus {
    /// The host's families of `scope` through the host snapshot service, into a buffer of the last
    /// read's size; a short buffer earns the one re-call, with headroom (a ticket-less re-call reads
    /// the snapshot again, which may have grown). `None` = not ready.
    fn read(
        &self,
        services: &Services,
        scope: u32,
    ) -> Result<Option<Vec<MetricFamily>>, ServiceError> {
        let none = CompletionHandle {
            ticket: Ticket::NONE,
            seq: 0,
            _reserved: 0,
        };
        let mut buf = vec![0u64; self.read_hint.load(Ordering::Relaxed).div_ceil(8)];
        let read = match services.snapshot_read(none, scope, &mut buf) {
            Err(ServiceError::Short { bytes, .. }) => {
                let want = usize::try_from(bytes).unwrap_or(usize::MAX / 2);
                let want = want.saturating_add(want / 4);
                self.read_hint.store(want, Ordering::Relaxed);
                buf = vec![0u64; want.div_ceil(8)];
                services.snapshot_read(none, scope, &mut buf)
            }
            other => other,
        };
        read.map(|families| families.map(|f| f.into_iter().map(lift).collect()))
    }
}

/// One host family, as the renderer reads it.
fn lift(f: busbar_contract::export_calls::Family) -> MetricFamily {
    MetricFamily {
        kind: busbar_contract::export_calls::type_word(f.kind)
            .unwrap_or("untyped")
            .to_string(),
        name: f.name,
        help: f.help,
        samples: f
            .samples
            .into_iter()
            .map(|s| MetricSample {
                name: s.name,
                labels: s.labels,
                value: s.value,
            })
            .collect(),
    }
}

impl Life for Prometheus {
    const CANCEL: u32 = cancel::ABORTED;

    fn validate(settings: &[u8]) -> Result<(), Refusal> {
        crate::validate(settings).map_err(Refusal::failed)
    }

    fn open(_: &[u8], _: &[&[u8]], _: u64) -> Result<Self, Refusal> {
        Ok(Self {
            read_hint: AtomicUsize::new(FIRST_READ),
        })
    }

    fn refresh(&self, _: &[u8], _: &[&[u8]], _: u64) -> Result<Refreshed, Refusal> {
        Ok(Refreshed::default())
    }
}

/// THE ONE READ AND WRITE OF HOST-LENT POINTERS the safe SDK does not cover for this kind.
#[allow(unsafe_code)]
mod lent {
    use busbar_contract::abi::export::{
        CheckIn, ScrapeFamily, ScrapeIn, ScrapeLabel, ScrapeSample, SCRAPE_KIND_COUNTER,
        SCRAPE_KIND_GAUGE, SCRAPE_KIND_HISTOGRAM, SCRAPE_KIND_SUMMARY,
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

    /// `check`'s instance names, in configuration order.
    pub(super) fn instance_names(input: &CheckIn) -> Vec<String> {
        // SAFETY (both reads): the export kind's ABI lends `instances` and every string they name
        // for the call.
        let list = unsafe { list(input.instances, input.instances_len) };
        list.iter()
            .map(|c| unsafe { text(c.name) }.unwrap_or_default())
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
        let families = lent::families(input.get());
        let body = if input.get().head.flags & SCRAPE_FLAG_HOOK_FAMILIES != 0 {
            crate::render_hooks(&families)
        } else {
            crate::render(&families)
        };
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
        instance: Instance<'_, Held<Prometheus>>,
        input: Lent<'_, CheckIn>,
        mut out: Out<'_, CheckOut>,
    ) -> Outcome {
        let Some(h) = instance.get() else {
            return Outcome::Refused;
        };
        if input.get().phase != CHECK_PHASE_LIMITS {
            return Outcome::Ready;
        }
        let lines = crate::check_limits(&lent::instance_names(input.get()));
        if !lines.is_empty() {
            let json = serde_json::to_vec(&lines).unwrap_or_default();
            out.lease(|o| &o.findings, h.leases(), json, BLOB_JSON);
        }
        Outcome::Ready
    }
}

/// `/metrics`' response headers: 1.5.5's content type.
const METRICS_HEADERS: &[AbiStr] = &[text("content-type"), text("text/plain; version=0.0.4")];
/// `/metrics/hooks`' response headers: 1.5.5's content type (the hook exposition carried a
/// charset).
const HOOKS_HEADERS: &[AbiStr] = &[
    text("content-type"),
    text("text/plain; version=0.0.4; charset=utf-8"),
];
/// The not-ready answer's headers: retry in a second.
const NOT_READY_HEADERS: &[AbiStr] = &[text("retry-after"), text("1")];

/// A renderer of the host's families.
type Render = fn(&[MetricFamily]) -> String;

/// `serve`: the sink's own two routes, over the host snapshot service.
pub struct Serve;

impl SafeSlot for Serve {
    type In = ServeIn;
    type Out = ServeOut;
    type State = Held<Prometheus>;
    fn call(
        instance: Instance<'_, Held<Prometheus>>,
        input: Lent<'_, ServeIn>,
        mut out: Out<'_, ServeOut>,
    ) -> Outcome {
        let Some(h) = instance.get() else {
            return Outcome::Refused;
        };
        let (scope, headers, render): (u32, &'static [AbiStr], Render) =
            match input.field(|i| &i.path).as_str() {
                Ok(METRICS) => (SNAPSHOT_SCOPE_WHOLE, METRICS_HEADERS, crate::render),
                Ok(METRICS_HOOKS) => (SNAPSHOT_SCOPE_HOOKS, HOOKS_HEADERS, crate::render_hooks),
                _ => return Outcome::Refused,
            };
        let Some(services) = h.host().and_then(|host| host.services()) else {
            return Outcome::Failed;
        };
        match h.life().read(&services, scope) {
            Ok(Some(families)) => {
                out.set(|o| &o.status_code, 200u16);
                out.list(|o| &o.headers_out, |o| &o.headers_out_len, headers);
                let body = render(&families).into_bytes();
                if body.is_empty() {
                    // An empty exposition leases no body: the static headers still name a lease.
                    out.keep(h.leases(), ());
                } else {
                    out.lease(|o| &o.body, h.leases(), body, BLOB_OCTETS);
                }
                Outcome::Ready
            }
            Ok(None) => {
                out.set(|o| &o.status_code, 503u16);
                out.list(
                    |o| &o.headers_out,
                    |o| &o.headers_out_len,
                    NOT_READY_HEADERS,
                );
                // The headers are program memory and no body is leased: the answer still names a
                // lease, as the kind's check requires of an answer that names headers.
                out.keep(h.leases(), ());
                Outcome::Ready
            }
            Err(_) => Outcome::Failed,
        }
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
