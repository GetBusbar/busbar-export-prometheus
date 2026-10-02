<!-- fleet:header:begin (rendered by `busbar-release plugin sync` from GetBusbar/busbar-release template/ and busbar's plugins.yaml; edit it there) -->
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

Configured under the `prometheus` module name.

## Build

```bash
cargo build --release -p busbar-export-prometheus-plugin
```

## Tests

```bash
cargo test --workspace --locked
```

## License

Apache-2.0. See [LICENSE](LICENSE).
