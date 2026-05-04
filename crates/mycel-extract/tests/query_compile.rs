//! Sanity check: every Query::new must succeed on the installed grammars.
//! If any of these tests fails, the offending query string targets a
//! field/node name that doesn't exist in the current tree-sitter grammar
//! version. Fix the query, do not skip the test.

use tree_sitter::{Language, Query};

fn try_compile(name: &str, lang: &Language, src: &str) {
    Query::new(lang, src).unwrap_or_else(|e| panic!("query `{name}` failed to compile: {e}"));
}

#[test]
fn typescript_queries_compile() {
    let ts: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let tsx: Language = tree_sitter_typescript::LANGUAGE_TSX.into();
    let symbols = include_str!("../src/languages/typescript_queries/symbols.scm");
    let calls = include_str!("../src/languages/typescript_queries/calls.scm");
    let imports = include_str!("../src/languages/typescript_queries/imports.scm");
    for (label, src) in &[("symbols", symbols), ("calls", calls), ("imports", imports)] {
        try_compile(&format!("ts/{label}"), &ts, src);
        try_compile(&format!("tsx/{label}"), &tsx, src);
    }
}

#[test]
fn rust_queries_compile() {
    let lang: Language = tree_sitter_rust::LANGUAGE.into();
    let symbols = include_str!("../src/languages/rust_queries/symbols.scm");
    let impls = include_str!("../src/languages/rust_queries/impls.scm");
    let uses = include_str!("../src/languages/rust_queries/uses.scm");
    let calls = include_str!("../src/languages/rust_queries/calls.scm");
    for (label, src) in &[
        ("symbols", symbols),
        ("impls", impls),
        ("uses", uses),
        ("calls", calls),
    ] {
        try_compile(&format!("rs/{label}"), &lang, src);
    }
}
