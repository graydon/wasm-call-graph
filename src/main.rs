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

    /// Only print starting function and final imports (leaf nodes), omitting internal functions
    #[arg(long)]
    leaves_only: bool,

    /// Output sequential call summaries in format X{A{C,D},B} instead of call chains.
    /// Optionally provide a pattern separated by .. to filter output (e.g., --paths=X..C..B)
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "")]
    paths: Option<String>,

    /// Add an implicit edge from an import to an export (host callback).
    /// Format: IMPORT:EXPORT (can be specified multiple times)
    #[arg(long)]
    implicit_call: Vec<String>,
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
    /// Ordered calls with duplicates preserved
    pub call_graph: HashMap<u32, Vec<u32>>,
    pub all_function_indices: Vec<u32>,
    pub imported_functions: HashSet<u32>,
    pub exported_functions: HashSet<u32>,
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
    let mut imported_functions: HashSet<u32> = HashSet::new();
    let mut exported_functions: HashSet<u32> = HashSet::new();

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
                        imported_functions.insert(num_imported_functions);
                        // Note: imports are NOT added to all_function_indices
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
                        exported_functions.insert(export.index);
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
                            callees.push(function_index);
                        }
                        Operator::ReturnCall { function_index } => {
                            callees.push(function_index);
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
        imported_functions,
        exported_functions,
    })
}

/// Parse implicit call arguments and return a map from import name to export name
fn parse_implicit_calls(args: &[String]) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    for arg in args {
        if let Some((import, export)) = arg.split_once(":") {
            map.insert(import.to_string(), export.to_string());
        } else {
            return Err(format!("Invalid implicit-call format '{}', expected IMPORT:EXPORT", arg));
        }
    }
    Ok(map)
}

/// Apply implicit calls to the call graph data.
/// For each import that has an implicit callback to an export, add an edge from the import to the export.
pub fn apply_implicit_calls(data: &mut CallGraphData, implicit_calls: &HashMap<String, String>) {
    // Build reverse lookup: function name -> function index
    let name_to_idx: HashMap<&str, u32> = data.function_names
        .iter()
        .map(|(&idx, name)| (name.as_str(), idx))
        .collect();

    for (import_name, export_name) in implicit_calls {
        // Find the import function index
        let import_idx = name_to_idx.get(import_name.as_str());
        // Find the export function index
        let export_idx = name_to_idx.get(export_name.as_str());

        if let (Some(&imp_idx), Some(&exp_idx)) = (import_idx, export_idx) {
            // Add edge from import to export in call_graph
            data.call_graph
                .entry(imp_idx)
                .or_insert_with(Vec::new)
                .push(exp_idx);
        }
    }
}

/// DFS to enumerate all call chains with recursion inhibition.
/// Returns a vector of call chain strings.
pub fn enumerate_call_chains(
    data: &CallGraphData,
    src_filter: &[String],
    dst_filter: &[String],
    leaves_only: bool,
) -> Vec<String> {
    let mut results = Vec::new();

    fn dfs(
        func_idx: u32,
        call_graph: &HashMap<u32, Vec<u32>>,
        function_names: &HashMap<u32, String>,
        imported_functions: &HashSet<u32>,
        current_path: &mut Vec<u32>,
        visited: &mut HashSet<u32>,
        results: &mut Vec<String>,
        dst_filter: &[String],
        leaves_only: bool,
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

        // A leaf is an imported function (callable from runtime, has no callees in call graph)
        let is_import = imported_functions.contains(&func_idx);

        // Check if we should include this path based on dst_filter
        let passes_dst_filter = if dst_filter.is_empty() {
            true
        } else {
            path_names.last().map_or(false, |last| dst_filter.iter().any(|d| d == *last))
        };

        // When leaves_only is true, only include paths that end at an import
        let should_include = passes_dst_filter && (!leaves_only || is_import);

        if should_include {
            if leaves_only && path_names.len() > 1 {
                // Only output start and end (leaf)
                results.push(format!("{},{}", path_names[0], path_names[path_names.len() - 1]));
            } else {
                results.push(path_names.join(","));
            }
        }

        // Continue DFS to non-visited callees
        if let Some(callees) = call_graph.get(&func_idx) {
            for &callee in callees {
                if !visited.contains(&callee) {
                    dfs(
                        callee,
                        call_graph,
                        function_names,
                        imported_functions,
                        current_path,
                        visited,
                        results,
                        dst_filter,
                        leaves_only,
                    );
                }
            }
        }

        current_path.pop();
        visited.remove(&func_idx);
    }

    // Determine which functions to start from
    // When leaves_only is true, only start from exported functions
    let candidate_functions: &[u32] = if leaves_only {
        // Filter to only exported functions
        &data.all_function_indices
            .iter()
            .filter(|idx| data.exported_functions.contains(idx))
            .copied()
            .collect::<Vec<_>>()
    } else {
        &data.all_function_indices
    };

    let start_functions: Vec<u32> = if src_filter.is_empty() {
        candidate_functions.to_vec()
    } else {
        candidate_functions
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
            &data.imported_functions,
            &mut current_path,
            &mut visited,
            &mut results,
            dst_filter,
            leaves_only,
        );
    }

    results.sort();
    results
}

