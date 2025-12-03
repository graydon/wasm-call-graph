// Copyright 2025 Stellar Development Foundation and contributors. Licensed
// under the Apache License, Version 2.0. See the COPYING file at the root
// of this distribution or at http://www.apache.org/licenses/LICENSE-2.0

use super::*;
use crate::parsing::{apply_implicit_calls, parse_wasm_module};
use std::collections::HashMap;

fn parse_wat(wat_source: &str) -> Vec<u8> {
    wat::parse_str(wat_source).expect("Failed to parse WAT")
}

#[test]
fn test_simple_chain() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $c))
            (func $c)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], false);

    // Should have chains: a, a->b, a->b->c, b, b->c, c
    assert!(chains.contains(&"a".to_string()));
    assert!(chains.contains(&"a,b".to_string()));
    assert!(chains.contains(&"a,b,c".to_string()));
    assert!(chains.contains(&"b".to_string()));
    assert!(chains.contains(&"b,c".to_string()));
    assert!(chains.contains(&"c".to_string()));
}

#[test]
fn test_direct_recursion() {
    let wasm = parse_wat(
        r#"
        (module
            (func $recursive (call $recursive))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], false);

    // Should only have "recursive" - recursion is inhibited
    assert_eq!(chains.len(), 1);
    assert!(chains.contains(&"recursive".to_string()));
}

#[test]
fn test_indirect_recursion_two_functions() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $a))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], false);

    // Starting from a: a, a->b (can't go back to a)
    // Starting from b: b, b->a (can't go back to b)
    assert!(chains.contains(&"a".to_string()));
    assert!(chains.contains(&"a,b".to_string()));
    assert!(chains.contains(&"b".to_string()));
    assert!(chains.contains(&"b,a".to_string()));
    assert_eq!(chains.len(), 4);
}

#[test]
fn test_indirect_recursion_three_functions() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $c))
            (func $c (call $a))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], false);

    // Starting from a: a, a->b, a->b->c (can't go back to a)
    // Starting from b: b, b->c, b->c->a (can't go back to b)
    // Starting from c: c, c->a, c->a->b (can't go back to c)
    assert!(chains.contains(&"a".to_string()));
    assert!(chains.contains(&"a,b".to_string()));
    assert!(chains.contains(&"a,b,c".to_string()));
    assert!(chains.contains(&"b".to_string()));
    assert!(chains.contains(&"b,c".to_string()));
    assert!(chains.contains(&"b,c,a".to_string()));
    assert!(chains.contains(&"c".to_string()));
    assert!(chains.contains(&"c,a".to_string()));
    assert!(chains.contains(&"c,a,b".to_string()));
    assert_eq!(chains.len(), 9);
}

#[test]
fn test_indirect_recursion_four_functions() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $c))
            (func $c (call $d))
            (func $d (call $a))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], false);

    // Starting from a: a, a->b, a->b->c, a->b->c->d (can't go back to a)
    assert!(chains.contains(&"a".to_string()));
    assert!(chains.contains(&"a,b".to_string()));
    assert!(chains.contains(&"a,b,c".to_string()));
    assert!(chains.contains(&"a,b,c,d".to_string()));
    
    // Starting from b: b, b->c, b->c->d, b->c->d->a
    assert!(chains.contains(&"b".to_string()));
    assert!(chains.contains(&"b,c".to_string()));
    assert!(chains.contains(&"b,c,d".to_string()));
    assert!(chains.contains(&"b,c,d,a".to_string()));
}

#[test]
fn test_src_filter() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $c))
            (func $c)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &["b".to_string()], &[], false);

    // Should only have chains starting from b: b, b->c
    assert!(chains.contains(&"b".to_string()));
    assert!(chains.contains(&"b,c".to_string()));
    assert_eq!(chains.len(), 2);
}

#[test]
fn test_dst_filter() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $c))
            (func $c)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &["c".to_string()], false);

    // Should only have chains ending at c
    assert!(chains.contains(&"a,b,c".to_string()));
    assert!(chains.contains(&"b,c".to_string()));
    assert!(chains.contains(&"c".to_string()));
    assert_eq!(chains.len(), 3);
}

#[test]
fn test_src_and_dst_filter() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $c))
            (func $c)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &["a".to_string()], &["c".to_string()], false);

    // Should only have a->b->c
    assert!(chains.contains(&"a,b,c".to_string()));
    assert_eq!(chains.len(), 1);
}

