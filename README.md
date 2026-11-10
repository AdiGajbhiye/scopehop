# scopehop

A lightweight, scope-aware language server: go-to-definition and
find-references using tree-sitter and the files in your workspace. No
compiler, no build sync, no type checker. Starts in a fraction of a second.

**Languages:** Kotlin today. The goal is any language with a tree-sitter
grammar; see [Roadmap](#roadmap).

## What it does (Kotlin)

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
cargo install --path . --root ~/.local   # puts scopehop in ~/.local/bin
```

### Zed

1. `zed: install dev extension` → pick the `zed/` folder.
2. In `settings.json`:

```json
"languages": {
  "Kotlin": { "language_servers": ["scopehop"] }
}
```

Add `"..."` to the list to keep other Kotlin servers running alongside. The
extension finds `scopehop` on `PATH`, or set
`"lsp": { "scopehop": { "binary": { "path": "/abs/path/scopehop" } } }`.

## Roadmap

Grammar-specific knowledge sits behind the `Language` trait in `src/lang.rs`:
which nodes declare names, which open scopes, where type hints live and what
shape an expression has. The index, ranking, references and LSP layer only
talk to that trait. Adding a language means adding a grammar crate, a type
implementing `Language`, and an entry in `lang::LANGUAGES` (plus the language
name in `zed/extension.toml`). Where a grammar ships `locals.scm`/`tags.scm`
queries, a query-driven implementation could cover most of the trait.

Two pieces of `src/index.rs` still assume a JVM-style language: visibility
ranking uses packages and imports, and top-level functions can match as
members (extension functions). Languages that import modules by path will
need a hook for the first.

## Layout

- `src/lang.rs`: the `Language` trait, the shared declaration model and the
  language registry
- `src/kotlin.rs`: the Kotlin implementation (declarations, scopes, type hints)
- `src/index.rs`: workspace index and resolution, language-neutral
- `src/main.rs`: LSP over stdio (`lsp-server`)
- `zed/`: Zed extension that launches the server
- `examples/`: `probe` (one lookup from the CLI), `stats` (resolve every
  identifier in a project), `dump` (print a syntax tree)

```sh
cargo run --release --example probe -- <root> <file> <line> <col> [def|refs]
cargo run --release --example stats -- <root>
```