/// A tree node representing a function call and its children
#[derive(Debug, Clone)]
struct CallNode {
    name: String,
    children: Vec<CallNode>,
}

impl CallNode {
    fn new(name: String) -> Self {
        CallNode { name, children: Vec::new() }
    }

    /// Convert the tree to a string in format X{A{C,D},B}
    fn to_string(&self) -> String {
        if self.children.is_empty() {
            self.name.clone()
        } else {
            let child_strs: Vec<String> = self.children.iter().map(|c| c.to_string()).collect();
            format!("{}{{{}}}", self.name, child_strs.join(","))
        }
    }

    /// Extract all names in order (depth-first, pre-order)
    fn names_in_order(&self) -> Vec<String> {
        let mut names = vec![self.name.clone()];
        for child in &self.children {
            names.extend(child.names_in_order());
        }
        names
    }

    /// Filter the tree to only include nodes that match the pattern or are on the path to matching nodes.
    /// The pattern must be matched in order across the tree traversal.
    /// Each pattern element is a Vec of alternatives (e.g., ["X", "Y"] means X or Y).
    /// Returns Some(filtered_node) if this subtree contributes to matching the pattern.
    fn filter_by_pattern(&self, remaining_pattern: &[Vec<String>]) -> Option<CallNode> {
        self.filter_by_pattern_inner(remaining_pattern).0
    }

    /// Inner helper that returns (filtered_node, remaining_pattern_after_subtree)
    fn filter_by_pattern_inner<'a>(&self, remaining_pattern: &'a [Vec<String>]) -> (Option<CallNode>, &'a [Vec<String>]) {
        if remaining_pattern.is_empty() {
            // Pattern fully matched, no need to include more nodes
            return (None, remaining_pattern);
        }

        // Check if this node matches any alternative in the current pattern element
        let matches_current = remaining_pattern[0].iter().any(|alt| alt == &self.name);
        let pattern_after_self = if matches_current {
            &remaining_pattern[1..]
        } else {
            remaining_pattern
        };

        // If this node matches and pattern is now empty, include just this node
        if matches_current && pattern_after_self.is_empty() {
            return (Some(CallNode::new(self.name.clone())), pattern_after_self);
        }

        // Recursively filter children, consuming pattern elements as we go
        let mut filtered_children = Vec::new();
        let mut current_pattern = pattern_after_self;
        
        for child in &self.children {
            let (filtered_child, pattern_after_child) = child.filter_by_pattern_inner(current_pattern);
            if let Some(fc) = filtered_child {
                filtered_children.push(fc);
            }
            current_pattern = pattern_after_child;
        }

        // Include this node if it matches the current pattern element, or if any child was included
        if matches_current || !filtered_children.is_empty() {
            let mut node = CallNode::new(self.name.clone());
            node.children = filtered_children;
            (Some(node), current_pattern)
        } else {
            (None, current_pattern)
        }
    }
}

/// Generate sequential call summaries in format X{A{C,D},B}
/// For loops (repeated calls to same function), unroll twice.
/// Pattern elements can contain alternatives separated by |.
pub fn generate_call_paths(
    data: &CallGraphData,
    src_filter: &[String],
    path_pattern: Option<&[Vec<String>]>,
) -> Vec<String> {
    let mut results = Vec::new();

    /// Build a call tree for a function, recursively expanding callees.
    /// For loops, we unroll twice by allowing a function to appear at most twice in the path.
    fn build_call_tree(
        func_idx: u32,
        call_graph: &HashMap<u32, Vec<u32>>,
        function_names: &HashMap<u32, String>,
        visit_counts: &mut HashMap<u32, u32>,
    ) -> CallNode {
        let name = function_names
            .get(&func_idx)
            .cloned()
            .unwrap_or_else(|| format!("func_{}", func_idx));

        // Check if we've already visited this function twice (loop unrolling limit)
        let count = *visit_counts.get(&func_idx).unwrap_or(&0);
        if count >= 2 {
            return CallNode::new(name);
        }

        // Mark this function as visited
        *visit_counts.entry(func_idx).or_insert(0) += 1;

        let mut node = CallNode::new(name);

        // Get the ordered calls for this function
        if let Some(callees) = call_graph.get(&func_idx) {
            for &callee in callees {
                let child = build_call_tree(callee, call_graph, function_names, visit_counts);
                node.children.push(child);
            }
        }

        // Unmark this function (decrement count)
        if let Some(c) = visit_counts.get_mut(&func_idx) {
            *c -= 1;
        }

        node
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
        let mut visit_counts: HashMap<u32, u32> = HashMap::new();
        let tree = build_call_tree(
            func_idx,
            &data.call_graph,
            &data.function_names,
            &mut visit_counts,
        );

        // Check if the tree matches the path pattern
        if let Some(pattern) = path_pattern {
            if matches_path_pattern_tree(&tree, pattern) {
                // Filter the tree to only show matching paths
                if let Some(filtered) = tree.filter_by_pattern(pattern) {
                    results.push(filtered.to_string());
                }
            }
        } else {
            results.push(tree.to_string());
        }
    }

    results.sort();
    results
}