#[test]
fn test_diamond_pattern() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b) (call $c))
            (func $b (call $d))
            (func $c (call $d))
            (func $d)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &["a".to_string()], &["d".to_string()], false);

    // Should have a->b->d and a->c->d
    assert!(chains.contains(&"a,b,d".to_string()));
    assert!(chains.contains(&"a,c,d".to_string()));
    assert_eq!(chains.len(), 2);
}

#[test]
fn test_no_matching_src() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &["nonexistent".to_string()], &[], false);

    assert!(chains.is_empty());
}

#[test]
fn test_no_matching_dst() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &["nonexistent".to_string()], false);

    assert!(chains.is_empty());
}

#[test]
fn test_complex_recursion_with_branch() {
    // a -> b -> c -> d -> b (cycle), also c -> e
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $c))
            (func $c (call $d) (call $e))
            (func $d (call $b))
            (func $e)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &["a".to_string()], &[], false);

    // From a: a, a->b, a->b->c, a->b->c->d (can't go to b), a->b->c->e
    assert!(chains.contains(&"a".to_string()));
    assert!(chains.contains(&"a,b".to_string()));
    assert!(chains.contains(&"a,b,c".to_string()));
    assert!(chains.contains(&"a,b,c,d".to_string()));
    assert!(chains.contains(&"a,b,c,e".to_string()));
}

#[test]
fn test_multiple_src_filters() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $c))
            (func $b (call $c))
            (func $c (call $d))
            (func $d)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    // Search for chains starting from either 'a' or 'b'
    let chains = enumerate_call_chains(&data, &["a".to_string(), "b".to_string()], &[], false);

    // From a: a, a->c, a->c->d
    // From b: b, b->c, b->c->d
    assert!(chains.contains(&"a".to_string()));
    assert!(chains.contains(&"a,c".to_string()));
    assert!(chains.contains(&"a,c,d".to_string()));
    assert!(chains.contains(&"b".to_string()));
    assert!(chains.contains(&"b,c".to_string()));
    assert!(chains.contains(&"b,c,d".to_string()));
    assert_eq!(chains.len(), 6);
}

#[test]
fn test_multiple_dst_filters() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b) (call $c))
            (func $b)
            (func $c)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    // Search for chains ending at either 'b' or 'c'
    let chains = enumerate_call_chains(&data, &[], &["b".to_string(), "c".to_string()], false);

    // Chains ending at b or c
    assert!(chains.contains(&"a,b".to_string()));
    assert!(chains.contains(&"a,c".to_string()));
    assert!(chains.contains(&"b".to_string()));
    assert!(chains.contains(&"c".to_string()));
    assert_eq!(chains.len(), 4);
}

#[test]
fn test_multiple_src_and_dst_filters() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $c))
            (func $b (call $c))
            (func $c (call $d) (call $e))
            (func $d)
            (func $e)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    // Search for chains starting from 'a' or 'b' and ending at 'd' or 'e'
    let chains = enumerate_call_chains(
        &data,
        &["a".to_string(), "b".to_string()],
        &["d".to_string(), "e".to_string()],
        false,
    );

    // From a ending at d or e: a->c->d, a->c->e
    // From b ending at d or e: b->c->d, b->c->e
    assert!(chains.contains(&"a,c,d".to_string()));
    assert!(chains.contains(&"a,c,e".to_string()));
    assert!(chains.contains(&"b,c,d".to_string()));
    assert!(chains.contains(&"b,c,e".to_string()));
    assert_eq!(chains.len(), 4);
}

