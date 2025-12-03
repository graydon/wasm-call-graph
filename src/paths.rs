// Copyright 2025 Stellar Development Foundation and contributors. Licensed
// under the Apache License, Version 2.0. See the COPYING file at the root
// of this distribution or at http://www.apache.org/licenses/LICENSE-2.0

use std::collections::HashMap;

use crate::parsing::CallGraphData;

/// A tree node representing a function call and its children
#[derive(Debug, Clone)]
pub struct CallNode {
    pub name: String,
    pub children: Vec<CallNode>,
}

impl CallNode {
    pub fn new(name: String) -> Self {
        CallNode { name, children: Vec::new() }
    }

    /// Convert the tree to a string in format X{A{C,D},B}
    pub fn to_string(&self) -> String {
        if self.children.is_empty() {
            self.name.clone()
        } else {
            let child_strs: Vec<String> = self.children.iter().map(|c| c.to_string()).collect();
            format!("{}{{{}}}", self.name, child_strs.join(","))
        }
    }

    /// Extract all names in order (depth-first, pre-order)
    pub fn names_in_order(&self) -> Vec<String> {
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
    pub fn filter_by_pattern(&self, remaining_pattern: &[Vec<String>]) -> Option<CallNode> {
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
pub fn matches_path_pattern_tree(tree: &CallNode, pattern: &[Vec<String>]) -> bool {
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

#[cfg(test)]
mod tests;
