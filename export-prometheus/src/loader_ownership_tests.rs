// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

use super::*;
#[test]
fn loader_owned_limit_check_keeps_the_first_invalid_instance() {
    assert!(validate(br#"{"bogus":1}"#).is_err());
    assert!(validate(br#"{"buffer_seconds":60}"#).is_ok());
    let findings = check_limits(&["metrics".into(), "second".into()]);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].starts_with("export.second: a second `module: prometheus`"));
}
