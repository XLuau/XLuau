# Tooling and Project Setup

This page covers what the current repository supports today.

## Compiler Commands

### Build the whole project

```bash
cargo run -- build
```

This reads `xluau.config.json`, compiles matching `.xl` files, and writes `.luau` output under `outDir`.

### Build one file

```bash
cargo run -- build src/main.xl
```

### Check without writing files

```bash
cargo run -- check
```

### Check one file

```bash
cargo run -- check src/main.xl
```

### Format source files

```bash
cargo run -- fmt src/
```

Use `--check` to verify formatting in CI without rewriting files.

### Run an entry file

```bash
cargo run -- run src/main.xl
```

This builds the selected entry, writes the emitted `.luau`, and launches a Luau runtime command.

## Package Manager

The repository now includes an `xlpkg`-style package workflow through the main `xluau` CLI.

### Current package commands

- `cargo run -- install ...`
- `cargo run -- remove ...`
- `cargo run -- update ...`
- `cargo run -- list`
- `cargo run -- bundle`
- `cargo run -- publish --dry-run`

### Current package behavior

- installed package source is stored in `xluau_packages/`
- exact pins are written to `xluau.lock`
- `packages.luau` is regenerated from the installed package set
- `require "@name"` resolves through the generated package bundle when `packages` is configured
- `publish` updates the sibling `XLpkg` repo's `index.json` when that registry repo is checked out next to `XLLuau`
- local `file:` package sources are fully covered by the repository test suite

## Language Server

The repository now ships a baseline language server as the `xluau-lsp` binary.

### Build it

```bash
cargo build --bin xluau-lsp
```

### Current LSP features

- parse and validation diagnostics for open `.xl` files
- document formatting through the same formatter used by `xluau fmt`
- top-level document symbols for functions, objects, enums, signals, state, type aliases, and top-level locals
- keyword, builtin global, and builtin type completions
- enum member completions, string member completions, and typed-object member completions when the current file provides enough annotation information
- hover for top-level declarations, builtin globals/types, string members, and resolved `require(...)` strings
- simple local type inference for literals and `#value` expressions such as strings and array-like tables
- go-to-definition for current-file top-level declarations and alias-resolved `require(...)` targets, including index-file resolution
- rename for current-file declaration names and project-wide `require(...)` specifier strings
- quick fixes for:
  - converting `const` declarations to `local`
  - adding fallback branches for non-exhaustive `switch`
  - adding fallback branches for non-exhaustive `match`

### Current LSP limits

- completions are still syntax- and annotation-driven rather than full semantic inference
- rename is intentionally conservative and does not yet do symbol-accurate cross-file refactors for arbitrary locals
- code actions are focused on a small set of high-confidence fixes
- hover and definition do not yet trace arbitrary exported values across module return tables

## VS Code Extension

A VS Code client lives under `vscode/xluau-vscode`.

### What it provides today

- `.xl` language registration
- syntax highlighting
- bracket and comment configuration
- diagnostics, formatting, symbols, completions, hover, definition, rename, and quick fixes through `xluau-lsp`
- a `xluau.restartServer` command

### Local development

```bash
cargo build --bin xluau-lsp
cd vscode/xluau-vscode
npm install
```

Then open the extension folder in VS Code and press `F5`.

### Current extension limits

- the server is launched from your local build output or `PATH`
- the extension does not bundle platform binaries yet
- file icons and task definitions are not added yet

## File Extensions

- `.xl`: XLuau source
- `.luau`: generated output, or plain Luau input
- `.lua`: also supported as a module resolution extension in config

## Current Config File

Create `xluau.config.json` in the project root.

Example:

```json
{
  "include": ["src/**/*.xl"],
  "exclude": [],
  "outDir": "out",
  "target": "filesystem",
  "baseDir": "src",
  "paths": {
    "@shared": "./src/shared"
  },
  "extensions": [".xl", ".luau", ".lua"],
  "indexFiles": ["init"]
}
```

## Current Targets

### `filesystem`

Emits string-style `require(...)` paths such as:

```lua
require("./src/shared/math")
```

### `roblox`

Emits instance-path requires such as:

```lua
require(script.Parent.Parent.shared.math)
```

When you run `build` for a Roblox target, XLuau also emits sibling `.rbxmx` files next to the generated `.luau` output.

Current wrapper heuristic:

- `src/server/main.xl` -> `Script`
- `src/client/main.xl` -> `LocalScript`
- everything else -> `ModuleScript`

### `custom`

Calls your configured resolver function:

```lua
require(resolveModule("shared/math"))
```

## Suggested Workflow

1. Write `.xl` files under `src/`.
2. Keep reusable modules under aliased folders like `src/shared`.
3. Run `cargo run -- fmt --check` or `cargo run -- fmt` before commits.
4. Run `cargo run -- check` while iterating.
5. Run `cargo run -- build` when you want emitted output.
6. Use `cargo run -- run ...` when you want a compile-and-execute shortcut.
7. Treat `out/` as generated code.

## What Is Still Planned

The remaining phase 9 work is mostly editor depth:

- deeper semantic completions and richer type-driven hover
- broader cross-file rename and symbol navigation
- more automated code actions
- richer VS Code packaging
