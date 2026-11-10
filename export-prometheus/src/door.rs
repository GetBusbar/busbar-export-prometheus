// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE DOOR: the prometheus sink on the export kind's table (`busbar_contract::abi::export`), its
//! lifecycle the SDK's generic one over `Prometheus` (`abi::sdk::life`), every kind op a
//! `SafeSlot`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use zeroize::{Zeroize, Zeroizing};

use busbar_contract::abi::export::{
    cancel, CheckIn, CheckOut, DeliverIn, ExportStream, Route, ScrapeIn, ScrapeOut, ScrapeSample,
    ServeIn, ServeOut, Tail, CHECK_PHASE_INSTANCES, CHECK_PHASE_LIMITS, ROUTE_AUTH_KEY,
    SCRAPE_FLAG_HOOK_FAMILIES,
};
use busbar_contract::abi::host::conn::connector::{Need, DIRECTION_INBOUND};
use busbar_contract::abi::host::service::{SNAPSHOT_SCOPE_HOOKS, SNAPSHOT_SCOPE_WHOLE};
use busbar_contract::abi::mechanism::call::{AbiStr, OutHead, Outcome, BLOB_JSON, BLOB_OCTETS};
use busbar_contract::abi::mechanism::check::HARD_MAX_BYTES;
use busbar_contract::abi::mechanism::door::{KindTailHead, Rewrite, Statement, MARK_ONE_INSTANCE};
use busbar_contract::abi::mechanism::ticket::CompletionHandle;
use busbar_contract::abi::sdk::door::{abi_str, alias, statement};
use busbar_contract::abi::sdk::export::{instance_names, tail, Quiet};
use busbar_contract::abi::sdk::life::{Held, Life, Refreshed, Refusal};
use busbar_contract::abi::sdk::{
    Instance, Lent, MetricFamily, MetricSample, Out, Safe, SafeSlot, ServiceError, Services,
};

use crate::{ALIAS, NAME};

/// The one stream the sink carries.
const STREAMS: &[u8] = &[ExportStream::Metrics as u8];

/// The well-known exposition path.
const METRICS: &str = "/metrics";
/// The well-known hook exposition path.
const METRICS_HOOKS: &str = "/metrics/hooks";

/// `GET path` behind the data plane's key.
const fn get(path: &'static str) -> Route {
    Route {
        path: abi_str(path),
        method: abi_str("GET"),
        auth: ROUTE_AUTH_KEY,
        _reserved: 0,
    }
}

/// The sink's own routes: the two well-known expositions behind the data key.
const ROUTES: &[Route] = &[get(METRICS), get(METRICS_HOOKS)];

const TAIL: Tail = tail(STREAMS, ROUTES);

const NEEDS: &[Need] = &[{
    let mut need = Need::EMPTY;
    need.direction = DIRECTION_INBOUND;
    need.transport = abi_str("http");
    need.target_from = abi_str("listen");
    need
}];

/// The most calls one instance holds in flight: the `max_inflight` its Statement declares.
pub const MAX_INFLIGHT: u32 = 64;

/// The module name an operator writes, as the alias the registry holds beside [`NAME`].
pub(crate) const REWRITES: &[Rewrite] = &[alias(ALIAS)];

