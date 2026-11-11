//! Resolution tests against the project in `tests/fixtures/typescript`. See
//! `common` for how cursors work.
//!
//! The fixture has two modules with a `Circle` class (`models/shapes.ts`
//! and the decoy `other/shapes.ts`), re-exports through `models/index.ts`,
//! a `tsconfig.json` with an `@/*` path alias, a TSX component, a CommonJS
//! file, and a Python `Circle` that TypeScript lookups must never return.

mod common;

use common::Fx;

const MAIN: &str = "src/app/main.ts";
const HELPER: &str = "src/app/helper.ts";
const VIEW: &str = "src/app/view.tsx";
const SHAPES: &str = "src/models/shapes.ts";
const COLOR: &str = "src/models/color.ts";
const INDEX: &str = "src/models/index.ts";
const STRINGS: &str = "src/util/strings.ts";
const DECOY: &str = "src/other/shapes.ts";
const OLD: &str = "src/legacy/old.js";
const ADVANCED: &str = "src/app/advanced.ts";

fn fx() -> Fx {
    Fx::new("typescript")
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

#[test]
fn blocks_and_shadowing() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "console.log(|total);" => MAIN, "    const |total = 1");
    assert_def!(fx, MAIN, "twice(|total)" => MAIN, "  let |total = 0");
    assert_def!(fx, MAIN, "i + items[|i]" => MAIN, "for (let |i = 0");
    assert_def!(fx, MAIN, "|limit) {\n    var" => MAIN, "|limit = 10");
}

#[test]
fn hoisting_catch_closures_and_labels() {
    let mut fx = fx();
    // `var` belongs to the function, not its block.
    assert_def!(fx, MAIN, "console.log(err, |late)" => MAIN, "var |late");
    assert_def!(fx, MAIN, "console.log(|err, late)" => MAIN, "catch (|err)");
    assert_def!(fx, MAIN, "=> |v * 2" => MAIN, "(|v: number)");
    assert_def!(fx, MAIN, "break |outer" => MAIN, "|outer:");
    assert_def!(fx, MAIN, "+ |radius + x" => MAIN, "const { |radius");
    assert_def!(fx, MAIN, "radius + |x;" => MAIN, "center: { |x }");
}

// ---------------------------------------------------------------------------
// Modules and imports
// ---------------------------------------------------------------------------

#[test]
fn imports_follow_re_exports() {
    let mut fx = fx();
    // `models/index.ts` re-exports `shapes` and `color`.
    assert_def!(fx, MAIN, "= |makeCircle(2)" => SHAPES, "export function |makeCircle");
    assert_def!(fx, MAIN, "console.log(|Colour.Red" => COLOR, "export enum |Color");
    assert_def!(fx, MAIN, "new |Palette()" => COLOR, "export default class |Palette");
    assert_def!(fx, MAIN, "|helper(new Canvas" => HELPER, "export function |helper");
    assert_def!(fx, HELPER, "canvas.|total()" => MAIN, "  |total(): number");
}

#[test]
fn default_namespace_and_aliased_imports() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "strings.|shout(\"hi\")" => STRINGS, "export function |shout");
    assert_def!(fx, MAIN, "|words(\"a b\")" => STRINGS, "export function |countWords");
    assert_def!(fx, MAIN, "|whisper(\"X\")" => STRINGS, "export default function |whisper");
    // `@/*` comes from tsconfig.json.
    assert_def!(fx, MAIN, "new |Decoy()" => DECOY, "export class |Circle");
}

#[test]
fn modules_as_locations() {
    let mut fx = fx();
    let models = fx.at(INDEX, "|export * from");
    let strings = fx.at(STRINGS, "|export function shout");
    assert_eq!(fx.def(MAIN, "from \"../|models\""), vec![models]);
    assert_eq!(fx.def(MAIN, "\"@/util/|strings\""), vec![strings.clone()]);
    assert_eq!(fx.def(MAIN, "|strings.shout"), vec![strings.clone()]);
    assert_eq!(fx.def(OLD, "const |strings = require"), vec![strings]);
}

