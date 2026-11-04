// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

use std::path::PathBuf;

/// CI builds this repository's real cdylib before running its tests.
pub fn cdylib() -> PathBuf {
    let path = match std::env::var_os("BUSBAR_PLUGIN_CDYLIB") {
        Some(path) => {
            let path = PathBuf::from(path);
            assert!(path.is_absolute(), "BUSBAR_PLUGIN_CDYLIB must be absolute");
            path
        }
        None => {
            let exe = std::env::current_exe().expect("the test binary has a path");
            exe.parent()
                .and_then(|d| d.parent())
                .expect("target/<profile>")
                .join(busbar_plugin_loader::plugin_library_filename(
                    "busbar_export_prometheus_plugin",
                ))
        }
    };
    assert!(
        path.is_file(),
        "build the real busbar-export-prometheus-plugin cdylib before testing: {}",
        path.display()
    );
    path
}