busbar_contract::statement_extensions!(EXTENSIONS: [
    (busbar_contract::export_calls::EXT_RECORDER_REQUIREMENTS, b"1"),
    (busbar_contract::abi::mechanism::extensions::EXT_PUBLIC_ROUTES, br#"["/metrics","/metrics/hooks"]"#),
    (busbar_contract::abi::mechanism::extensions::EXT_PERSISTENT_ROUTES, br#"["/metrics/hooks"]"#),
]);

/// This plugin's Statement: its name and version, the `metrics` stream, its two routes and its
/// alias.
pub const STATEMENT: Statement = Statement {
    settings_schema: busbar_contract::abi::mechanism::call::Blob {
        ptr: crate::SETTINGS_SCHEMA.as_ptr(),
        len: crate::SETTINGS_SCHEMA.len(),
        fmt: busbar_contract::abi::mechanism::call::BLOB_JSON,
        flags: 0,
    },
    kind_tail: (&TAIL as *const Tail).cast::<KindTailHead>(),
    rewrites: REWRITES.as_ptr(),
    rewrites_len: REWRITES.len(),
    extensions: EXTENSIONS,
    needs: NEEDS.as_ptr(),
    needs_len: NEEDS.len(),

    marks: MARK_ONE_INSTANCE,
    ..statement(NAME, env!("CARGO_PKG_VERSION"), MAX_INFLIGHT)
};

/// The buffer the first snapshot read starts with.
const FIRST_READ: usize = 64 * 1024;

/// The largest snapshot layout a short read may size its re-call buffer for. A layout spends at
/// most one `ScrapeSample` record (plus alignment) on a sample whose exposition line is at least
/// two bytes, and less per rendered byte on families and labels (a family's unit, which the
/// exposition omits, aside: the host's units are short words), so a layout past this many times
/// [`HARD_MAX_BYTES`] can only render past the export answer limit: it is refused before the
/// buffer is allocated rather than after the read and the render.
const MAX_LAYOUT: u64 = HARD_MAX_BYTES * std::mem::size_of::<ScrapeSample>() as u64;

/// Why a snapshot read failed: the host service's answer, or the sink's own read buffer.
enum ReadError {
    Service(ServiceError),
    /// The read buffer could not be allocated.
    NoMemory,
}

impl From<ServiceError> for ReadError {
    fn from(e: ServiceError) -> Self {
        Self::Service(e)
    }
}

fn read_buffer(bytes: usize) -> Result<Zeroizing<Vec<u64>>, ReadError> {
    let words = bytes.div_ceil(std::mem::size_of::<u64>());
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(words)
        .map_err(|_| ReadError::NoMemory)?;
    buffer.resize(words, 0);
    Ok(Zeroizing::new(buffer))
}

/// The plugin's generated family copies. Borrowed host input remains the host's, while every
/// initialized string copied here is wiped on every terminal path and unwind.
struct Families(Vec<MetricFamily>);

impl Drop for Families {
    fn drop(&mut self) {
        for f in &mut self.0 {
            f.name.zeroize();
            f.kind.zeroize();
            f.help.zeroize();
            for s in &mut f.samples {
                s.name.zeroize();
                s.labels.zeroize();
                s.value.zeroize();
            }
        }
    }
}

/// One opened instance: a scratch-buffer hint per snapshot scope, derived from that scope's last
/// successful snapshot size.
#[derive(Debug)]
pub struct Prometheus {
    recorder: busbar_contract::export_calls::RecorderRequirements,
    /// Indexed by [`hint_slot`]: WHOLE and HOOKS differ greatly in size, so neither sizes the other.
    read_hints: [AtomicUsize; 2],
}

/// The [`Prometheus::read_hints`] slot of `scope`.
fn hint_slot(scope: u32) -> usize {
    usize::from(scope == SNAPSHOT_SCOPE_HOOKS)
}

impl Prometheus {
    /// The host's families of `scope` through the host snapshot service, into a buffer of the last
    /// read's size; a short buffer earns one re-call with headroom on the same
    /// ticketed snapshot. Pending reads retain their host request and are polled
    /// on the same handle. `None` = not ready. Oversized layouts are refused.
    fn read(
        &self,
        services: &Services<busbar_contract::plugin::markers::ExportKind>,
        handle: CompletionHandle,
        scope: u32,
    ) -> Poll<Result<Option<Families>, ReadError>> {
        let hint = &self.read_hints[hint_slot(scope)];
        let mut buf = read_buffer(hint.load(Ordering::Relaxed))?;
        let read = match services.snapshot_read_with_used(handle, scope, &mut buf) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(ServiceError::Short { bytes, .. })) if bytes <= MAX_LAYOUT => {
                let want = usize::try_from(bytes).map_err(|_| ServiceError::Broken)?;
                let want = want.checked_add(want / 4).ok_or(ServiceError::Broken)?;
                buf = read_buffer(want)?;
                let Poll::Ready(read) = services.snapshot_read_with_used(handle, scope, &mut buf)
                else {
                    return Poll::Pending;
                };
                read
            }
            Poll::Ready(other) => other,
        };
        if let Ok(Some((_, used))) = &read {
            let next = used.checked_add(used / 4).ok_or(ServiceError::Broken)?;
            hint.store(next.max(FIRST_READ), Ordering::Relaxed);
        }
        Poll::Ready(Ok(
            read?.map(|(f, _used)| Families(f.into_iter().map(lift).collect()))
        ))
    }
}

/// The exposition's word for a host family kind; a kind the contract does not name is `untyped`.
fn kind_word(kind: u8) -> String {
    busbar_contract::export_calls::type_word(kind)
        .unwrap_or("untyped")
        .to_string()
}

