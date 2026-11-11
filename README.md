# scopehop

A lightweight, scope-aware language server: go-to-definition and
find-references using tree-sitter and the files in your workspace. No
compiler, no build sync, no type checker. Starts in a fraction of a second.

**Languages:** Kotlin and Java, including mixed projects where each calls the
other, and Python. The goal is any language with a tree-sitter grammar; see
[Roadmap](#roadmap).

## What it does (Kotlin)

- **Definition** (`textDocument/definition`)
  - Scope-aware: lambda params (incl. implicit `it`), function params,
    `for`/`catch`/`when` variables, local `val`/`var` declared earlier in the
    block, class members, then the file's top level.
  - Project-wide lookups only consider what Kotlin can see: same file,
    imports (incl. `*` and `as`), same package, inherited members.
  - Names on `import` lines resolve to the declaration they import.
  - `x.foo` infers `x`'s type from annotations, `= Foo()`, initializers and
    return types, then looks `foo` up in that class and its supertypes.
    Library receivers don't match project members.
  - Named arguments, `Outer.Inner` types, extension receivers (`fun Dp.f()`),
    infix calls and `"$name"` string templates.
- **References** (`textDocument/references`): locals are searched in their own
  file; everything else is a whole-word search across the workspace, keeping
  only hits that resolve to the same declaration.
- **Document highlights** and **workspace symbols**.

## What it does (Java)

The same requests, with the same rules where Java has them:

- Scopes: lambda params, method and constructor params, `for`/enhanced `for`,
  `catch`, try-with-resources, `instanceof` pattern variables, locals declared
  earlier in the block, members of the class (and of anonymous classes).
- Imports: single-type, `*`, `static` member and `static *` imports; same
  package; inherited members. Names on `import` lines resolve too.
- Types come from declarations, `var x = new Foo()`, casts, return types
  (also of calls without a receiver) and fields. Arrays are not their element
  type. Constructors and `new Foo()` go to the class; records' components work
  as fields and accessors (`p.x()`).

Kotlin and Java files share one index, so `import com.example.JavaThing` in
Kotlin and `new KotlinThing()` in Java both resolve. Kotlin's property syntax
for Java getters (`thing.name` for `getName()`) does not resolve yet.

## What it does (Python)

- Scopes follow Python, not blocks: a name assigned anywhere in a function is
  local to all of it, and every use goes to its first binding there
  (assignments, `for`, `with ... as`, `except ... as`, walrus, `match`
  captures, nested `def`/`class`). Lambdas and comprehensions have their own
  scope, `global`/`nonlocal` are honored, and class bodies are not visible
  from their methods. Default values resolve outside the function.
- Every file is a module, named by its package chain (`__init__.py`
  directories), or by its path under the workspace root (minus a leading
  `src/`) when it is not in a package. A bare name is the file's own or
  imported: `from m import x [as y]`, `import a.b [as m]`, `from m import *`,
  and relative imports (`from .models import Foo`, `from .. import x`).
  Imports follow re-exports, so `from pkg import Foo` reaches `Foo` when
  `pkg/__init__.py` imports it. Names on `import` lines resolve too, modules
  to their file.
- Modules work as receivers (`m.make()`, `pkg.sub.CONST`).
- Types come from annotations (incl. `"Foo"`, `Optional[Foo]`, `Foo | None`),
  `self`/`cls`, `Foo()` calls, return annotations and `@property` getters.
  Instance attributes are the `self.x = ...` assignments in methods, typed by
  their annotation, by `Foo()`, or by the annotated parameter they are
  assigned from. Members are looked up through base classes and `super()`.
- Classes with the same name in different modules stay apart: a type
  remembers the file it was written in and is resolved there.
- Keyword arguments go to the parameter: `__init__`'s for a class, or a
  dataclass's annotated fields.

Python files never resolve to Kotlin or Java declarations, or the other way
round.

## Limits

When a type can't be inferred (e.g. untyped lambda params), member lookups fall
back to every project member with that name, and the receiver type is whatever
most of those members agree on, so results can include extra candidates. Not
handled yet: Java `switch` pattern variables (`case Circle c ->`), and imports
of types nested two levels deep (`Outer.Middle.Inner`), since a declaration
only records its immediate container.

Python: no element types for containers (`list[Foo]`, `for x in foos`), no
`import a.b` when `a` is a namespace package without `__init__.py`, and no
keyword names in class patterns (`case Point(x=...)`). Names that only exist
at runtime (`setattr`, `__getattr__`, `globals()`) are not seen. On a sample
of the standard library (asyncio, email, http, json, logging, unittest,
concurrent: 100 files, 68k identifiers), 81% resolve to exactly one
declaration and 3% to several; most of the rest are builtins and modules
outside the sample.

## Install

```sh
cargo install --path . --root ~/.local   # puts scopehop in ~/.local/bin
```

### Zed

1. `zed: install dev extension` → pick the `zed/` folder.
2. In `settings.json`:

```json
"languages": {
  "Kotlin": { "language_servers": ["scopehop"] },
  "Java": { "language_servers": ["scopehop"] },
  "Python": { "language_servers": ["scopehop"] }
}
```

Add `"..."` to a list to keep that language's other servers running alongside. The
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

Three parts of the trait decide how a language fits the shared index:

- `family()`: languages in the same family see each other's declarations
  (Kotlin and Java are `"jvm"`). Others are never candidates.
- `visibility()`: `Packages` (JVM: same package visible, inherited members
  visible bare, top-level functions as extension members) or `Modules` (one
  module per file, imports and re-exports, modules as receivers).
- `place()` and `absolute_import()`: fill in what depends on the file's
  location, such as a module name from the path and absolute paths for
  relative imports.

TypeScript/JavaScript and Rust should fit `Modules` with their own `place()`;
Go needs package-qualified access (`fmt.Println`) on top of `Packages`.

## Layout

- `src/lang.rs`: the `Language` trait, the shared declaration model and the
  language registry
- `src/kotlin.rs`, `src/java.rs`, `src/python.rs`: the implementations
  (declarations, scopes, type hints)
- `src/index.rs`: workspace index and resolution, language-neutral
- `src/snapshot.rs`: text dump of everything the index resolves, for tests
- `tests/`: resolution tests over small Kotlin, Java, mixed and Python
  projects in `tests/fixtures/`, plus a snapshot of every lookup in each
- `src/main.rs`: LSP over stdio (`lsp-server`)
- `zed/`: Zed extension that launches the server
- `examples/`: `probe` (one lookup from the CLI), `stats` (resolve every
  identifier in a project), `snapshot` (dump every lookup in a project, to
  diff two builds), `dump` (print a syntax tree)

```sh
cargo run --release --example probe -- <root> <file> <line> <col> [def|refs]
cargo run --release --example stats -- <root>
```

## Testing

```sh
cargo test
```

`tests/kotlin.rs`, `tests/java.rs`, `tests/mixed.rs` and `tests/python.rs`
each check specific lookups in their project under `tests/fixtures/` (scopes,
members, imports, packages, references) and compare every lookup in it against
`tests/snapshots/<name>.txt`. After an intended change, accept the new
snapshots and review their diff:

```sh
UPDATE_SNAPSHOTS=1 cargo test
git diff tests/snapshots
```

To see how a change affects a real project, dump it before and after:

```sh
cargo run --release --example snapshot -- <root> > before.txt
# make the change
cargo run --release --example snapshot -- <root> > after.txt
diff before.txt after.txt
```

Pass `--no-refs` to skip references on large projects.