#[test]
fn names_in_import_statements() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "import { |Circle, makeCircle" => SHAPES, "export class |Circle");
    assert_def!(fx, MAIN, "{ Circle as |Decoy }" => DECOY, "export class |Circle");
    assert_def!(fx, MAIN, "{ countWords as |words }" => STRINGS, "export function |countWords");
    assert_def!(fx, MAIN, "import |whisper" => STRINGS, "export default function |whisper");
}

// ---------------------------------------------------------------------------
// Members and types
// ---------------------------------------------------------------------------

#[test]
fn classes_fields_and_getters() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "c.|area(), c.radius" => SHAPES, "  |area(): number {\n    return 3.14");
    // A parameter property.
    assert_def!(fx, MAIN, "c.|radius, c.name" => SHAPES, "public |radius");
    assert_def!(fx, MAIN, "c.|name);" => SHAPES, "  |name = \"circle\"");
    assert_def!(fx, MAIN, "grown.|diameter" => SHAPES, "get |diameter");
    assert_def!(fx, MAIN, "c.center.|x" => SHAPES, "{ |x: number");
    assert_def!(fx, MAIN, "Circle.unit().|radius" => SHAPES, "public |radius");
    // `DEFAULT = makeCircle(2)`: typed from its initializer.
    assert_def!(fx, MAIN, "DEFAULT.|area()" => SHAPES, "  |area(): number {\n    return 3.14");
    assert_def!(fx, SHAPES, "this.|radius + by" => SHAPES, "public |radius");
}

#[test]
fn inheritance_and_super() {
    let mut fx = fx();
    assert_def!(fx, SHAPES, "super.|area()" => SHAPES, "  |area(): number {\n    return 3.14");
    assert_def!(fx, SHAPES, "this.|inner * this.inner" => SHAPES, "public |inner");
    // `name` comes from the `Named` interface through `Shape`.
    assert_def!(fx, MAIN, "this.|shapes.push" => MAIN, "  |shapes: Shape[]");
}

#[test]
fn elements_promises_and_indexes() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "s.|area()" => SHAPES, "  |area(): number;");
    assert_def!(fx, MAIN, "items[i].|radius" => SHAPES, "public |radius");
    assert_def!(fx, MAIN, "item.|radius > limit" => SHAPES, "public |radius");
    // `loadCircles(): Promise<Circle[]>`, awaited.
    assert_def!(fx, MAIN, "one.|describe()" => SHAPES, "  |describe(): string");
    assert_def!(fx, MAIN, "pts[0].|y" => SHAPES, "|y: number }");
    assert_def!(fx, COLOR, "this.|colors[0]" => COLOR, "  |colors: Color[]");
}

#[test]
fn object_literals() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "config.db.|host" => MAIN, "db: { |host:");
    assert_def!(fx, MAIN, "config.|start()" => MAIN, "|start() { return 1; }");
    assert_def!(fx, MAIN, "config.|port)" => MAIN, "{ |port: 8080");
    assert_def!(fx, MAIN, "p.|x, pts" => SHAPES, "{ |x: number");
    // Keys of a literal written against a type are that type's members.
    assert_def!(fx, MAIN, "Point = { |x: 1" => SHAPES, "{ |x: number");
    assert_def!(fx, MAIN, "[{ |x: 3" => SHAPES, "{ |x: number");
    assert_def!(fx, MAIN, "{ |port: 8080" => MAIN, "{ |port: 8080");
}

#[test]
fn same_named_classes_stay_apart() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "new Decoy().|area()" => DECOY, "  |area(): number {\n    return 0");
}

#[test]
fn unknown_receivers_only_match_typescript() {
    let mut fx = fx();
    // An untyped callback parameter.
    let got = fx.def(MAIN, "shape.|area()");
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(got.iter().all(|l| l.contains(".ts:")), "{got:?}");
}

