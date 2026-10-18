// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Busbar Inc and contributors

//! THE SINK'S SOURCE STATES NO CONTRACT-ABI RANGE: the kind's one ABI version is the only version
//! fact admission reads (`BUSBAR-1.6.0.md` §11.2, l.1309-1316), and the loader parses a manifest's
//! `declares.contract_abi` only to never consult it. So no source file documents or asserts that
//! superseded range, and `declares.json` can drop it without a source edit; nor does any date a
//! shape to an ABI "minor", a numbering the per-kind ABIs do not have. RED before the fix:
//! `DECLARES` was documented as "the contract-ABI range it speaks" and a test asserted the range.

#[test]
fn no_source_file_names_the_contract_abi_range() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("src reads") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a source file");
            for (n, line) in text.lines().enumerate() {
                // Nor a minor number: a kind's ABI is one version, never a major.minor pair.
                assert!(
                    !line.contains("contract_abi")
                        && !line.contains("contract-ABI")
                        && !line.contains("ABI minor"),
                    "{}:{}: {line}",
                    path.display(),
                    n + 1
                );
            }
        }
    }
}

/// THE ORDERING RATIONALE SITS ON THE ORDERING: `canonical_order`, the function that orders a
/// snapshot, carries the doc that explains the order, and the small kind-ranking helper carries its
/// own. RED before the fix: the rationale sat on `kind_rank` and `canonical_order` had none.
#[test]
fn the_ordering_function_carries_its_doc() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let text = std::fs::read_to_string(&path).expect("the crate root");
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("fn canonical_order("))
        .expect("canonical_order is defined");
    assert!(
        at > 0 && lines[at - 1].trim_start().starts_with("///"),
        "canonical_order has no doc of its own"
    );
    let doc: String = lines[..at]
        .iter()
        .rev()
        .take_while(|l| l.trim_start().starts_with("///"))
        .copied()
        .collect();
    assert!(
        doc.contains("THE KIND GROUPING"),
        "the ordering rationale is not on canonical_order"
    );
}
