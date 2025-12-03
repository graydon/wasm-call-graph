// Copyright 2025 Stellar Development Foundation and contributors. Licensed
// under the Apache License, Version 2.0. See the COPYING file at the root
// of this distribution or at http://www.apache.org/licenses/LICENSE-2.0

use super::*;
use crate::parsing::{apply_implicit_calls, parse_wasm_module};
use std::collections::HashMap;

fn parse_wat(wat_source: &str) -> Vec<u8> {
    wat::parse_str(wat_source).expect("Failed to parse WAT")
}

/// Helper to create a pattern from strings. Each string can contain | for alternatives.
fn pat(elements: &[&str]) -> Vec<Vec<String>> {
    elements.iter()
        .map(|s| s.split('|').map(|alt| alt.to_string()).collect())
        .collect()
}

#[test]
fn test_paths_simple_chain() {
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
    let paths = generate_call_paths(&data, &[], None);

    // a calls b, b calls c, c calls nothing
    assert!(paths.contains(&"a{b{c}}".to_string()));
    assert!(paths.contains(&"b{c}".to_string()));
    assert!(paths.contains(&"c".to_string()));
}

#[test]
fn test_paths_multiple_calls() {
    // X calls A and then B, A calls C and D
    let wasm = parse_wat(
        r#"
        (module
            (func $X (call $A) (call $B))
            (func $A (call $C) (call $D))
            (func $B)
            (func $C)
            (func $D)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let paths = generate_call_paths(&data, &["X".to_string()], None);

    // X{A{C,D},B}
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "X{A{C,D},B}");
}

#[test]
fn test_paths_pattern_matching() {
    // X calls A and then B, A calls C and D
    let wasm = parse_wat(
        r#"
        (module
            (func $X (call $A) (call $B))
            (func $A (call $C) (call $D))
            (func $B)
            (func $C)
            (func $D)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();

    // Pattern X..C..B should match and output only X{A{C},B} (D is filtered out)
    let paths = generate_call_paths(&data, &["X".to_string()], Some(&pat(&["X", "C", "B"])));
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "X{A{C},B}");

    // Pattern X..B should match and output only X{B} (A and its children are filtered out)
    let paths = generate_call_paths(&data, &["X".to_string()], Some(&pat(&["X", "B"])));
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "X{B}");

    // Pattern X..B..D should NOT match (B appears before D in the pattern, but D appears before B in summary)
    let paths = generate_call_paths(&data, &["X".to_string()], Some(&pat(&["X", "B", "D"])));
    assert!(paths.is_empty());
}

#[test]
fn test_paths_direct_recursion() {
    // A function that calls itself (direct recursion)
    let wasm = parse_wat(
        r#"
        (module
            (func $recursive (call $recursive))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let paths = generate_call_paths(&data, &[], None);

    // Should unroll twice: recursive{recursive{recursive}}
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "recursive{recursive{recursive}}");
}

#[test]
fn test_paths_indirect_recursion() {
    // A calls B, B calls A (indirect recursion)
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $b))
            (func $b (call $a))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let paths = generate_call_paths(&data, &["a".to_string()], None);

    // From a: a{b{a{b{a}}}}
    // Wait, let's think: a calls b, b calls a, a calls b (2nd time), b calls a (2nd time), a is at limit
    // Actually with visit count tracking: a(1)->b(1)->a(2)->b(2)->a(at limit, return just "a")
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "a{b{a{b{a}}}}");
}

