// Copyright 2025 Stellar Development Foundation and contributors. Licensed
// under the Apache License, Version 2.0. See the COPYING file at the root
// of this distribution or at http://www.apache.org/licenses/LICENSE-2.0

use std::collections::{HashMap, HashSet};

use crate::parsing::CallGraphData;

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

#[cfg(test)]
mod tests;