/// One host family, as the renderer reads it. The `unit` field the exposition omits is wiped
/// before `f` drops, so it never reaches the allocator uncleared.
fn lift(mut f: busbar_contract::export_calls::Family) -> MetricFamily {
    f.unit.zeroize();
    MetricFamily {
        kind: kind_word(f.kind),
        name: std::mem::take(&mut f.name),
        help: f.help.take(),
        samples: std::mem::take(&mut f.samples)
            .into_iter()
            .map(|mut s| MetricSample {
                name: std::mem::take(&mut s.name),
                labels: std::mem::take(&mut s.labels),
                value: std::mem::take(&mut s.value),
            })
            .collect(),
    }
}

impl Life for Prometheus {
    type Kind = busbar_contract::plugin::markers::ExportKind;
    const CANCEL: u32 = cancel::ABORTED;

    fn validate(settings: &[u8]) -> Result<(), Refusal> {
        crate::validate(settings).map_err(Refusal::failed)
    }

    /// Refuses settings `validate` refuses, in its words: an instance never opens on settings the
    /// configuration would refuse.
    fn open(settings: &[u8], _: &[&[u8]], _: u64) -> Result<Self, Refusal> {
        let recorder = crate::recorder_requirements(settings).map_err(Refusal::failed)?;
        Ok(Self {
            recorder,
            read_hints: [AtomicUsize::new(FIRST_READ), AtomicUsize::new(FIRST_READ)],
        })
    }

    /// Refuses settings `validate` refuses, in its words; the instance stays as it was.
    fn refresh(&self, settings: &[u8], _: &[&[u8]], _: u64) -> Result<Refreshed, Refusal> {
        let recorder = crate::recorder_requirements(settings).map_err(Refusal::failed)?;
        if recorder != self.recorder {
            return Err(Refusal::refused(
                "host recorder requirements are fixed at startup; restart required",
            ));
        }
        Ok(Refreshed::default())
    }
}

/// The text `s` names, through the SDK's safe lend; `None` when it is absent (a NULL string).
/// Bytes that are not UTF-8 read lossily into a buffer sized for the worst case up front (an
/// invalid sequence of one or more bytes becomes one 3-byte U+FFFD), so the copy never
/// reallocates and so never frees an unwiped prefix of it.
fn text(s: Lent<'_, AbiStr>) -> Option<String> {
    if s.is_absent() {
        return None;
    }
    let bytes = s.bytes();
    if let Ok(t) = std::str::from_utf8(bytes) {
        return Some(t.to_owned());
    }
    let mut out = String::with_capacity(bytes.len().checked_mul(3)?);
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            out.push(char::REPLACEMENT_CHARACTER);
        }
    }
    Some(out)
}

/// The snapshot `input` lends, in the order the host gave it, read through the SDK's accessors;
/// `None` when a family name, sample name, label or sample value is absent. Every copy joins the
/// returned owner as soon as it exists, so an early `None` wipes what was already copied.
fn families(input: Lent<'_, ScrapeIn>) -> Option<Families> {
    let mut out = Families(Vec::new());
    for f in input.families().iter() {
        let name = text(f.field(|f| &f.name))?;
        out.0.push(MetricFamily {
            name,
            kind: kind_word(f.kind),
            help: text(f.field(|f| &f.help)),
            samples: Vec::new(),
        });
        let family = out.0.last_mut()?;
        for s in f.samples().iter() {
            let name = text(s.field(|s| &s.name))?;
            family.samples.push(MetricSample {
                name,
                labels: Vec::new(),
                value: String::new(),
            });
            let sample = family.samples.last_mut()?;
            for l in s.labels().iter() {
                let mut key = Zeroizing::new(text(l.field(|l| &l.key))?);
                let value = text(l.field(|l| &l.value))?;
                sample.labels.push((std::mem::take(&mut *key), value));
            }
            sample.value = text(s.field(|s| &s.value))?;
        }
    }
    Some(out)
}

/// `scrape`: the snapshot rendered into the host's buffer. A rendering past the export answer
/// limit, or a snapshot missing a required text, is FAILED with words, never answered short.
pub struct Scrape;

