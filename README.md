# wasm-call-graph

A static analysis tool that extracts and enumerates all possible call chains from WebAssembly modules.

## What it does

This tool parses WebAssembly bytecode modules, builds a static call graph, and outputs all possible call chains (paths through the call graph) to stdout. Each line of output represents one call chain, with function names separated by commas.

Key features:
- **Recursion inhibition**: Cycles in the call graph are detected and broken to avoid infinite paths
- **Source/destination filtering**: Filter chains by starting and/or ending function names
- **Multiple filters**: Specify multiple `--src` and/or `--dst` values to match any of them
- **Environment symbol translation**: Translate cryptic import names using a JSON mapping file
- **Multi-file support**: Process multiple WASM files in a single invocation

## Installation

```bash
cargo build --release
```

The binary will be at `target/release/wasm-call-graph`.

## Usage

```
wasm-call-graph [OPTIONS] <FILES>...
```

### Arguments

- `<FILES>...` - One or more WebAssembly (.wasm) files to analyze

### Options

- `-s, --src <SRC>` - Only show chains starting from functions with this name (can be specified multiple times)
- `-d, --dst <DST>` - Only show chains ending at functions with this name (can be specified multiple times)
- `-e, --env-symbols <ENV_SYMBOLS>` - Path to JSON file mapping import symbols to readable names
- `-f, --filename [<FILENAME>]` - Prefix output lines with filename (auto-enabled for multiple files)
- `-p, --paths [<PATTERN>]` - Output sequential call summaries instead of comma-separated chains
- `-i, --implicit-call <IMPORT:EXPORT>` - Add implicit call edge from import to export (can be specified multiple times)
- `-h, --help` - Print help
- `-V, --version` - Print version

### Exit Codes

- `0` - Success (or no filters specified)
- `1` - Filters were specified but no matching chains were found

## Examples

### Basic usage

Enumerate all call chains in a module:

```bash
wasm-call-graph module.wasm
```

Output:
```
main
main,helper
main,helper,log
helper
helper,log
log
```

### Filter by source function

Show only chains starting from `main`:

```bash
wasm-call-graph --src main module.wasm
```

### Filter by destination function

Show only chains that end at `panic`:

```bash
wasm-call-graph --dst panic module.wasm
```

### Combined filters

Show chains from `main` to `panic`:

```bash
wasm-call-graph --src main --dst panic module.wasm
```

### Multiple source/destination filters

Show chains starting from either `init` or `main`, ending at either `log` or `panic`:

```bash
wasm-call-graph --src init --src main --dst log --dst panic module.wasm
```

### Environment symbol translation

When analyzing WASM modules with obfuscated import names, provide a JSON file to translate them:

```bash
wasm-call-graph --env-symbols env.json module.wasm
```

The JSON file format:

```json
{
  "modules": [
    {
      "name": "x",
      "functions": [
        { "name": "_", "symbol": "log_message" },
        { "name": "0", "symbol": "get_time" }
      ]
    },
    {
      "name": "i",
      "functions": [
        { "name": "0", "symbol": "obj_to_u64" }
      ]
    }
  ]
}
```

This translates imports like `(import "x" "_" (func ...))` to the readable name `log_message`.

### Multiple files

Analyze multiple files (automatically prefixes output with filenames):

```bash
wasm-call-graph *.wasm
```

Output:
```
module1.wasm:main
module1.wasm:main,helper
module2.wasm:init
module2.wasm:init,setup
```

Force filename prefix on/off:

```bash
wasm-call-graph --filename true module.wasm   # Force prefix
wasm-call-graph --filename false *.wasm       # Suppress prefix
```

### Paths mode

The `--paths` option outputs call trees as nested sequential summaries instead of individual comma-separated chains. This provides a compact representation of all calls reachable from each source function.

```bash
wasm-call-graph --paths --src main module.wasm
```

Output format uses curly braces to show the call tree structure:
```
main{helper{log},process{validate,save}}
```

This means:
- `main` calls `helper` and `process`
- `helper` calls `log`
- `process` calls `validate` and `save`

#### Pattern filtering

You can filter paths to only show call chains matching a specific pattern. The pattern uses `..` to separate function names that must appear in order:

```bash
wasm-call-graph --paths="main..validate..save" module.wasm
```

This shows only paths where `main` eventually calls `validate`, which eventually calls `save`. Functions between pattern elements are included in the output.

#### Pattern alternatives with `|`

Use `|` to match any of several alternatives at a pattern position:

```bash
wasm-call-graph --paths="main..log|error..cleanup" module.wasm
```

This matches paths through either `log` or `error` before reaching `cleanup`.

You can also use alternatives for the starting point:

```bash
wasm-call-graph --paths="init|main..shutdown" module.wasm
```

#### Loop handling

Loops (recursive calls) are detected and unrolled twice to capture the loop body calls before being truncated. This ensures calls made within recursive functions are properly represented.

### Implicit calls

The `--implicit-call` option adds edges to the call graph for host callbacks. This is useful when a WASM module imports a function that internally calls back into an exported function.

```bash
wasm-call-graph --implicit-call "register_callback:on_event" module.wasm
```

The format is `import_name:export_name`. This adds a call edge from the import `register_callback` to the export `on_event`, so any function calling `register_callback` will show `on_event` in its call chain.

You can specify multiple implicit calls:

```bash
wasm-call-graph \
  --implicit-call "set_timer:timer_callback" \
  --implicit-call "add_listener:event_handler" \
  module.wasm
```

This is particularly useful for analyzing modules that use callback-based APIs where the actual control flow isn't visible in the WASM bytecode.

## How it works

1. **Parse imports**: Extracts imported functions and assigns them indices starting at 0
2. **Parse exports**: Records exported function names
3. **Parse name section**: Extracts debug names from the WASM name section (if present)
4. **Apply env symbol translation**: Overrides import names using the provided JSON mapping
5. **Build call graph**: Scans all function bodies for `call` instructions
6. **Enumerate chains**: Performs depth-first search from each function (or filtered sources), tracking visited nodes to prevent cycles

## License

See repository for license information.
