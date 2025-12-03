// Copyright 2025 Stellar Development Foundation and contributors. Licensed
// under the Apache License, Version 2.0. See the COPYING file at the root
// of this distribution or at http://www.apache.org/licenses/LICENSE-2.0

use super::*;
use std::collections::HashMap;

fn parse_wat(wat_source: &str) -> Vec<u8> {
    wat::parse_str(wat_source).expect("Failed to parse WAT")
}

#[test]
fn test_env_symbol_translation() {
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

    // Check that imports were translated
    assert_eq!(
        data.function_names.get(&0),
        Some(&"log_from_linear_memory".to_string())
    );
    assert_eq!(
        data.function_names.get(&1),
        Some(&"obj_to_u64".to_string())
    );

    // Check that imports are tracked
    assert!(data.imported_functions.contains(&0));
    assert!(data.imported_functions.contains(&1));
    assert!(!data.imported_functions.contains(&2)); // main is not an import
}

#[test]
fn test_imports_not_standalone() {
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

    // Verify import tracking
    assert!(data.imported_functions.contains(&0)); // $ext is index 0
    assert!(!data.imported_functions.contains(&1)); // $a is index 1
    assert!(!data.imported_functions.contains(&2)); // $b is index 2

    // Imports should not be in all_function_indices (not starting points)
    assert!(!data.all_function_indices.contains(&0));
    assert!(data.all_function_indices.contains(&1));
    assert!(data.all_function_indices.contains(&2));
}

#[test]
fn test_imports_not_as_starting_point() {
    // Verify that imports are not in all_function_indices
    let wasm = parse_wat(
        r#"
        (module
            (import "env" "external_func" (func $ext))
            (func $a (call $ext))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();

    // Import should not be a starting point
    assert!(!data.all_function_indices.contains(&0));
    // But the non-import function should be
    assert!(data.all_function_indices.contains(&1));
}

#[test]
fn test_implicit_call_parsing() {
    let args = vec!["import1:export1".to_string(), "import2:export2".to_string()];
    let map = parse_implicit_calls(&args).unwrap();
    
    assert_eq!(map.get("import1"), Some(&"export1".to_string()));
    assert_eq!(map.get("import2"), Some(&"export2".to_string()));
}

#[test]
fn test_implicit_call_parsing_error() {
    let args = vec!["invalid_format".to_string()];
    let result = parse_implicit_calls(&args);
    assert!(result.is_err());
}

#[test]
fn test_implicit_call_basic() {
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
    
    // Add implicit call from host_func to callback
    let mut implicit_calls = HashMap::new();
    implicit_calls.insert("host_func".to_string(), "callback".to_string());
    apply_implicit_calls(&mut data, &implicit_calls);

    // Check that the edge was added
    let host_func_idx = data.function_names.iter()
        .find(|(_, name)| *name == "host_func")
        .map(|(&idx, _)| idx)
        .unwrap();
    let callback_idx = data.function_names.iter()
        .find(|(_, name)| *name == "callback")
        .map(|(&idx, _)| idx)
        .unwrap();

    assert!(data.call_graph.get(&host_func_idx).unwrap().contains(&callback_idx));
}

#[test]
fn test_implicit_call_multiple() {
    let wasm = parse_wat(
        r#"
        (module
            (import "env" "host1" (func $host1))
            (import "env" "host2" (func $host2))
            (func $main (export "main") (call $host1) (call $host2))
            (func $cb1 (export "cb1"))
            (func $cb2 (export "cb2"))
        )
        "#,
    );

    let mut data = parse_wasm_module(&wasm, None).unwrap();
    
    // Add multiple implicit calls
    let mut implicit_calls = HashMap::new();
    implicit_calls.insert("host1".to_string(), "cb1".to_string());
    implicit_calls.insert("host2".to_string(), "cb2".to_string());
    apply_implicit_calls(&mut data, &implicit_calls);

    // Check that both edges were added
    let host1_idx = data.function_names.iter()
        .find(|(_, name)| *name == "host1")
        .map(|(&idx, _)| idx)
        .unwrap();
    let cb1_idx = data.function_names.iter()
        .find(|(_, name)| *name == "cb1")
        .map(|(&idx, _)| idx)
        .unwrap();

    assert!(data.call_graph.get(&host1_idx).unwrap().contains(&cb1_idx));
}
