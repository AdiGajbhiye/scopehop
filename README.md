# scopehop

A lightweight, scope-aware language server: go-to-definition and
find-references using tree-sitter and the files in your workspace. No
compiler, no build sync, no type checker. Starts in a fraction of a second.

**Languages:** Kotlin and Java, including mixed projects where each calls the
other, Python, Go, and TypeScript/JavaScript (including TSX and JSX). The goal is any language with a tree-sitter grammar; see
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

## What it does (Go)

- Block scopes, with a local visible only after its declaration: `:=`, `var`,
  `const`, local types, `if`/`switch`/`for` initializers, `range`
  variables, `case v := <-ch`, type switch aliases (in their cases), closure
  parameters, named results, labels, and type parameters (including the
  ones a generic receiver declares, `func (b *Box[T])`). `a, err := f()`
  reuses an `err` declared earlier in the same block, as Go does.
- A package is a directory, named by its import path: the `module` line of
  the nearest `go.mod` plus the directory under it (`vendor/` paths drop
  the prefix). Its files see each other's top-level names. An external test
  package (`package foo_test`) is separate from `foo`.
- Imports bind a package name (the alias, or the last element of the path,
  skipping `/v2` and `yaml.v3`-style version suffixes) used as a qualifier:
  `models.NewCircle()`, `*models.Circle`. Dot imports put the package's
  names in scope. Import paths, aliases and package clauses resolve to the
  package (its first file).
- Methods belong to their receiver's type wherever they are declared in the
  package, alongside struct fields and interface methods. Embedded fields
  and interfaces are supertypes, so promoted members resolve
  (`c.Name()` through an embedded `Base`).
- Types come from declarations, `T{...}`, `&T{...}`, `new(T)`, conversions,
  type assertions, and function and method results (the first of several).
  Slice, map and channel types keep their element types, so `range`,
  `xs[i]`, `m[k]` and `<-ch` are typed whatever the container comes from
  (a field, a call, a named type such as `type Routes []Route`). Untyped
  package-level `var x = f()` is typed from its initializer when it is used
  as a receiver. A `const` group's implicit repetitions keep the type
  (`Green` after `Red Color = iota`).
- Anonymous structs have members too, so table-driven tests work:
  `tt.name` in `for _, tt := range tests`, where `tests := []struct{ name
  string }{{name: "a"}}`, goes to the field, and so does the `name:` key.
- Build constraints: files left out of a plain `go build` on this machine
  (`//go:build nomsgpack`, `_windows.go`) are still indexed, but a name
  declared in both such a file and its twin goes to the twin that is in
  the build. The blank identifier `_` names nothing.
- Same-named types in different packages stay apart: a type name keeps its
  qualifier and is read in the file it was written in.
- Keys of struct literals go to the field, including elided element
  literals (`[]Point{{X: 1}}`). Map literal keys stay expressions.

Go files only resolve to Go declarations.

## What it does (TypeScript and JavaScript)

- Block scopes for `let`, `const`, classes and function declarations;
  function scopes for parameters (including destructured ones) and hoisted
  `var`; `for`, `for...of`, `catch`, named function expressions, labels
  and type parameters.
