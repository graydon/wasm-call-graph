// Copyright 2025 Stellar Development Foundation and contributors. Licensed
// under the Apache License, Version 2.0. See the COPYING file at the root
// of this distribution or at http://www.apache.org/licenses/LICENSE-2.0

mod chains;
mod parsing;
mod paths;

use std::fs;
use std::path::Path;

use clap::Parser;

use chains::enumerate_call_chains;
use parsing::{apply_implicit_calls, build_env_symbol_map, parse_implicit_calls, parse_wasm_module};
use paths::generate_call_paths;

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