impl SafeSlot for Scrape {
    type Kind = busbar_contract::plugin::markers::ExportKind;
    type In = ScrapeIn;
    type Out = ScrapeOut;
    type State = Held<Prometheus>;
    fn call(
        _: Instance<'_, Held<Prometheus>, busbar_contract::plugin::markers::ExportKind>,
        input: Lent<'_, ScrapeIn>,
        mut out: Out<'_, ScrapeOut>,
    ) -> Outcome {
        let Some(families) = families(input) else {
            return out.fail(Refusal::failed(SCRAPE_UNNAMED));
        };
        let body = if input.head.flags & SCRAPE_FLAG_HOOK_FAMILIES != 0 {
            crate::render_hooks_owned(&families.0)
        } else {
            crate::render_owned(&families.0)
        };
        if body.len() as u64 > HARD_MAX_BYTES {
            return out.fail(Refusal::failed(OVER_LIMIT));
        }
        let mut buf = input.buf();

        if body.len() > buf.cap() {
            out.set(|o| &o.needed, body.len());
            return Outcome::Failed;
        }
        buf.extend(body.as_bytes());
        out.set(|o| &o.written, buf.written());
        Outcome::Ready
    }
}

/// `deliver`: nothing kept — the host pulls `metrics`.
pub struct Deliver;

impl SafeSlot for Deliver {
    type Kind = busbar_contract::plugin::markers::ExportKind;
    type In = DeliverIn;
    type Out = OutHead;
    type State = Held<Prometheus>;
    fn call(
        instance: Instance<'_, Held<Prometheus>, busbar_contract::plugin::markers::ExportKind>,
        _: Lent<'_, DeliverIn>,
        mut out: Out<'_, OutHead>,
    ) -> Outcome {
        if instance.get().is_none() {
            return out.fail(Refusal::refused(NO_INSTANCE));
        }
        Outcome::Ready
    }
}

// The error texts of `deliver`, `scrape`, `serve` and `check`: one per cause, so the
// host's warn names why instead of a bare outcome.

/// A kind op called on a slot with no opened instance.
pub const NO_INSTANCE: &str = "no opened prometheus instance";
/// `check` at a phase this sink does not know.
pub const UNKNOWN_PHASE: &str = "not a check phase this sink knows (limits, instances or recorder)";
const NOT_A_ROUTE: &str = "not a route this sink serves (only GET /metrics and /metrics/hooks)";
const NO_SERVICES: &str =
    "the host offers this sink no services: the exposition cannot read the snapshot";
const READ_UNSERVED: &str =
    "the host serves no snapshot.read: the exposition cannot read the snapshot";
const READ_REFUSED: &str = "the host refused the snapshot read";
const READ_FAILED: &str = "the host failed the snapshot read";
const READ_BROKEN: &str = "the host's snapshot broke the service's rules";
const READ_SHORT: &str = "the snapshot outgrew the buffer on the one re-call";
const READ_NO_MEMORY: &str = "the sink could not allocate the snapshot read buffer";
const OVER_LIMIT: &str = "the rendered snapshot exceeds the export answer limit";
const SCRAPE_UNNAMED: &str =
    "the host's scrape snapshot lacks a family name, sample name, label or sample value";

/// `check`: at the limits phase, a second instance is refused.
pub struct Check;

impl SafeSlot for Check {
    type Kind = busbar_contract::plugin::markers::ExportKind;
    type In = CheckIn;
    type Out = CheckOut;
    type State = Held<Prometheus>;
    fn call(
        instance: Instance<'_, Held<Prometheus>, busbar_contract::plugin::markers::ExportKind>,
        input: Lent<'_, CheckIn>,
        mut out: Out<'_, CheckOut>,
    ) -> Outcome {
        let Some(h) = instance.get() else {
            return out.fail(Refusal::refused(NO_INSTANCE));
        };
        match input.phase {
            busbar_contract::abi::export::CHECK_PHASE_RECORDER => {
                return busbar_contract::abi::sdk::export::declare_recorder(
                    h,
                    out,
                    h.life().recorder,
                );
            }
            CHECK_PHASE_LIMITS => {}

            CHECK_PHASE_INSTANCES => return Outcome::Ready,

            _ => return out.fail(Refusal::refused(UNKNOWN_PHASE)),
        }
        let lines = crate::check_limits(&instance_names(input));
        if !lines.is_empty() {
            let json = serde_json::to_vec(&lines).unwrap_or_default();
            out.lease(|o| &o.findings, h.leases(), json, BLOB_JSON);
        }
        Outcome::Ready
    }
}

