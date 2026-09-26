// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! **ONE SCRAPE SINK, BOTH DOORS, ONE ROW** — the prometheus sink's linked + dropped-in conformance,
//! run against the busbar rev this repo pins (`.busbar-ref`).
//!
//! The sink is held two ways at once: LINKED (its `linked::EXPORT` statement and boundary, the row a
//! busbar build that compiles it in registers) and DROPPED IN (this crate's built cdylib, signed
//! first-party under the SAME statement into a temp `plugins/` directory and found by the loader's
//! scan). Each arm is opened by the one `open_export` and driven through one script: the row it
//! states and what the host grants it, the streams it carries and the routes it claims, its settings
//! refusals (the configuration's own words, the zero-retention refusal included), and a scrape — the
//! recorder snapshot of an exposition carrying every family type the recorder writes, rendered by
//! the sink and served by the host's one scrape answer. The two arms must agree byte for byte, and
//! the bytes served must be the recorder's own.
//!
//! The RED arms are in the same test: the same cdylib dropped in under a THIRD-PARTY signature is a
//! different row (not first-party, another publisher) and its transcript diverges; and a host with no
//! scrape sink serves the recorder's text under its own content type, which is not what either door
//! serves.
//!
//! Ported from busbar's `crates/busbar/src/root/tests/linked_scrape.rs` (K9d) and the kernel's
//! `export/tests/scrape_tests.rs`, where the sink was proven before it moved to this repo; busbar
//! still runs those tests against the pinned sink.

use busbar_plugin_loader::sign::{sign, Manifest, SigningKey, TrustPolicy};
use busbar_plugin_loader::{LinkedPlugin, PluginRegistry};

/// The release key the first-party dropped-in arm is signed with, and the policy's first-party key.
fn release() -> SigningKey {
    SigningKey::from_bytes(&[31u8; 32])
}

/// The version both arms state (a linked row states its binary's version; here, this crate's).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The settings every opened sink is handed.
const SETTINGS: &str = r#"{"buffer_seconds":60}"#;

/// The recorder exposition the scrape renders: every family type the recorder writes — a HELP-less
/// counter with escaped label values, a histogram, a gauge, a quantile summary — never empty, so the
/// proof is not vacuous.
const OWN: &str = "# TYPE busbar_requests_total counter\n\
                   busbar_requests_total{pool=\"a\\\"b\\\\c\\nd\",outcome=\"ok\"} 3\n\
                   \n\
                   # HELP busbar_request_duration_seconds request latency\n\
                   # TYPE busbar_request_duration_seconds histogram\n\
                   busbar_request_duration_seconds_bucket{le=\"0.5\"} 1\n\
                   busbar_request_duration_seconds_bucket{le=\"+Inf\"} 2\n\
                   busbar_request_duration_seconds_sum 0.75\n\
                   busbar_request_duration_seconds_count 2\n\
                   \n\
                   # TYPE busbar_conformance_gauge gauge\n\
                   busbar_conformance_gauge 1.5\n\
                   \n\
                   # TYPE busbar_plane_request_duration_seconds summary\n\
                   busbar_plane_request_duration_seconds{quantile=\"0.99\"} 0.0125\n\
                   busbar_plane_request_duration_seconds_sum 1e-3\n\
                   busbar_plane_request_duration_seconds_count 4\n\
                   \n";

/// This crate's built cdylib (uplifted or under `deps`, newest wins). A missing artifact is a
/// failure, never a skip: this test IS the dropped-in door's proof.
fn cdylib() -> Vec<u8> {
    let exe = std::env::current_exe().expect("the test binary has a path");
    let profile = exe
        .parent()
        .and_then(|d| d.parent())
        .expect("target/<profile>");
    let file = busbar_plugin_loader::plugin_library_filename("busbar_export_prometheus_plugin");
    let found = [profile.join(&file), profile.join("deps").join(&file)]
        .into_iter()
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max()
        .map(|(_, p)| p)
        .unwrap_or_else(|| {
            panic!("the busbar-export-prometheus-plugin cdylib ({file}) is not built")
        });
    std::fs::read(found).expect("read the cdylib")
}