- Every file is a module. Imports resolve through relative paths
  (`./x`, `./x.js`, a directory's `index` file), `paths` and `baseUrl` in
  the nearest `tsconfig.json` or `jsconfig.json` (comments, trailing commas
  and a relative `extends` are fine), named, default and namespace
  imports, re-exports (`export { a as b } from`, `export * from`,
  `export { default as X } from`), and top-level `require`. Specifiers and
  names in import statements resolve to the module or declaration.
- Classes, interfaces, enums and object type aliases have members, with
  `extends` and `implements` as supertypes, so `super.area()` and members
  inherited from interfaces resolve. Constructor parameter properties
  (`constructor(public radius: number)`) and JavaScript's `this.count = 0`
  assignments are fields.
- Types come from annotations, `new T()`, `x as T`, return types (a getter's
  too), and untyped top-level `const x = f()` when used as a receiver.
  `Promise<T>` reads as `T`, so `await` keeps the type. Arrays, `Set` and
  other iterables give their element type to `for...of` and `xs[i]`, and
  `Record<K, V>` to indexing.
- Object literals have members: `config.db.host` goes to the `host:` key.
  A literal written against a type (`const p: Point = { x: 1 }`, `as`,
  `satisfies`, an array of `Point`, a function's return type) has that
  type's members as keys.
- Destructured names get the member's type: `circle` in `function
  Badge({ circle }: Props)`, or `const { a: { b } } = x`.
- Overloads (and a getter with its setter) are one declaration, the first.
  Type parameters of call signatures, `infer X` and mapped types (`[K in
  keyof T]`) are scoped like any other.
- JSX components resolve like any name; attribute names resolve to
  nothing. JavaScript files use the TSX grammar.
- tree-sitter-typescript fails on two valid forms: call signatures on
  separate lines with no `;` between them, and `export type * from`. A file
  that doesn't parse is parsed again with those patched (same length, same
  lines), and the better tree is kept.

TypeScript and JavaScript files resolve to each other's declarations and to
nothing else. `node_modules` is never indexed.

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

Go: in `a, err := f()` only `a` gets a type (the index records a function's
first result only), so `err.Error()` matches every project `Error` method.
A type switch alias (`v` in `case *Circle:`) has no type. A package whose
name differs from the last element of its import path is only found
through an alias. Standard library and module cache packages are not
indexed, so their names resolve to nothing. On gin (99 files, 50k
identifiers) 63% resolve to exactly one declaration and 7 identifiers to
several; on caddy (414 files, 207k identifiers) 69% and 0.1%. Nearly all of
the rest are standard library and dependency names.

TypeScript: no names containing `$` and no `#private` members (the word
under the cursor stops at those characters). Destructuring keys
(`center` in `const { center: { x } } = c`) and JSX attribute names don't
resolve to the member they name, and nothing in an argument is typed from
the parameter it is passed to. `module.exports = ...` exports are not
followed: a `require`d name resolves to the module's top-level declaration
of that name. Packages from `node_modules` and the standard library types
are not indexed, and a callback parameter typed only through a library's
generics (`app.get("/", (c) => c.text("hi"))`) is untyped, so its members
match every project member with that name. On hono (460 files, 147k
identifiers) 65% resolve to exactly one declaration and 2.2% to several;
on taxonomy, a Next.js app (131 files), 49% and 1.2%. Nearly all of the
rest are names from libraries, the DOM and test frameworks.

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
  "Python": { "language_servers": ["scopehop"] },
  "Go": { "language_servers": ["scopehop", "..."] },
  "TypeScript": { "language_servers": ["scopehop", "..."] },
  "TSX": { "language_servers": ["scopehop", "..."] },
  "JavaScript": { "language_servers": ["scopehop", "..."] }
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
  visible bare, top-level functions as extension members) or `Modules` (a
  module is a file in Python and TypeScript or a directory in Go; imports
  and re-exports, modules as receivers, type names read in the file that
  wrote them).
- `place()` and `absolute_import()`: fill in what depends on the file's
  location, such as a module name from the path (or `go.mod`), absolute
  paths for relative imports, and `tsconfig.json` path aliases.

Rust should fit `Modules` with its own `place()`.

## Layout

- `src/lang.rs`: the `Language` trait, the shared declaration model and the
  language registry
- `src/kotlin.rs`, `src/java.rs`, `src/python.rs`, `src/go.rs`,
  `src/typescript.rs`: the implementations
  (declarations, scopes, type hints)
- `src/index.rs`: workspace index and resolution, language-neutral
- `src/snapshot.rs`: text dump of everything the index resolves, for tests
- `tests/`: resolution tests over small Kotlin, Java, mixed, Python, Go and
  TypeScript projects in `tests/fixtures/`, plus a snapshot of every lookup in each
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

`tests/kotlin.rs`, `tests/java.rs`, `tests/mixed.rs`, `tests/python.rs`,
`tests/go.rs` and `tests/typescript.rs` each check specific lookups in their project under `tests/fixtures/` (scopes,
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
