//! Resolution tests against the project in `tests/fixtures/go` (module
//! `example.com/shop`). See `common` for how cursors work.
//!
//! The fixture has two packages with a `Circle` type (`models` and the decoy
//! `other`), a package split over two files (`models/shapes.go` and
//! `models/grow.go`), an external test package, a dot import, build-tagged
//! twins (`util/fast.go`, `util/slow.go`, `util/speed_plan9.go`), and a Java
//! `Circle` that Go lookups must never return.

mod common;

use common::Fx;

const MAIN: &str = "app/main.go";
const HELPER: &str = "app/helper.go";
const TABLES: &str = "app/tables.go";
const SHAPES: &str = "models/shapes.go";
const GROW: &str = "models/grow.go";
const TEST: &str = "models/shapes_test.go";
const DECOY: &str = "other/shapes.go";
const STRINGS: &str = "util/strings.go";
const FAST: &str = "util/fast.go";
const SLOW: &str = "util/slow.go";

fn fx() -> Fx {
    Fx::new("go")
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

#[test]
fn blocks_and_shadowing() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "\t\t_ = |total" => MAIN, "\t\t|total := 1");
    assert_def!(fx, MAIN, "if |total > limit" => MAIN, "\t|total := 0\n\tfor i");
    assert_def!(fx, MAIN, "return |total\n}\n\nfunc parse" => MAIN, "\t|total := 0\n\tfor i");
}

#[test]
fn short_declarations_reuse_earlier_names() {
    let mut fx = fx();
    // `y, err := ...` reuses the `err` from the line before.
    assert_def!(fx, MAIN, "y, |err := parse" => MAIN, "x, |err := parse");
    assert_def!(fx, MAIN, "if |err != nil" => MAIN, "x, |err := parse");
    // An `if` initializer is its own scope.
    assert_def!(fx, MAIN, "Println(|err)" => MAIN, "n, |err := parse");
    assert_def!(fx, MAIN, "total += |n\n" => MAIN, "if |n, err");
}

#[test]
fn ranges_closures_switches_and_labels() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "i + int(|item" => MAIN, "for i, |item := range");
    assert_def!(fx, MAIN, "return |v * 2" => MAIN, "func(|v int)");
    assert_def!(fx, MAIN, "total = |v\n" => MAIN, "switch |v :=");
    assert_def!(fx, MAIN, "goto |done" => MAIN, "|done:");
    assert_def!(fx, MAIN, "total > |limit" => MAIN, "|limit int) int");
}

#[test]
fn the_blank_identifier_names_nothing() {
    let mut fx = fx();
    assert_eq!(fx.def(MAIN, "|_ = total"), Vec::<String>::new());
    assert_eq!(fx.def(MAIN, "for |_, s := range"), Vec::<String>::new());
}

#[test]
fn generics() {
    let mut fx = fx();
    assert_def!(fx, GROW, "items []|T" => GROW, "Box[|T any]");
    // A method's receiver declares its own type parameters.
    assert_def!(fx, GROW, "Put(item |T)" => GROW, "*Box[|T]) Put");
}

// ---------------------------------------------------------------------------
// Packages and imports
// ---------------------------------------------------------------------------

#[test]
fn package_level_names_span_files() {
    let mut fx = fx();
    assert_def!(fx, HELPER, "|counter = 0" => MAIN, "var |counter int");
    assert_def!(fx, MAIN, "|helper()" => HELPER, "func |helper");
    assert_def!(fx, GROW, "return |NewCircle(1)" => SHAPES, "func |NewCircle");
    assert_def!(fx, GROW, "(c *|Circle) Grow" => SHAPES, "type |Circle struct");
}

