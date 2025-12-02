// Copyright 2025 Stellar Development Foundation and contributors. Licensed
// under the Apache License, Version 2.0. See the COPYING file at the root
// of this distribution or at http://www.apache.org/licenses/LICENSE-2.0

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use clap::Parser;
use serde::Deserialize;
use wasmparser::{ExternalKind, Name, Operator, Payload, TypeRef};

/// A tool to analyze WebAssembly module call graphs and enumerate call chains.
#[derive(Parser, Debug)]
#[command(name = "wasm-call-graph")]
#[command(about = "Analyzes WebAssembly modules and outputs all possible call chains")]
#[command(long_about = "Parses one or more WebAssembly bytecode modules, builds a static call graph,\n\
    and outputs all possible call chains (with recursion inhibition).\n\
    Each line of output shows a comma-separated list of function names in the call chain.")]
struct Args {
    /// WebAssembly file(s) to analyze
    #[arg(required = true)]
    files: Vec<String>,

    /// Only show chains that start from functions with this name (can be specified multiple times)
    #[arg(long, short = 's')]
    src: Vec<String>,

    /// Only show chains that end at functions with this name (can be specified multiple times)
    #[arg(long, short = 'd')]
    dst: Vec<String>,

    /// Path to env.json file for translating short import names to long names
    #[arg(long)]
    env_symbols: Option<String>,

    /// Print filename prefix on each output line (default: false for 1 file, true for >1 files)
    #[arg(long, value_parser = parse_bool_arg)]
    filename: Option<bool>,
}

fn parse_bool_arg(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(format!("Invalid boolean value: {}", s)),
    }
}

/// Represents a function entry in the env.json module
#[derive(Debug, Deserialize)]
struct EnvFunction {
    export: String,
    name: String,
}

/// Represents a module entry in the env.json file
#[derive(Debug, Deserialize)]
struct EnvModule {
    export: String,
    functions: Vec<EnvFunction>,
}

/// Root structure of env.json
#[derive(Debug, Deserialize)]
struct EnvConfig {
    modules: Vec<EnvModule>,
}

/// Build a lookup map from "module_export.func_export" -> "long_name"
fn build_env_symbol_map(env_path: &str) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(env_path)?;
    let config: EnvConfig = serde_json::from_str(&content)?;
    
    let mut map = HashMap::new();
    for module in config.modules {
        for func in module.functions {
            let key = format!("{}.{}", module.export, func.export);
            map.insert(key, func.name);
        }
    }
    Ok(map)
}

/// Parsed call graph data for a single wasm module
#[derive(Debug)]
pub struct CallGraphData {
    pub function_names: HashMap<u32, String>,
    pub call_graph: HashMap<u32, Vec<u32>>,
    pub all_function_indices: Vec<u32>,
}