#[test]
fn test_paths_loop_body_calls() {
    // Function with a loop that makes multiple calls
    // We simulate this with repeated calls in the bytecode
    let wasm = parse_wat(
        r#"
        (module
            (func $loop_func (call $helper) (call $helper))
            (func $helper)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let paths = generate_call_paths(&data, &["loop_func".to_string()], None);

    // Two calls to helper should appear
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "loop_func{helper,helper}");
}

#[test]
fn test_paths_complex_with_loop() {
    // Complex case with calls and a loop
    let wasm = parse_wat(
        r#"
        (module
            (func $main (call $setup) (call $process) (call $process) (call $cleanup))
            (func $setup)
            (func $process (call $helper))
            (func $cleanup)
            (func $helper)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let paths = generate_call_paths(&data, &["main".to_string()], None);

    // main calls setup, process (with helper), process again (with helper), cleanup
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "main{setup,process{helper},process{helper},cleanup}");
}

#[test]
fn test_paths_diamond_pattern() {
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
    let paths = generate_call_paths(&data, &["a".to_string()], None);

    // a calls b (which calls d), then c (which calls d)
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "a{b{d},c{d}}");
}

#[test]
fn test_paths_with_imports() {
    let wasm = parse_wat(
        r#"
        (module
            (import "env" "log" (func $log))
            (func $main (call $log) (call $helper))
            (func $helper (call $log))
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let paths = generate_call_paths(&data, &["main".to_string()], None);

    // main calls log, then helper (which calls log)
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "main{log,helper{log}}");
}

#[test]
fn test_call_node_names_in_order() {
    // Build a tree: X{A{C,D},B}
    let mut x = CallNode::new("X".to_string());
    let mut a = CallNode::new("A".to_string());
    a.children.push(CallNode::new("C".to_string()));
    a.children.push(CallNode::new("D".to_string()));
    x.children.push(a);
    x.children.push(CallNode::new("B".to_string()));

    assert_eq!(
        x.names_in_order(),
        vec!["X", "A", "C", "D", "B"]
    );
}

#[test]
fn test_call_node_to_string() {
    // Build a tree: X{A{C,D},B}
    let mut x = CallNode::new("X".to_string());
    let mut a = CallNode::new("A".to_string());
    a.children.push(CallNode::new("C".to_string()));
    a.children.push(CallNode::new("D".to_string()));
    x.children.push(a);
    x.children.push(CallNode::new("B".to_string()));

    assert_eq!(x.to_string(), "X{A{C,D},B}");
}

#[test]
fn test_call_node_filter_by_pattern() {
    // Build a tree: X{A{C,D},B}
    let mut x = CallNode::new("X".to_string());
    let mut a = CallNode::new("A".to_string());
    a.children.push(CallNode::new("C".to_string()));
    a.children.push(CallNode::new("D".to_string()));
    x.children.push(a);
    x.children.push(CallNode::new("B".to_string()));

    // Pattern X..C..B should filter to X{A{C},B}
    let filtered = x.filter_by_pattern(&pat(&["X", "C", "B"])).unwrap();
    assert_eq!(filtered.to_string(), "X{A{C},B}");

    // Pattern X..B should filter to X{B}
    let filtered = x.filter_by_pattern(&pat(&["X", "B"])).unwrap();
    assert_eq!(filtered.to_string(), "X{B}");

    // Pattern X..A..C should filter to X{A{C}}
    let filtered = x.filter_by_pattern(&pat(&["X", "A", "C"])).unwrap();
    assert_eq!(filtered.to_string(), "X{A{C}}");
}

#[test]
fn test_matches_path_pattern_tree() {
    // Build a tree: X{A{C,D},B}
    let mut x = CallNode::new("X".to_string());
    let mut a = CallNode::new("A".to_string());
    a.children.push(CallNode::new("C".to_string()));
    a.children.push(CallNode::new("D".to_string()));
    x.children.push(a);
    x.children.push(CallNode::new("B".to_string()));

    // X..C..B: X appears, then C, then B - should match
    assert!(matches_path_pattern_tree(&x, &pat(&["X", "C", "B"])));

    // X..B: X appears, then B - should match
    assert!(matches_path_pattern_tree(&x, &pat(&["X", "B"])));

    // X..B..D: X, then B, then D - should NOT match (D comes before B)
    assert!(!matches_path_pattern_tree(&x, &pat(&["X", "B", "D"])));

    // X..A..C: should match
    assert!(matches_path_pattern_tree(&x, &pat(&["X", "A", "C"])));

    // Z..A: should NOT match (Z is not in tree)
    assert!(!matches_path_pattern_tree(&x, &pat(&["Z", "A"])));

    // Empty pattern should match everything
    assert!(matches_path_pattern_tree(&x, &[]));
}

#[test]
fn test_pattern_alternatives() {
    // X calls A and then B, A calls C and D
    let wasm = parse_wat(
        r#"
        (module
            (func $X (call $A) (call $B))
            (func $A (call $C) (call $D))
            (func $B)
            (func $C)
            (func $D)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();

    // Pattern X..C|D..B should match (C or D, then B)
    // C matches first, consuming the C|D element, then B matches
    let paths = generate_call_paths(&data, &["X".to_string()], Some(&pat(&["X", "C|D", "B"])));
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "X{A{C},B}");

    // Pattern X..C|B should match C or B
    // C matches first (via A), consuming the pattern
    let paths = generate_call_paths(&data, &["X".to_string()], Some(&pat(&["X", "C|B"])));
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "X{A{C}}");

    // Pattern Y|X..B should match (Y or X, then B)
    let paths = generate_call_paths(&data, &["X".to_string()], Some(&pat(&["Y|X", "B"])));
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "X{B}");

    // Pattern Z|W..B should NOT match (neither Z nor W is in tree)
    let paths = generate_call_paths(&data, &["X".to_string()], Some(&pat(&["Z|W", "B"])));
    assert!(paths.is_empty());
}

#[test]
fn test_pattern_alternatives_matching() {
    // Build a tree: X{A{C,D},B}
    let mut x = CallNode::new("X".to_string());
    let mut a = CallNode::new("A".to_string());
    a.children.push(CallNode::new("C".to_string()));
    a.children.push(CallNode::new("D".to_string()));
    x.children.push(a);
    x.children.push(CallNode::new("B".to_string()));

    // X|Y..C..B: X or Y, then C, then B - should match
    assert!(matches_path_pattern_tree(&x, &pat(&["X|Y", "C", "B"])));

    // Z|W..C..B: neither Z nor W is in tree - should NOT match
    assert!(!matches_path_pattern_tree(&x, &pat(&["Z|W", "C", "B"])));

    // X..C|D..B: C or D, then B - should match
    assert!(matches_path_pattern_tree(&x, &pat(&["X", "C|D", "B"])));

    // X..A..C|D: should match (C or D at the end)
    assert!(matches_path_pattern_tree(&x, &pat(&["X", "A", "C|D"])));
}

#[test]
fn test_paths_three_level_recursion() {
    // a -> b -> c -> a (cycle of 3)
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
    let paths = generate_call_paths(&data, &["a".to_string()], None);

    // a(1)->b(1)->c(1)->a(2)->b(2)->c(2)->a(at limit)
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "a{b{c{a{b{c{a}}}}}}");
}

#[test]
fn test_paths_no_calls() {
    let wasm = parse_wat(
        r#"
        (module
            (func $leaf)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    let paths = generate_call_paths(&data, &[], None);

    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "leaf");
}

#[test]
fn test_paths_src_filter() {
    let wasm = parse_wat(
        r#"
        (module
            (func $a (call $c))
            (func $b (call $c))
            (func $c)
        )
        "#,
    );

    let data = parse_wasm_module(&wasm, None).unwrap();
    
    // Only from a
    let paths = generate_call_paths(&data, &["a".to_string()], None);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "a{c}");

    // From both a and b
    let paths = generate_call_paths(&data, &["a".to_string(), "b".to_string()], None);
    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&"a{c}".to_string()));
    assert!(paths.contains(&"b{c}".to_string()));
}

#[test]
fn test_implicit_call_paths_mode() {
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

    // Check paths mode output
    let paths = generate_call_paths(&data, &["main".to_string()], None);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "main{host_func{callback{helper}}}");
}

#[test]
fn test_implicit_call_paths_mode_multiple() {
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

    // Check paths mode output
    let paths = generate_call_paths(&data, &["main".to_string()], None);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "main{host1{cb1},host2{cb2}}");
}