#[test]
fn overloads_unions_and_function_types() {
    let mut fx = fx();
    // Three `find` signatures are one method.
    assert_def!(fx, ADVANCED, "reg.|find(\"a\")" => ADVANCED, "  |find(name: string): Circle;");
    assert_def!(fx, ADVANCED, "found.|radius" => SHAPES, "public |radius");
    // `Circle | Promise<Circle>`, awaited.
    assert_def!(fx, ADVANCED, "loaded.|radius" => SHAPES, "public |radius");
    // A field typed `(radius: number) => Circle`.
    assert_def!(fx, ADVANCED, "built.|radius" => SHAPES, "public |radius");
    assert_def!(fx, ADVANCED, "either.|radius" => SHAPES, "public |radius");
}

#[test]
fn type_level_scopes() {
    let mut fx = fx();
    assert_def!(fx, ADVANCED, "(seed: |T)" => ADVANCED, "<|T extends Circle>");
    assert_def!(fx, ADVANCED, "? |V : never" => ADVANCED, "infer |V");
    assert_def!(fx, ADVANCED, "T[|K]" => ADVANCED, "[|K in keyof");
    assert_def!(fx, ADVANCED, "keyof |T]" => ADVANCED, "Flags<|T>");
}

#[test]
fn grammar_gaps_are_repaired() {
    let fx = fx();
    let path = fx.root.join(ADVANCED);
    assert!(!fx.ix.files[&path].tree.root_node().has_error());
}

#[test]
fn object_keys_are_not_members_of_unknown_receivers() {
    let mut fx = fx();
    assert_def!(fx, ADVANCED, "settings.headers.|accept" => ADVANCED, "{ |accept: \"json\" }");
    // `headers:` in the literal above is only a member of that literal.
    assert_def!(fx, ADVANCED, "res.|headers" => none);
    assert_def!(fx, ADVANCED, "other.|headers" => none);
}

// ---------------------------------------------------------------------------
// JSX and JavaScript
// ---------------------------------------------------------------------------

#[test]
fn jsx_components_and_destructured_props() {
    let mut fx = fx();
    // `{ circle, label }: Props`
    assert_def!(fx, VIEW, "circle.|radius" => SHAPES, "public |radius");
    assert_def!(fx, VIEW, "title={|label}" => VIEW, "{ circle, |label }");
    assert_def!(fx, VIEW, "<|Badge" => VIEW, "function |Badge");
    assert_def!(fx, VIEW, "label={|label}" => VIEW, "const |label = \"big\"");
    // Attribute names are not variables.
    assert_def!(fx, VIEW, "<Badge |circle=" => none);
    assert_def!(fx, VIEW, "{Circle.|unit()}" => SHAPES, "static |unit");
}

#[test]
fn commonjs_and_assigned_fields() {
    let mut fx = fx();
    assert_def!(fx, OLD, "c.|increment().increment()" => OLD, "  |increment() {");
    assert_def!(fx, OLD, "c.increment().|increment()" => OLD, "  |increment() {");
    assert_def!(fx, OLD, "this.|count += 1" => OLD, "this.|count = 0");
    assert_def!(fx, OLD, "c.|count;" => OLD, "this.|count = 0");
    assert_def!(fx, OLD, "strings.|countWords" => STRINGS, "export function |countWords");
    assert_def!(fx, OLD, "|shout(\"a b\")" => STRINGS, "export function |shout");
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

#[test]
fn references_across_modules() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(SHAPES, "import { |shout }"),
        fx.at(SHAPES, "return |shout(this.name)"),
        fx.at(MAIN, "strings.|shout(\"hi\")"),
        fx.at(OLD, "const { |shout }"),
        fx.at(OLD, "|shout(\"a b\")"),
    ];
    want.sort();
    assert_eq!(fx.refs(STRINGS, "export function |shout"), want);
}

#[test]
fn references_of_a_local() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(MAIN, "    |total += i"),
        fx.at(MAIN, "if (|total > limit)"),
        fx.at(MAIN, "var late = |total"),
        fx.at(MAIN, "twice(|total)"),
    ];
    want.sort();
    assert_eq!(fx.refs(MAIN, "  let |total = 0"), want);
}

#[test]
fn results_do_not_depend_on_hash_seed() {
    common::assert_seed_independent("typescript");
}

#[test]
fn snapshot_matches() {
    common::assert_snapshot("typescript");
}