#[test]
fn imports_and_package_qualifiers() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "m.|NewCircle(2)" => SHAPES, "func |NewCircle");
    assert_def!(fx, MAIN, "circle *m.|Circle" => SHAPES, "type |Circle struct");
    assert_def!(fx, SHAPES, "util.|Shout" => STRINGS, "func |Shout");
    // Dot imports.
    assert_def!(fx, MAIN, "|Shout(\"hi\")" => STRINGS, "func |Shout");
    assert_def!(fx, MAIN, "|CountWords(" => STRINGS, "func |CountWords");
    // An external test package imports the package it tests.
    assert_def!(fx, TEST, "models.|NewCircle" => SHAPES, "func |NewCircle");
}

#[test]
fn packages_as_locations() {
    let mut fx = fx();
    let models = fx.at(GROW, "|package models");
    // A package's first file in the build stands for it.
    let util = fx.at(FAST, "|//go:build");
    assert_eq!(fx.def(MAIN, "|m.NewCircle(2)"), vec![models.clone()]);
    assert_eq!(fx.def(MAIN, "|m \"example.com"), vec![models.clone()]);
    assert_eq!(fx.def(MAIN, "shop/|models\""), vec![models.clone()]);
    assert_eq!(fx.def(SHAPES, "shop/|util\""), vec![util]);
    assert_eq!(fx.def(SHAPES, "package |models"), vec![models]);
    // The standard library is not in the project.
    assert_eq!(fx.def(MAIN, "fmt.|Println(c.Area"), Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// Members and types
// ---------------------------------------------------------------------------

#[test]
fn methods_and_fields() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "c.|Area(), c.Radius" => SHAPES, "(c *Circle) |Area");
    assert_def!(fx, MAIN, "c.|Radius, c.Name" => SHAPES, "\t|Radius float64");
    // A method declared in another file of the package.
    assert_def!(fx, MAIN, "c.|Grow(1)" => GROW, "(c *Circle) |Grow");
    assert_def!(fx, MAIN, "grown.|Radius" => SHAPES, "\t|Radius float64");
    assert_def!(fx, MAIN, "cv.|Add(c)" => MAIN, "(cv *Canvas) |Add");
    assert_def!(fx, MAIN, "cv.circle.|Describe()" => SHAPES, "(c *Circle) |Describe");
}

#[test]
fn embedded_fields_promote_members() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "c.|Name())" => SHAPES, "(b Base) |Name");
    assert_def!(fx, SHAPES, "c.|Name(), c.Area" => SHAPES, "(b Base) |Name");
    assert_def!(fx, SHAPES, "\t|Named\n}" => SHAPES, "type |Named interface");
}

#[test]
fn types_from_calls_literals_and_declarations() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "p.Moved(0.5).|X" => SHAPES, "\t|X, Y float64");
    assert_def!(fx, MAIN, "c.Center.Moved(1).|Y" => SHAPES, "X, |Y float64");
    // `var Default = NewCircle(1)`: typed from its initializer when used.
    assert_def!(fx, MAIN, "m.Default.|Area()" => SHAPES, "(c *Circle) |Area");
    // `Green` repeats `Red Color = iota`.
    assert_def!(fx, MAIN, "m.Green.|String()" => SHAPES, "(c Color) |String");
    assert_def!(fx, MAIN, "box.|Put(1)" => GROW, "*Box[T]) |Put");
    // Ranging over a typed parameter, including a variadic one.
    assert_def!(fx, MAIN, "sh.|Area()" => SHAPES, "\t|Area() float64");
    assert_def!(fx, MAIN, "e.|Radius" => SHAPES, "\t|Radius float64");
}

#[test]
fn composite_literal_keys() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "m.Point{|X: 1" => SHAPES, "\t|X, Y float64");
    assert_def!(fx, MAIN, "[]m.Point{{|X: 3}}" => SHAPES, "\t|X, Y float64");
    assert_def!(fx, MAIN, "&Canvas{|circle: c}" => MAIN, "\t|circle *m.Circle");
    assert_def!(fx, SHAPES, "&Circle{|Base:" => SHAPES, "\t|Base\n\tCenter");
    assert_def!(fx, SHAPES, "Base{|label:" => SHAPES, "\t|label string");
}