/// The LINKED row: exactly what busbar's composition root states for `linked::EXPORT`.
fn linked_row() -> LinkedPlugin {
    let (name, alias, declares, entry) = busbar_export_prometheus::linked::EXPORT;
    let abi = busbar_plugin_loader::supported_abi("export")
        .iter()
        .copied()
        .max()
        .unwrap_or_default();
    let manifest = Manifest {
        name: name.into(),
        alias: alias.into(),
        kind: "export".into(),
        version: VERSION.into(),
        publisher: busbar_plugin_loader::sign::FIRST_PARTY_PUBLISHER.into(),
        abi_version: abi,
        sha256: String::new(),
        signature: String::new(),
        description: String::new(),
        homepage: String::new(),
        license: String::new(),
        needs: Default::default(),
        settings_schema: None,
        schema_derived: false,
        host: None,
        declares: serde_json::from_str(declares).expect("declares parses"),
    };
    LinkedPlugin::boundary(manifest, entry)
}

/// THE DROPPED-IN DOOR: `lib` signed by `signer` under `manifest` into a fresh `plugins/`
/// directory, scanned under a policy holding the release key; any other signer is allowlisted as
/// the manifest's publisher (trusted, not first-party).
fn dropped(tag: &str, manifest: Manifest, lib: &[u8], signer: &SigningKey) -> PluginRegistry {
    let dir = std::env::temp_dir().join(format!(
        "export-prometheus-conf-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let publisher = (manifest.publisher.clone(), signer.verifying_key());
    let signed = sign(signer, manifest, lib);
    let tarball = busbar_plugin_loader::tarball::package(&signed, "libsink.so", lib).unwrap();
    std::fs::write(dir.join("sink.tar.gz"), tarball).unwrap();
    let third_party = signer.verifying_key() != release().verifying_key();
    let policy = TrustPolicy {
        first_party_key: Some(release().verifying_key()),
        binary_version: VERSION.into(),
        first_party_floors: Default::default(),
        first_party_high_water: Default::default(),
        publishers: std::iter::once(publisher).filter(|_| third_party).collect(),
        allow_unsigned: false,
        allow_third_party: third_party,
        min_versions: Default::default(),
    };
    let registry =
        busbar_plugin_loader::scan_and_validate(&dir, &policy).expect("the signed sink scans");
    let _ = std::fs::remove_dir_all(&dir);
    registry
}

/// What one door does with `alias`, as one comparable transcript: the row's statement (every
/// manifest field but the two describing a tarball), whether it is first-party, the streams and
/// routes the opened sink carries, its refusals of each settings bag, the exposition it renders from
/// the recorder's snapshot, and the host's scrape answer served through it.
fn transcript(registry: &PluginRegistry, alias: &str) -> serde_json::Value {
    let p = registry.resolve(alias).expect("the alias resolves");
    let stated = Manifest {
        sha256: String::new(),
        signature: String::new(),
        ..p.manifest.clone()
    };
    let sink = registry
        .open_export(alias, SETTINGS)
        .expect("the sink opens");
    let validation: Vec<_> = [
        serde_json::json!({ "buffer_seconds": 60 }),
        serde_json::json!({ "buffer_seconds": 60, "key_gauge_limit": 5 }),
        serde_json::json!({}),
        serde_json::json!({ "buffer_seconds": 60, "buffer": 1 }),
        serde_json::json!({ "buffer_seconds": 0 }),
        serde_json::json!({ "buffer_seconds": "sixty" }),
    ]
    .iter()
    .map(|s| sink.validate("m", s).expect("the sink answers validate"))
    .collect();
    let families = busbar_plugin_loader::scrape::snapshot(OWN).expect("the exposition snapshots");
    let rendered = sink.scrape(families).expect("the sink renders");
    let served = busbar_plugin_loader::scrape::exposition(Some(&sink), Some(OWN.into()), "x/own");
    serde_json::json!({
        "row": stated,
        "first_party": p.first_party(),
        "streams": format!("{:?}", sink.streams()),
        "routes": format!("{:?}", sink.routes()),
        "validation": validation,
        "rendered": [rendered.0, rendered.1],
        "served": {
            "status": served.status,
            "headers": served.headers,
            "body": String::from_utf8(served.body).expect("utf-8 exposition"),
        },
    })
}

/// The prometheus sink registers ONE row and behaves as ONE sink through either door, and the bytes
/// it serves are the recorder's own — and the same cdylib under a third-party signature, or no sink
/// at all, does not (the RED arms).
#[test]
fn the_linked_and_the_dropped_in_prometheus_sink_are_one_sink() {
    let row = linked_row();
    let (manifest, alias) = (row.manifest.clone(), row.manifest.alias.clone());
    assert_eq!(manifest.name, "busbar-export-prometheus");
    assert_eq!(alias, "prometheus");
    let lib = cdylib();

    let linked_registry = PluginRegistry::empty().link(vec![row]).unwrap();
    let linked = transcript(&linked_registry, &alias);
    let dropped_registry = dropped("first-party", manifest.clone(), &lib, &release());
    let dropped_in = transcript(&dropped_registry, &alias);
    assert_eq!(linked, dropped_in, "the two doors are not one sink");

    // The script did what the sink is for: it carries `metrics`, claims no route of its own (the
    // host serves /metrics), refuses in the configuration's words, and renders the recorder's bytes.
    assert_eq!(linked["first_party"], true);
    assert_eq!(linked["streams"], "[Metrics]");
    assert_eq!(linked["routes"], "[]");
    assert_eq!(
        linked["validation"],
        serde_json::json!([
            [],
            [],
            ["export.m.settings: missing field `buffer_seconds`"],
            [
                "export.m.settings: unknown field `buffer`, expected `buffer_seconds` or \
                 `key_gauge_limit`"
            ],
            [
                "the `module: prometheus` export instance sets settings.buffer_seconds: 0, which \
                 retains no observations — every scrape would report empty quantiles while still \
                 paying the recording cost. Name a positive retention window in seconds, or remove \
                 the instance to turn metrics off"
            ],
            ["export.m.settings: invalid type: string \"sixty\", expected u64"],
        ])
    );
    let content_type = busbar_plugin_sdk::TEXT_EXPOSITION;
    assert_eq!(
        linked["rendered"],
        serde_json::json!([content_type, OWN]),
        "the scrape is the recorder's bytes, back"
    );
    assert_eq!(
        linked["served"],
        serde_json::json!({
            "status": 200,
            "headers": [["content-type", content_type]],
            "body": OWN,
        }),
        "the host serves the sink's rendering"
    );

    // RED ARM 1: the same bytes, dropped in under a third-party signature, are a different row.
    let mut third = manifest;
    third.publisher = "acme".into();
    let third_registry = dropped(
        "third-party",
        third,
        &lib,
        &SigningKey::from_bytes(&[32u8; 32]),
    );
    let red = transcript(&third_registry, &alias);
    assert_ne!(
        red, linked,
        "a third-party row must not be the first-party sink"
    );
    assert_eq!(red["first_party"], false);
    assert_eq!(red["row"]["publisher"], "acme");

    // RED ARM 2: with no scrape sink the host serves the recorder's text under its OWN type — the
    // transcript's served answer is sensitive to whether the sink rendered.
    let bare = busbar_plugin_loader::scrape::exposition(None, Some(OWN.into()), "x/own");
    assert_eq!(
        bare.headers,
        vec![("content-type".to_string(), "x/own".to_string())]
    );
    assert_ne!(
        serde_json::json!(bare.headers),
        linked["served"]["headers"],
        "a host with no sink must not serve the sink's content type"
    );
}
