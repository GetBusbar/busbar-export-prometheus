<!-- fleet:header:begin (rendered by `busbar-release plugin heal` from GetBusbar/busbar-release template/ and busbar's plugins.yaml; edit it there) -->
# busbar-export-prometheus

First-party signed kind:export plugin cdylib: the prometheus export sink (module: prometheus), packaged as a droppable busbar plugin. Drop the signed tarball into plugins/ and name it from an export.<name>.module: prometheus block.

| kind | alias | crate | busbar | license |
|---|---|---|---|---|
| `export` | `prometheus` | `busbar-export-prometheus-plugin` | 1.6.0 (pinned in `.busbar-ref`) | Apache-2.0 |

[![ci](https://github.com/GetBusbar/busbar-export-prometheus/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/GetBusbar/busbar-export-prometheus/actions/workflows/ci.yml)
<!-- fleet:header:end -->

## What it is for

`busbar-export-prometheus` is a `kind: export` busbar plugin.

## Config

Configured under the `prometheus` module name, as an `export.<name>.module: prometheus` block.

Settings:

- `buffer_seconds` (required): the retention window of the rolling quantile summary, in seconds. It must be positive; `0` is refused, because it retains no observations and every scrape would report empty quantiles while still paying the recording cost. To turn metrics off, remove the instance.
- `key_gauge_limit` (optional): the bound on per-key scrape-time gauges.

Any other setting is refused.

At most one `module: prometheus` instance may be configured; a second is refused, since the sink serves the one well-known `/metrics` route.

Routes, both served on the data listener behind the data key:

- `GET /metrics`: the host's metric families as the Prometheus text exposition (`text/plain; version=0.0.4`). Before the host's recorder is installed it answers `503` with `Retry-After: 1`.
- `GET /metrics/hooks`: the host's hook metric families (`text/plain; version=0.0.4; charset=utf-8`).

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
