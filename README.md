# treenav

A lightweight language server for Kotlin: go-to-definition and find-references
using tree-sitter and the files in your workspace. No JVM, no Gradle sync, no
type checker. Starts in a fraction of a second.

## What it does

- **Definition** (`textDocument/definition`)
  - Scope-aware: lambda params (incl. implicit `it`), function params,
    `for`/`catch`/`when` variables, local `val`/`var` declared earlier in the
    block, class members, then the file's top level.
  - Project-wide lookups only consider what Kotlin can see: same file,
    imports (incl. `*` and `as`), same package, inherited members.
  - `x.foo` infers `x`'s type from annotations, `= Foo()`, initializers and
    return types, then looks `foo` up in that class and its supertypes.
    Library receivers don't match project members.
  - Named arguments, `Outer.Inner` types, extension receivers (`fun Dp.f()`),
    infix calls and `"$name"` string templates.
- **References** (`textDocument/references`): locals are searched in their own
  file; everything else is a whole-word search across the workspace, keeping
  only hits that resolve to the same declaration.
- **Document highlights** and **workspace symbols**.

When a type can't be inferred (e.g. untyped lambda params), member lookups fall
back to every project member with that name, so results can include extra
candidates.

## Install

```sh
cargo install --path . --root ~/.local   # puts treenav in ~/.local/bin
```

### Zed

1. `zed: install dev extension` → pick the `zed/` folder.
2. In `settings.json`:

```json
"languages": {
  "Kotlin": { "language_servers": ["treenav"] }
}
```

Add `"..."` to the list to keep other Kotlin servers running alongside. The
extension finds `treenav` on `PATH`, or set
`"lsp": { "treenav": { "binary": { "path": "/abs/path/treenav" } } }`.

## Layout

- `src/kotlin.rs`: grammar-specific bits (declarations, scopes, type hints)
- `src/index.rs`: workspace index and resolution
- `src/main.rs`: LSP over stdio (`lsp-server`)
- `zed/`: Zed extension that launches the server
- `examples/`: `probe` (one lookup from the CLI), `stats` (resolve every
  identifier in a project), `dump` (print a syntax tree)

```sh
cargo run --release --example probe -- <root> <file> <line> <col> [def|refs]
cargo run --release --example stats -- <root>
```