#[test]
fn same_named_types_stay_apart() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "d.|Area()" => DECOY, "(c Circle) |Area");
    assert_def!(fx, MAIN, "other.Circle{|Radius: 1}" => DECOY, "\t|Radius int");
}

#[test]
fn anonymous_structs() {
    let mut fx = fx();
    // A table-driven test: `tests := []struct{...}{...}`.
    assert_def!(fx, TABLES, "tt.|name" => TABLES, "\t\t|name  string");
    assert_def!(fx, TABLES, "tt.input.|X" => SHAPES, "\t|X, Y float64");
    assert_def!(fx, TABLES, "{|name: \"origin\"" => TABLES, "\t\t|name  string");
    assert_def!(fx, TABLES, "input: m.Point{|X: 0}" => SHAPES, "\t|X, Y float64");
    // Ranging over the literal itself.
    assert_def!(fx, TABLES, "row.|label" => TABLES, "struct{ |label string }");
    assert_def!(fx, TABLES, "{{|label: \"a\"}}" => TABLES, "struct{ |label string }");
}

#[test]
fn elements_of_fields_calls_indexes_and_channels() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "s.|Area()" => SHAPES, "\t|Area() float64");
    // `Rings()` returns a `map[string]Ring`, and `Ring` is a `[]m.Circle`.
    assert_def!(fx, TABLES, "ring[0].|Radius" => SHAPES, "\t|Radius float64");
    assert_def!(fx, TABLES, "rc.|Area()" => SHAPES, "(c *Circle) |Area");
    assert_def!(fx, TABLES, "first.|Name()" => SHAPES, "\t|Name() string");
    assert_def!(fx, TABLES, "got.|Radius" => SHAPES, "\t|Radius float64");
    assert_def!(fx, TABLES, "c.|Describe()" => SHAPES, "(c *Circle) |Describe");
}

#[test]
fn unknown_receivers_only_match_go() {
    let mut fx = fx();
    // A type switch alias has no type yet.
    let got = fx.def(TABLES, "v.|Area()");
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(got.iter().all(|l| l.contains(".go:")), "{got:?}");
}

#[test]
fn build_constraints_pick_one_twin() {
    let mut fx = fx();
    // `fast.go` is in a plain build; `slow.go` needs `-tags slow` and
    // `speed_plan9.go` a plan9 target.
    assert_def!(fx, TABLES, "util.|Speed()" => FAST, "func |Speed");
    assert_def!(fx, SLOW, "func |Speed" => SLOW, "func |Speed");
    // An import wins over a package-level name in another file.
    assert_eq!(fx.def(SLOW, "|strings.ToUpper"), Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

#[test]
fn references_across_packages() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(MAIN, "e.|Radius"),
        fx.at(MAIN, "int(item.|Radius)"),
        fx.at(MAIN, "c.|Radius, c.Name"),
        fx.at(MAIN, "grown.|Radius"),
        fx.at(TABLES, "ring[0].|Radius"),
        fx.at(TABLES, "got.|Radius"),
        fx.at(GROW, "c.|Radius += by"),
        fx.at(SHAPES, "|Radius: r}"),
        fx.at(SHAPES, "c.|Radius * c.Radius"),
        fx.at(SHAPES, "c.Radius * c.|Radius"),
    ];
    want.sort();
    assert_eq!(fx.refs(SHAPES, "\t|Radius float64"), want);
}

#[test]
fn references_of_a_redeclared_local() {
    let mut fx = fx();
    let mut want = vec![fx.at(MAIN, "y, |err := parse"), fx.at(MAIN, "if |err != nil")];
    want.sort();
    assert_eq!(fx.refs(MAIN, "x, |err := parse"), want);
}

#[test]
fn results_do_not_depend_on_hash_seed() {
    common::assert_seed_independent("go");
}

#[test]
fn snapshot_matches() {
    common::assert_snapshot("go");
}
