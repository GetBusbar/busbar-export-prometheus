// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

pub(crate) fn render_metrics() -> String {
    busbar_kernel::snapshot::init_for_test(std::time::Duration::from_secs(60));
    use crate::root::loader::dispatch::{
        kinds::export::Export, load_dropped, rendering_of_library, Bind, ConnTable, DispatchConfig,
        Dispatcher, NoSink,
    };
    use busbar_contract::export_calls::ExportCalls as _;
    let path = std::path::PathBuf::from(
        std::env::var_os("BUSBAR_TEST_PROMETHEUS_CDYLIB")
            .expect("BUSBAR_TEST_PROMETHEUS_CDYLIB names the actual built metrics exporter"),
    );
    let statement = rendering_of_library(&path)
        .expect("the actual exporter statement reads")
        .expect("the actual exporter carries its statement");
    #[cfg(distribution_root)]
    crate::dist_root_install::install();
    let connector = crate::root::connector::installed()
        .expect("the normal root installs the actual transport connector");
    let dispatcher = std::sync::Arc::new(Dispatcher::new(
        DispatchConfig::default(),
        busbar_kernel::host_services::caller_binding(),
    ));
    crate::root::connector::install_io(&dispatcher);
    let plugin = load_dropped::<Export>(
        &path,
        &statement,
        Bind {
            instance: "serve-test-metrics".into(),
            max_inflight_cap: 64,
            sink: std::sync::Arc::new(NoSink),
            dispatcher: dispatcher.adopter(),
            conns: ConnTable::Host(crate::root::connector::admit(
                std::sync::Arc::clone(connector)
                    as std::sync::Arc<dyn busbar_contract::conn::DeclaredConns>,
            )),
        },
    )
    .expect("the actual exporter loads through its memory ABI");
    let exporter = crate::root::loader::export_door::ExportInstance::open(
        plugin,
        dispatcher,
        br#"{"buffer_seconds":60}"#,
    )
    .expect("the actual exporter opens");
    let families =
        busbar_kernel::snapshot::snapshot().expect("the real metrics recorder is installed");
    String::from_utf8(
        exporter
            .scrape(&families)
            .expect("the actual exporter scrapes"),
    )
    .expect("the actual exporter emits UTF-8")
}

pub(crate) fn metrics_routes() -> std::sync::Arc<busbar_kernel::plugin_routes::PluginRouteTable> {
    busbar_kernel::snapshot::init_for_test(std::time::Duration::from_secs(60));
    use crate::root::loader::dispatch::{
        kinds::export::{Export, ExportFacts},
        load_dropped, rendering_of_library, Bind, ConnTable, DispatchConfig, Dispatcher, NoSink,
    };
    let path = std::path::PathBuf::from(
        std::env::var_os("BUSBAR_TEST_PROMETHEUS_CDYLIB")
            .expect("BUSBAR_TEST_PROMETHEUS_CDYLIB names the actual built metrics exporter"),
    );
    let statement = rendering_of_library(&path)
        .expect("the actual exporter statement reads")
        .expect("the actual exporter carries its statement");
    #[cfg(distribution_root)]
    crate::dist_root_install::install();
    let connector = crate::root::connector::installed()
        .expect("the normal root installs the actual transport connector");

    let dispatcher = std::sync::Arc::new(Dispatcher::with_services(
        DispatchConfig::default(),
        std::sync::Arc::new(
            busbar_kernel::host_services::KernelServices::new().with_pool(std::sync::Arc::new(
                busbar_kernel::host_services::BlockingPool::new(
                    tokio::runtime::Handle::current(),
                    64,
                ),
            )),
        ),
        busbar_kernel::host_services::caller_binding(),
    ));
    crate::root::connector::install_io(&dispatcher);
    let plugin = load_dropped::<Export>(
        &path,
        &statement,
        Bind {
            instance: "serve-http-metrics".into(),
            max_inflight_cap: 64,
            sink: std::sync::Arc::new(NoSink),
            dispatcher: dispatcher.adopter(),
            conns: ConnTable::Host(crate::root::connector::admit(
                std::sync::Arc::clone(connector)
                    as std::sync::Arc<dyn busbar_contract::conn::DeclaredConns>,
            )),
        },
    )
    .expect("the actual HTTP exporter loads through its memory ABI");
    let facts = plugin
        .context::<ExportFacts>()
        .expect("the actual exporter declares its routes");
    let routes = facts.routes.clone();
    let public_routes = facts.public_routes.clone();
    assert!(
        public_routes.iter().any(|path| path == "/metrics")
            && public_routes.iter().any(|path| path == "/metrics/hooks"),
        "the qualified actual Prometheus image declares both public scrape grants"
    );
    assert!(
        routes.iter().any(|route| route.path == "/metrics"),
        "the actual exporter declares the scrape route"
    );
    let exporter = std::sync::Arc::new(
        crate::root::loader::export_door::ExportInstance::open(
            plugin,
            dispatcher,
            br#"{"buffer_seconds":60}"#,
        )
        .expect("the actual HTTP exporter opens"),
    );
    let dispatch = busbar_kernel::export::serve_test_sink(exporter);
    let declarations = routes
        .into_iter()
        .map(|route| busbar_kernel::plugin_routes::RouteDecl {
            owner: "serve-http-metrics".into(),
            source_instance: "serve-http-metrics".into(),
            dispatch_available: true,
            kind: busbar_kernel::plugin_routes::RouteKind::Export,

            scrape: public_routes.contains(&route.path),
            persistent: false,
            route,
            dispatch: dispatch.clone(),
        })
        .collect();
    std::sync::Arc::new(
        busbar_kernel::plugin_routes::build_route_table(declarations)
            .expect("the actual export routes obey the product namespace rules"),
    )
}

mod metrics_cross_plane;