/// Check if a call tree matches a path pattern.
/// Each pattern element is a Vec of alternatives.
fn matches_path_pattern_tree(tree: &CallNode, pattern: &[Vec<String>]) -> bool {
    if pattern.is_empty() {
        return true;
    }

    let names = tree.names_in_order();
    
    // Check if pattern elements appear in order in names
    // Each pattern element can match any of its alternatives
    let mut pattern_idx = 0;
    for name in &names {
        if pattern_idx < pattern.len() && pattern[pattern_idx].iter().any(|alt| alt == name) {
            pattern_idx += 1;
        }
    }

    pattern_idx == pattern.len()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Load env symbols if provided
    let env_symbol_map = if let Some(ref env_path) = args.env_symbols {
        Some(build_env_symbol_map(env_path)?)
    } else {
        None
    };

    // Parse implicit calls
    let implicit_calls = parse_implicit_calls(&args.implicit_call)
        .map_err(|e| Box::<dyn std::error::Error>::from(e))?;

    // Determine whether to show filename prefix
    let show_filename = args.filename.unwrap_or(args.files.len() > 1);

    let mut total_paths = 0;
    
    // Parse path pattern if --paths was provided with a non-empty value
    // Each element can have alternatives separated by |
    let path_pattern: Option<Vec<Vec<String>>> = match &args.paths {
        Some(pattern) if !pattern.is_empty() => {
            Some(pattern.split("..")
                .map(|s| s.split('|').map(|alt| alt.to_string()).collect())
                .collect())
        }
        _ => None,
    };

    let use_paths_mode = args.paths.is_some();
    let has_filter = !args.src.is_empty() || !args.dst.is_empty() || path_pattern.is_some();

    for file_path in &args.files {
        let wasm_bytes = fs::read(file_path)?;
        let filename = Path::new(file_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(file_path);

        let mut data = parse_wasm_module(&wasm_bytes, env_symbol_map.as_ref())?;

        // Apply implicit calls to add edges from imports to exports
        if !implicit_calls.is_empty() {
            apply_implicit_calls(&mut data, &implicit_calls);
        }

        if use_paths_mode {
            let summaries = generate_call_paths(
                &data,
                &args.src,
                path_pattern.as_deref(),
            );

            for summary in &summaries {
                if show_filename {
                    println!("{}:{}", filename, summary);
                } else {
                    println!("{}", summary);
                }
            }
            total_paths += summaries.len();
        } else {
            let chains = enumerate_call_chains(&data, &args.src, &args.dst, args.leaves_only);

            for chain in &chains {
                if show_filename {
                    println!("{}:{}", filename, chain);
                } else {
                    println!("{}", chain);
                }
            }
            total_paths += chains.len();
        }
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

    /// Helper to create a pattern from strings. Each string can contain | for alternatives.
    fn pat(elements: &[&str]) -> Vec<Vec<String>> {
        elements.iter()
            .map(|s| s.split('|').map(|alt| alt.to_string()).collect())
            .collect()
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

        // Imports should appear as destinations in call chains
        let chains = enumerate_call_chains(&data, &["main".to_string()], &[], false);
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
    fn test_imports_not_as_starting_point() {
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

    // ============================================================
    // Tests for --paths mode (sequential call summaries)
    // ============================================================

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

    // ============================================================
    // Tests for --implicit-call
    // ============================================================

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

        // Check paths mode output
        let paths = generate_call_paths(&data, &["main".to_string()], None);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "main{host1{cb1},host2{cb2}}");
    }
}