#[test]
fn test_env_symbol_translation_chains() {
    // Build a mock env symbol map
    let mut env_map = HashMap::new();
    env_map.insert("x._".to_string(), "log_from_linear_memory".to_string());
    env_map.insert("i.0".to_string(), "obj_to_u64".to_string());

    let wasm = parse_wat(
        r#"
        (module
            (import "x" "_" (func $log))
            (import "i" "0" (func $to_u64))
            (func $main (call $log) (call $to_u64))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, Some(&env_map)).unwrap();

    // Imports should appear as destinations in call chains
    let chains = enumerate_call_chains(&data, &["main".to_string()], &[], false);
    assert!(chains.contains(&"main".to_string()));
    assert!(chains.contains(&"main,log_from_linear_memory".to_string()));
    assert!(chains.contains(&"main,obj_to_u64".to_string()));
}

#[test]
fn test_imports_not_standalone_chains() {
    // Test that imports appear as destinations but not as standalone entries
    let wasm = parse_wat(
        r#"
        (module
            (import "env" "external_func" (func $ext))
            (func $a (call $ext) (call $b))
            (func $b (call $ext))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], false);

    // Should have chains for a and b as starting points
    // Imports should appear as destinations when called (name is "ext" from WAT $ext)
    assert!(chains.contains(&"a".to_string()));
    assert!(chains.contains(&"a,ext".to_string())); // import as destination
    assert!(chains.contains(&"a,b".to_string()));
    assert!(chains.contains(&"a,b,ext".to_string())); // import as destination
    assert!(chains.contains(&"b".to_string()));
    assert!(chains.contains(&"b,ext".to_string())); // import as destination

    // Import should NOT appear as a standalone entry
    assert!(!chains.contains(&"ext".to_string()));

    assert_eq!(chains.len(), 6);
}

#[test]
fn test_imports_not_as_starting_point_chains() {
    // Verify that imports cannot be used as src filter targets
    let wasm = parse_wat(
        r#"
        (module
            (import "env" "external_func" (func $ext))
            (func $a (call $ext))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();

    // Try to filter by import name - should return empty since imports aren't starting points
    // (name is "ext" from WAT $ext due to name section)
    let chains = enumerate_call_chains(&data, &["ext".to_string()], &[], false);
    assert!(chains.is_empty());

    // But imports can be used as dst filter targets
    let chains = enumerate_call_chains(&data, &[], &["ext".to_string()], false);
    assert!(chains.contains(&"a,ext".to_string()));
    assert_eq!(chains.len(), 1);
}

#[test]
fn test_leaves_only() {
    // leaves_only: start from exports, end at imports
    let wasm = parse_wat(
        r#"
        (module
            (import "env" "log" (func $log))
            (import "env" "print" (func $print))
            (func $a (export "a") (call $b) (call $log))
            (func $b (call $c) (call $print))
            (func $c (call $log))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], true);

    // With leaves_only, should only show exported start -> imported leaf pairs
    // From a (exported): a->log, a->b->print, a->b->c->log
    assert!(chains.contains(&"a,log".to_string()));
    assert!(chains.contains(&"a,print".to_string()));
    // Should not contain intermediate paths like a,b or a,b,c
    assert!(!chains.iter().any(|c| c == "a,b" || c == "a,b,c"));
}

#[test]
fn test_leaves_only_no_imports() {
    // When there are no imports, leaves_only returns nothing (no valid leaves)
    let wasm = parse_wat(
        r#"
        (module
            (func $a (export "a") (call $b))
            (func $b (call $c))
            (func $c)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], true);

    // No imports means no valid leaves, so no results
    assert!(chains.is_empty());
}

#[test]
fn test_leaves_only_multiple_exports() {
    // Multiple exported functions, each reaching imports
    let wasm = parse_wat(
        r#"
        (module
            (import "env" "log" (func $log))
            (func $a (export "a") (call $log))
            (func $b (export "b") (call $log))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let chains = enumerate_call_chains(&data, &[], &[], true);

    // Both exports should have paths to the import
    assert!(chains.contains(&"a,log".to_string()));
    assert!(chains.contains(&"b,log".to_string()));
    assert_eq!(chains.len(), 2);
}

#[test]
fn test_implicit_call_chains() {
    // main calls an import, the import implicitly calls back to callback export
    let wasm = parse_wat(
        r#"
        (module
            (import "env" "host_func" (func $host_func))
            (func $main (export "main") (call $host_func))
            (func $callback (export "callback") (call $helper))
            (func $helper)
        )
        "#,
    );

    let mut data = parse_wasm_module(&wasm, None).unwrap();
    
    // Without implicit call, main only reaches host_func
    let chains = enumerate_call_chains(&data, &["main".to_string()], &[], false);
    assert!(chains.contains(&"main".to_string()));
    assert!(chains.contains(&"main,host_func".to_string()));
    assert!(!chains.iter().any(|c| c.contains("callback")));

    // Add implicit call from host_func to callback
    let mut implicit_calls = HashMap::new();
    implicit_calls.insert("host_func".to_string(), "callback".to_string());
    apply_implicit_calls(&mut data, &implicit_calls);

    // Now main should reach callback through host_func
    let chains = enumerate_call_chains(&data, &["main".to_string()], &[], false);
    assert!(chains.contains(&"main,host_func,callback".to_string()));
    assert!(chains.contains(&"main,host_func,callback,helper".to_string()));
}
