// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

use busbar_contract::export_calls::{ExportCalls, RecorderRequirements};
use busbar_plugin_loader::dispatch::kinds::export::Export;
use busbar_plugin_loader::dispatch::{
    load_dropped, load_linked, Bind, ConnTable, DispatchConfig, Dispatcher, LinkedRow, NoSink,
};
use busbar_plugin_loader::export_door::ExportInstance;
use std::sync::Arc;
#[path = "support/cdylib.rs"]
mod artifact;
#[path = "support/inbound.rs"]
mod inbound;

#[test]
fn both_doors_declare_their_own_recorder_settings() {
    for dropped in [false, true] {
        for (settings, expected) in [
            (
                br#"{"buffer_seconds":60}"#.as_slice(),
                RecorderRequirements {
                    retention_seconds: 60,
                    key_gauge_limit: 2000,
                },
            ),
            (
                br#"{"buffer_seconds":7,"key_gauge_limit":0}"#.as_slice(),
                RecorderRequirements {
                    retention_seconds: 7,
                    key_gauge_limit: 0,
                },
            ),
        ] {
            let d = Arc::new(Dispatcher::new(
                DispatchConfig::default(),
                busbar_contract::services::test_caller_binding(),
            ));
            let row = LinkedRow::of(busbar_export_prometheus::door).unwrap();
            let bind = Bind {
                instance: Arc::from("export.metrics"),
                max_inflight_cap: 64,
                sink: Arc::new(NoSink),
                dispatcher: d.adopter(),
                conns: ConnTable::Host(inbound::host(d.conn_waker(), None)),
            };
            let plugin = if dropped {
                load_dropped::<Export>(&artifact::cdylib(), &row.statement, bind)
            } else {
                load_linked::<Export>(&row, bind)
            }
            .unwrap();
            let instance = ExportInstance::open(plugin, d, settings).unwrap();
            assert_eq!(instance.recorder_requirements(), Some(expected));
        }
    }
}

#[test]
fn configuration_probe_gets_the_declaration_without_a_live_listener() {
    use busbar_plugin_loader::export_axis::ExportRows;
    use busbar_plugin_loader::{LinkedPlugin, PluginRegistry};
    let manifest = busbar_plugin_loader::sign::Manifest {
        name: busbar_export_prometheus::NAME.into(),
        alias: busbar_export_prometheus::ALIAS.into(),
        kind: "export".into(),
        version: "1.6.0".into(),
        publisher: "busbar".into(),
        abi_version: busbar_contract::abi::export::ABI_VERSION,
        sha256: String::new(),
        signature: String::new(),
        description: String::new(),
        homepage: String::new(),
        license: String::new(),
        needs: Default::default(),
        settings_schema: None,
        schema_derived: false,
        host: None,
        declares: Default::default(),
        statement: None,
        former_names: Vec::new(),
    };
    let registry = PluginRegistry::empty()
        .link(vec![LinkedPlugin::door(
            manifest,
            busbar_export_prometheus::door,
        )])
        .unwrap();
    let d = Arc::new(Dispatcher::new(
        DispatchConfig::default(),
        busbar_contract::services::test_caller_binding(),
    ));
    assert!(d.lend_seal(busbar_contract::plugin::TestKernelSeal));
    let rows = ExportRows::new(&registry, d);
    let (_, problems, recorder) = rows
        .probe(
            "prometheus",
            "metrics",
            &serde_json::json!({"buffer_seconds":60}).into(),
        )
        .unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        recorder,
        Some(RecorderRequirements {
            retention_seconds: 60,
            key_gauge_limit: 2000
        })
    );
    let (_, refused, _) = rows
        .probe(
            busbar_export_prometheus::ALIAS,
            "metrics",
            &serde_json::json!({"buffer_seconds":60,"bogus":1}).into(),
        )
        .expect("declared export row");
    assert_eq!(refused, [
        "export.metrics.settings: unknown field `bogus`, expected `buffer_seconds` or `key_gauge_limit`"
    ]);
    let (_, refused, _) = rows
        .probe(
            busbar_export_prometheus::ALIAS,
            "metrics",
            &serde_json::json!({"buffer_seconds":0}).into(),
        )
        .expect("declared export row");
    assert_eq!(refused.len(), 1);
    assert!(
        refused[0].starts_with("the `module: prometheus` export instance"),
        "{refused:?}"
    );
}