/// `/metrics`' response headers.
const METRICS_HEADERS: &[AbiStr] = &[
    abi_str("content-type"),
    abi_str("text/plain; version=0.0.4"),
];
/// `/metrics/hooks`' response headers; this content type carries a charset.
const HOOKS_HEADERS: &[AbiStr] = &[
    abi_str("content-type"),
    abi_str("text/plain; version=0.0.4; charset=utf-8"),
];
/// The not-ready answer's headers: retry in a second.
const NOT_READY_HEADERS: &[AbiStr] = &[abi_str("retry-after"), abi_str("1")];

/// What one route answers from: the snapshot scope it reads, its response headers and its
/// renderer.
type Exposition = (
    u32,
    &'static [AbiStr],
    fn(&[MetricFamily]) -> Zeroizing<String>,
);

/// `serve`: the sink's own two routes, over the host snapshot service.
pub struct Serve;

impl SafeSlot for Serve {
    type Kind = busbar_contract::plugin::markers::ExportKind;
    type In = ServeIn;
    type Out = ServeOut;
    type State = Held<Prometheus>;
    fn call(
        instance: Instance<'_, Held<Prometheus>, busbar_contract::plugin::markers::ExportKind>,
        input: Lent<'_, ServeIn>,
        mut out: Out<'_, ServeOut>,
    ) -> Outcome {
        let Some(h) = instance.get() else {
            return out.fail(Refusal::refused(NO_INSTANCE));
        };
        let (scope, headers, render): Exposition = match input.field(|i| &i.path).as_str() {
            Ok(METRICS) => (SNAPSHOT_SCOPE_WHOLE, METRICS_HEADERS, crate::render_owned),
            Ok(METRICS_HOOKS) => (
                SNAPSHOT_SCOPE_HOOKS,
                HOOKS_HEADERS,
                crate::render_hooks_owned,
            ),
            _ => return out.fail(Refusal::refused(NOT_A_ROUTE)),
        };
        let Some(services) = h.host().and_then(|host| host.services()) else {
            out.error(NO_SERVICES);
            return Outcome::Failed;
        };
        let handle = CompletionHandle {
            ticket: instance.ticket(),
            seq: 0,
            ..CompletionHandle::ZERO
        };
        let Poll::Ready(read) = h.life().read(&services, handle, scope) else {
            return Outcome::Pending;
        };
        let (status, headers, body) = match read {
            Ok(Some(families)) => (200u16, headers, render(&families.0)),
            Ok(None) => (503u16, NOT_READY_HEADERS, Zeroizing::new(String::new())),
            Err(e) => {
                let cause = match e {
                    ReadError::NoMemory => READ_NO_MEMORY,
                    ReadError::Service(ServiceError::Unserved) => READ_UNSERVED,
                    ReadError::Service(ServiceError::Declined(Outcome::Refused)) => READ_REFUSED,
                    ReadError::Service(ServiceError::Declined(_)) => READ_FAILED,
                    ReadError::Service(ServiceError::Broken) => READ_BROKEN,
                    ReadError::Service(ServiceError::Short { bytes, .. }) if bytes > MAX_LAYOUT => {
                        OVER_LIMIT
                    }
                    ReadError::Service(ServiceError::Short { .. }) => READ_SHORT,
                };
                let scope = if scope == SNAPSHOT_SCOPE_HOOKS {
                    "HOOKS"
                } else {
                    "WHOLE"
                };
                return out.fail(Refusal::failed(format!("{cause} (scope {scope})")));
            }
        };
        if body.len() as u64 > HARD_MAX_BYTES {
            return out.fail(Refusal::failed(OVER_LIMIT));
        }
        out.set(|o| &o.status_code, status);

        out.list(|o| &o.headers_out, |o| &o.headers_out_len, headers);
        if body.is_empty() {
            out.keep(h.leases(), ());
        } else {
            let mut body = body;
            out.lease_secret(
                |o| &o.body,
                h.leases(),
                std::mem::take(&mut *body).into_bytes(),
                BLOB_OCTETS,
            );
        }
        Outcome::Ready
    }
}

mod table {
    use super::{Check, Deliver, Prometheus, Quiet, Safe, Scrape, Serve};

    busbar_contract::plugin_door! {
        ops: busbar_contract::abi::export::Ops,
        statement: super::STATEMENT,
        lifecycle: life(Prometheus),
        kind_ops: {
            deliver: Safe<Deliver>, scrape: Safe<Scrape>, status: Safe<Quiet<Prometheus>>,
            check: Safe<Check>, serve: Safe<Serve>,
        },
    }
}

/// This plugin's door: the one a compiled-in build links and the dropped-in image exports.
pub use table::{door, MACRO_DOOR};
