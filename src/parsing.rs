// Copyright 2025 Stellar Development Foundation and contributors. Licensed
// under the Apache License, Version 2.0. See the COPYING file at the root
// of this distribution or at http://www.apache.org/licenses/LICENSE-2.0

use std::collections::{HashMap, HashSet};
use std::fs;

use serde::Deserialize;
use wasmparser::{ExternalKind, Name, Operator, Payload, TypeRef};

/// Represents a function entry in the env.json module
#[derive(Debug, Deserialize)]
pub struct EnvFunction {
    pub export: String,
    pub name: String,
}

/// Represents a module entry in the env.json file
#[derive(Debug, Deserialize)]
pub struct EnvModule {
    pub export: String,
    pub functions: Vec<EnvFunction>,
}

/// Root structure of env.json
#[derive(Debug, Deserialize)]
pub struct EnvConfig {
    pub modules: Vec<EnvModule>,
}

/// Build a lookup map from "module_export.func_export" -> "long_name"
pub fn build_env_symbol_map(env_path: &str) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
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
pub fn parse_implicit_calls(args: &[String]) -> Result<HashMap<String, String>, String> {
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

#[cfg(test)]
mod tests;
