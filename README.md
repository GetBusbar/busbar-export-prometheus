<!-- fleet:header:begin (rendered by `busbar-release plugin heal` from GetBusbar/busbar-release template/ and busbar's plugins.yaml; edit it there) -->
# busbar-export-prometheus

First-party signed kind:export plugin cdylib: the prometheus export sink (module: prometheus), packaged as a droppable busbar plugin. Drop the signed tarball into plugins/ and name it from an export.<name>.module: prometheus block.

| kind | alias | crate | busbar | license |
|---|---|---|---|---|
| `export` | `prometheus` | `busbar-export-prometheus-plugin` | 1.6.0 (pinned in `.busbar-ref`) | Apache-2.0 |

[![ci](https://github.com/GetBusbar/busbar-export-prometheus/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/GetBusbar/busbar-export-prometheus/actions/workflows/ci.yml)
<!-- fleet:header:end -->

## What it is for

`busbar-export-prometheus` serves the host recorder's metrics and hook metrics on the data
listener. It is a pull sink: it reads the host snapshot service and opens no sockets of its own.
The same implementation works linked into busbar or loaded as a signed plugin.

## Config

Configured under the `prometheus` module name, as an `export.<name>.module: prometheus` block.

Settings:

- `buffer_seconds` (required): the retention window of the rolling quantile summary, in seconds. It must be positive; `0` is refused, because it retains no observations and every scrape would report empty quantiles while still paying the recording cost. To turn metrics off, remove the instance.
- `key_gauge_limit` (optional): the host's bound on per-key scrape-time gauges (2000 when omitted). The sink validates its type; the host applies the value.

Any other setting is refused.

At most one `module: prometheus` instance may be configured; a second is refused, since the sink serves the one well-known `/metrics` route.

Routes, both served on the data listener behind the data key:

- `GET /metrics`: the host's metric families as the Prometheus text exposition (`text/plain; version=0.0.4`). Before the host's recorder is installed it answers `503` with `Retry-After: 1`.
- `GET /metrics/hooks`: the host's hook metric families (`text/plain; version=0.0.4; charset=utf-8`). The boot hook snapshot is available before the whole recorder and answers `200`, including an empty body when no hook reports metrics.

The Statement's inbound HTTP need names the existing root `listen` data binding. It adds no
listener setting. The operator's data authentication chain governs this binding; both routes
retain their data-key policy. A host snapshot service that explicitly reports `NotReady` for
either scope gets a `503` with `Retry-After: 1`; the scripted service controls test that fallback
without claiming it is the production hook boot state.

The Statement declares these public paths through `listener.public_routes`; the host grants
them only to its existing trusted snapshot sink. `listener.persistent_routes` declares
`/metrics/hooks` for the boot sink's lifetime. `/metrics` follows the configured instance's
lifetime, so removing that instance turns its route off. Both paths keep their data-key
authentication.

Labels, values and HELP text retain the host's existing escaping. The whole-recorder exposition
is sorted into a stable family and series order; the hook exposition keeps each family's sample
order and sorts families by name. The plugin does not collect or persist metrics itself.

The export ABI limits one response body to 16 MiB. A larger rendered snapshot fails with an
explanation instead of returning a partial exposition. Snapshot decoding and rendering still
materialize owned data; this plugin does not promise allocation-free scrapes.

## Differences from 1.5.5

- A rendered exposition larger than 16 MiB is failed with an explanation
  (`the rendered snapshot exceeds the export answer limit`), on `GET /metrics`, on
  `GET /metrics/hooks` and on the host's `scrape`. A host snapshot whose reported layout could
  only render past that limit is refused before any buffer is allocated for it. 1.5.5
  (`crates/busbar/src/export/prometheus.rs:43`) rendered the in-process recorder straight into
  the response with no size bound. The export ABI caps one
  answer at 16 MiB, and a bounded answer keeps a runaway recorder from tying up unbounded memory.

## Build

```bash
cargo build --release -p busbar-export-prometheus-plugin --features dropped-in
```

## Tests

```bash
cargo test --workspace --locked
```

## License

Apache-2.0. See [LICENSE](LICENSE).
