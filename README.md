# scopehop

A lightweight, scope-aware language server: go-to-definition and
find-references using tree-sitter and the files in your workspace. No
compiler, no build sync, no type checker. Starts in a fraction of a second.

**Languages:** Kotlin and Java, including mixed projects where each calls the
other, Python, Go, TypeScript/JavaScript (including TSX and JSX), and C#. The goal is any language with a tree-sitter grammar; see
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

In both, classes with the same name in different packages stay apart when
a file sees only one of them (its own, its package's, or an imported one),
an `import ... as` alias works as a type, and an inherited member declared
lower in the hierarchy (an override) wins over the one it overrides.

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

## What it does (C#)

- Scopes: locals declared earlier in the block, local functions (visible
  in the whole block), lambda and method parameters, `for`/`foreach`,
  `catch`, `using`, LINQ query variables (`from`, `let`, `join`, `into`),
  type parameters, primary constructor parameters, and setters' `value`.
  Pattern and `out` variables (`o is Circle c`, `case Point p`, `out var
  n`) are visible where C# puts them: past an `if` or expression statement
  into the rest of the block, along `&&`, into a switch arm.
- Namespaces are packages. `using N;`, `using static N.C;`, `using A =
  N.C;` (a namespace alias too: `A.Circle`), parent namespaces, file-scoped
  and nested namespaces. A namespace's own types win over `using`
  directives, as in C#. `global using` and the project file's `<Using
  Include="..."/>` items apply to the files of their project (the nearest
  `.csproj`) only. Fully qualified names in code (`Shop.Models.Circle`)
  resolve.
- Members of classes, structs, interfaces, records (positional parameters
  are properties) and enums; partial classes (a bare name in one part finds
  members of the others; go to definition picks the main part: the file
  named after the class, or the part that names the supertypes). Extension
  methods are found on their receiver (`"hi".Shout()`), and only when called.
- Types come from declarations, `var x = new Foo()`, target-typed `new()`,
  casts, `as`, `default(T)`, return types, properties, `where T : IShape`
  constraints, and `Func<A, R>`/`Action<A>` for lambda parameters.
  Generic arguments are kept, so `foreach`, `xs[i]` and `await` give
  `List<T>`/`T[]`/`Dictionary<K, V>`/`Task<T>` element types. LINQ (and
  Entity Framework) calls keep their element type: `xs.Where(x =>
  ...).First()` is an element of `xs`, and so is `x`. A lambda passed to a
  method on any type with one generic argument takes that argument
  (`repo.Find(u => ...)` on `IRepository<User>`, `builder.Property(o =>
  ...)` on `EntityTypeBuilder<Order>`), and so does one passed to a bare
  call in a class whose base has one (FluentValidation's `RuleFor(x =>
  ...)`). `Get<Foo>()` returns a `Foo` (a `Task<Foo>` when it is
  `...Async`).
- Overloads are told apart by argument count (including optional and
  `params` parameters, and an extension method's receiver), and generic
  types by their number of type arguments (`Box` and `Box<T>`). Named
  arguments go to the parameter, including constructors' (`new
  Circle(radius: 2)`), and object initializer keys (`new Circle { Radius =
  1 }`, `with { X = 1 }`) to the member.
- tree-sitter-c-sharp only parses preprocessor directives between whole
  declarations and statements. A file that doesn't parse is parsed again
  with only the first branch of each `#if` kept and every directive line
  blanked (same length, same lines), and the better tree is kept.

C# files only resolve to C# declarations.

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

C#: overloads with the same number of parameters (`WriteValue(int)` and
`WriteValue(long)`) all match, as nothing is typed by its arguments. A
lambda assigned to a project delegate type (`Measure m = s => ...`) has an
untyped parameter, and so does one passed to a project method that takes a
delegate. An untyped receiver's members match every project member with
that name (in the class's own namespace when the type is known). Members of
anonymous types and `dynamic` don't resolve, attribute names
(`[Required]`) don't reach their `...Attribute` class, and assemblies
(NuGet packages, the base class library) are not indexed. On eShop (547
files, 40k identifiers) 55% resolve to exactly one declaration and 0.45%
to several; on Newtonsoft.Json (951 files, 229k identifiers) 66% and
2.2%, nearly all of those being overloads that differ only by parameter
types. Nearly all of the rest are base class library and NuGet names.

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
  "JavaScript": { "language_servers": ["scopehop", "..."] },
  "CSharp": { "language_servers": ["scopehop", "..."] }
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
- `visibility()`: `Packages` (JVM and C# namespaces: same package visible,
  inherited members visible bare, top-level functions as extension members) or `Modules` (a
  module is a file in Python and TypeScript or a directory in Go; imports
  and re-exports, modules as receivers, type names read in the file that
  wrote them).
- `place()` and `absolute_import()`: fill in what depends on the file's
  location, such as a module name from the path (or `go.mod`), absolute
  paths for relative imports, `tsconfig.json` path aliases, and the C#
  project a file belongs to.

Rust should fit `Modules` with its own `place()`.

## Layout

- `src/lang.rs`: the `Language` trait, the shared declaration model and the
  language registry
- `src/kotlin.rs`, `src/java.rs`, `src/python.rs`, `src/go.rs`,
  `src/typescript.rs`, `src/csharp.rs`: the implementations
  (declarations, scopes, type hints)
- `src/index.rs`: workspace index and resolution, language-neutral
- `src/snapshot.rs`: text dump of everything the index resolves, for tests
- `tests/`: resolution tests over small Kotlin, Java, mixed, Python, Go,
  TypeScript and C# projects in `tests/fixtures/`, plus a snapshot of every lookup in each
- `src/main.rs`: LSP over stdio (`lsp-server`)
- `zed/`: Zed extension that launches the server
- `examples/`: `probe` (one lookup from the CLI), `stats` (resolve every
  identifier in a project), `snapshot` (dump every lookup in a project, to
  diff two builds), `dump` (print a syntax tree), `errcount` (files that
  don't parse, before and after `repair()`)

```sh
cargo run --release --example probe -- <root> <file> <line> <col> [def|refs]
cargo run --release --example stats -- <root>
```

## Testing

```sh
cargo test
```

`tests/kotlin.rs`, `tests/java.rs`, `tests/mixed.rs`, `tests/python.rs`,
`tests/go.rs`, `tests/typescript.rs` and `tests/csharp.rs` each check specific lookups in their project under `tests/fixtures/` (scopes,
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