/// Parse a wasm module and extract call graph data
pub fn parse_wasm_module(
    wasm_bytes: &[u8],
    env_symbol_map: Option<&HashMap<String, String>>,
) -> Result<CallGraphData, Box<dyn std::error::Error>> {
    let mut num_imported_functions: u32 = 0;
    let mut function_names: HashMap<u32, String> = HashMap::new();
    let mut env_translated: HashSet<u32> = HashSet::new(); // Track which names came from env translation
    let mut call_graph: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut current_func_index: u32 = 0;
    let mut all_function_indices: Vec<u32> = Vec::new();

    for payload in wasmparser::Parser::new(0).parse_all(wasm_bytes) {
        let payload = payload?;
        match payload {
            Payload::ImportSection(reader) => {
                for import in reader {
                    let import = import?;
                    if let TypeRef::Func(_) = import.ty {
                        // Try to translate using env_symbol_map if available
                        let name = if let Some(map) = env_symbol_map {
                            let key = format!("{}.{}", import.module, import.name);
                            if let Some(translated) = map.get(&key) {
                                env_translated.insert(num_imported_functions);
                                translated.clone()
                            } else {
                                format!("{}:{}", import.module, import.name)
                            }
                        } else {
                            format!("{}:{}", import.module, import.name)
                        };
                        function_names.insert(num_imported_functions, name);
                        all_function_indices.push(num_imported_functions);
                        num_imported_functions += 1;
                    }
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export?;
                    if let ExternalKind::Func = export.kind {
                        // Don't override env-translated names
                        if !env_translated.contains(&export.index) {
                            function_names.insert(export.index, export.name.to_string());
                        }
                    }
                }
            }
            Payload::CustomSection(reader) => {
                if reader.name() == "name" {
                    if let wasmparser::KnownCustom::Name(name_reader) = reader.as_known() {
                        for name in name_reader {
                            if let Ok(Name::Function(func_names)) = name {
                                for naming in func_names {
                                    if let Ok(naming) = naming {
                                        // Don't override env-translated names
                                        if !env_translated.contains(&naming.index) {
                                            function_names.insert(naming.index, naming.name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let func_index = num_imported_functions + current_func_index;
                all_function_indices.push(func_index);
                let mut callees: Vec<u32> = Vec::new();

                let mut reader = body.get_operators_reader()?;
                while !reader.eof() {
                    let op = reader.read()?;
                    match op {
                        Operator::Call { function_index } => {
                            if !callees.contains(&function_index) {
                                callees.push(function_index);
                            }
                        }
                        Operator::ReturnCall { function_index } => {
                            if !callees.contains(&function_index) {
                                callees.push(function_index);
                            }
                        }
                        _ => {}
                    }
                }

                call_graph.insert(func_index, callees);
                current_func_index += 1;
            }
            _ => {}
        }
    }

    // Generate default names for any functions without names
    for &idx in &all_function_indices {
        if !function_names.contains_key(&idx) {
            function_names.insert(idx, format!("func_{}", idx));
        }
    }

    Ok(CallGraphData {
        function_names,
        call_graph,
        all_function_indices,
    })
}

/// DFS to enumerate all call chains with recursion inhibition.
/// Returns a vector of call chain strings.
pub fn enumerate_call_chains(
    data: &CallGraphData,
    src_filter: &[String],
    dst_filter: &[String],
) -> Vec<String> {
    let mut results = Vec::new();

    fn dfs(
        func_idx: u32,
        call_graph: &HashMap<u32, Vec<u32>>,
        function_names: &HashMap<u32, String>,
        current_path: &mut Vec<u32>,
        visited: &mut HashSet<u32>,
        results: &mut Vec<String>,
        dst_filter: &[String],
    ) {
        current_path.push(func_idx);
        visited.insert(func_idx);

        // Build the path string
        let path_names: Vec<&str> = current_path
            .iter()
            .map(|idx| {
                function_names
                    .get(idx)
                    .map(|s| s.as_str())
                    .unwrap_or("unknown")
            })
            .collect();

        // Check if we should include this path based on dst_filter
        let should_include = if dst_filter.is_empty() {
            true
        } else {
            path_names.last().map_or(false, |last| dst_filter.iter().any(|d| d == *last))
        };

        if should_include {
            results.push(path_names.join(","));
        }

        // Continue DFS to non-visited callees
        if let Some(callees) = call_graph.get(&func_idx) {
            for &callee in callees {
                if !visited.contains(&callee) {
                    dfs(
                        callee,
                        call_graph,
                        function_names,
                        current_path,
                        visited,
                        results,
                        dst_filter,
                    );
                }
            }
        }

        current_path.pop();
        visited.remove(&func_idx);
    }

    // Determine which functions to start from
    let start_functions: Vec<u32> = if src_filter.is_empty() {
        data.all_function_indices.clone()
    } else {
        data.all_function_indices
            .iter()
            .filter(|&&idx| {
                data.function_names
                    .get(&idx)
                    .map(|name| src_filter.iter().any(|s| s == name))
                    .unwrap_or(false)
            })
            .copied()
            .collect()
    };

    for func_idx in start_functions {
        let mut current_path: Vec<u32> = Vec::new();
        let mut visited: HashSet<u32> = HashSet::new();
        dfs(
            func_idx,
            &data.call_graph,
            &data.function_names,
            &mut current_path,
            &mut visited,
            &mut results,
            dst_filter,
        );
    }

    results
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Load env symbols if provided
    let env_symbol_map = if let Some(ref env_path) = args.env_symbols {
        Some(build_env_symbol_map(env_path)?)
    } else {
        None
    };

    // Determine whether to show filename prefix
    let show_filename = args.filename.unwrap_or(args.files.len() > 1);

    let mut total_paths = 0;
    let has_filter = !args.src.is_empty() || !args.dst.is_empty();

    for file_path in &args.files {
        let wasm_bytes = fs::read(file_path)?;
        let filename = Path::new(file_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(file_path);

        let data = parse_wasm_module(&wasm_bytes, env_symbol_map.as_ref())?;
        let chains = enumerate_call_chains(&data, &args.src, &args.dst);

        for chain in &chains {
            if show_filename {
                println!("{}:{}", filename, chain);
            } else {
                println!("{}", chain);
            }
        }

        total_paths += chains.len();
    }

    // Exit with code 1 if filters were applied and no paths matched
    if has_filter && total_paths == 0 {
        std::process::exit(1);
    }

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

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
        let chains = enumerate_call_chains(&data, &[], &[]);

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
        let chains = enumerate_call_chains(&data, &[], &[]);

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
        let chains = enumerate_call_chains(&data, &[], &[]);

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
        let chains = enumerate_call_chains(&data, &[], &[]);

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
        let chains = enumerate_call_chains(&data, &[], &[]);

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
        let chains = enumerate_call_chains(&data, &["b".to_string()], &[]);

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
        let chains = enumerate_call_chains(&data, &[], &["c".to_string()]);

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
        let chains = enumerate_call_chains(&data, &["a".to_string()], &["c".to_string()]);

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
        let chains = enumerate_call_chains(&data, &["a".to_string()], &["d".to_string()]);

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
        let chains = enumerate_call_chains(&data, &["nonexistent".to_string()], &[]);

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
        let chains = enumerate_call_chains(&data, &[], &["nonexistent".to_string()]);

        assert!(chains.is_empty());
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

        let chains = enumerate_call_chains(&data, &["main".to_string()], &[]);
        assert!(chains.contains(&"main".to_string()));
        assert!(chains.contains(&"main,log_from_linear_memory".to_string()));
        assert!(chains.contains(&"main,obj_to_u64".to_string()));
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
        let chains = enumerate_call_chains(&data, &["a".to_string()], &[]);

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
        let chains = enumerate_call_chains(&data, &["a".to_string(), "b".to_string()], &[]);

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
        let chains = enumerate_call_chains(&data, &[], &["b".to_string(), "c".to_string()]);

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
        );

        // From a ending at d or e: a->c->d, a->c->e
        // From b ending at d or e: b->c->d, b->c->e
        assert!(chains.contains(&"a,c,d".to_string()));
        assert!(chains.contains(&"a,c,e".to_string()));
        assert!(chains.contains(&"b,c,d".to_string()));
        assert!(chains.contains(&"b,c,e".to_string()));
        assert_eq!(chains.len(), 4);
    }
}
