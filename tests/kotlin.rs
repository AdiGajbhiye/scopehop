//! Resolution tests against the project in `tests/fixtures/kotlin`. See
//! `common` for how cursors work.

mod common;

use common::Fx;

const MAIN: &str = "app/Main.kt";
const OTHER: &str = "app/Other.kt";
const SHAPES: &str = "model/Shapes.kt";
const EXT: &str = "model/Extensions.kt";
const HOLDERS: &str = "model/Holders.kt";
const BOXES: &str = "model/Boxes.kt";
const STRINGS: &str = "util/Strings.kt";

fn fx() -> Fx {
    Fx::new("kotlin")
}

// ---------------------------------------------------------------------------
// Local scopes
// ---------------------------------------------------------------------------

#[test]
fn locals_shadow_members() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "return co|unt\n" => MAIN, "val co|unt = 10");
    assert_def!(fx, MAIN, "it + co|unt" => MAIN, "private var co|unt");
}

#[test]
fn lambda_parameters() {
    let mut fx = fx();
    // An implicit `it` resolves to the lambda that declares it.
    assert_def!(fx, MAIN, "{ i|t + count" => MAIN, "map |{ it");
    assert_def!(fx, MAIN, "-> sh|ape.radius" => MAIN, "{ sh|ape ->");
}

#[test]
fn block_scoped_declarations() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "+= |i }" => MAIN, "for (|i in");
    assert_def!(fx, MAIN, "{ |e.message" => MAIN, "catch (|e:");
    assert_def!(fx, MAIN, "-> |s.radius" => MAIN, "val |s = made");
    assert_def!(fx, MAIN, "+ |first.toInt" => MAIN, "val (|first,");
    assert_def!(fx, MAIN, "is $|r\"" => MAIN, "val |r = c");
}

// ---------------------------------------------------------------------------
// Members and inheritance
// ---------------------------------------------------------------------------

#[test]
fn members_through_inferred_types() {
    let mut fx = fx();
    // `val c = Circle(2.0)` and the `start: Circle` constructor parameter.
    assert_def!(fx, MAIN, "= c.ra|dius" => SHAPES, "class Circle(val ra|dius");
    assert_def!(fx, MAIN, "start.ar|ea" => SHAPES, "override fun ar|ea(): Double = 3.14");
    assert_def!(fx, MAIN, "this.co|unt" => MAIN, "private var co|unt");
}

#[test]
fn receiverless_calls_use_the_return_type() {
    let mut fx = fx();
    // `area` is in four classes; only pick()'s return type narrows it.
    assert_def!(fx, MAIN, "pick().ar|ea()" => SHAPES, "override fun ar|ea(): Double = 3.14");
}

#[test]
fn inherited_members() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "c.na|me" => SHAPES, "open class Shape(val na|me");
    // Shape's override wins over the Named interface it implements.
    assert_def!(fx, MAIN, "c.la|bel" => SHAPES, "override val la|bel");
    assert_def!(fx, MAIN, "super.ar|ea" => SHAPES, "open fun ar|ea");
}

#[test]
fn enums_companions_and_nested_types() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "Color.R|ED" => SHAPES, "R|ED(");
    assert_def!(fx, MAIN, "RED.sh|out" => SHAPES, "fun sh|out(): String");
    assert_def!(fx, MAIN, "Circle.un|it()" => SHAPES, "fun un|it()");
    assert_def!(fx, MAIN, "nested: Outer.In|ner" => SHAPES, "class In|ner {");
    assert_def!(fx, MAIN, "= Outer.In|ner()" => SHAPES, "class In|ner {");
    assert_def!(fx, MAIN, "nested.pi|ng" => SHAPES, "fun pi|ng()");
    assert_def!(fx, MAIN, "Mode.Li|st" => SHAPES, "data object Li|st");
}

#[test]
fn library_members_resolve_to_nothing() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "e.mes|sage" => none);
    assert_def!(fx, MAIN, "catch (e: Ex|ception)" => none);
}

// ---------------------------------------------------------------------------
// Extension functions
// ---------------------------------------------------------------------------

#[test]
fn extension_functions() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "c.dia|meter" => EXT, "fun Circle.dia|meter");
    // Through a typealias: ShapeList has no members of its own.
    assert_def!(fx, MAIN, "shapes.total|Area" => EXT, "fun List<Shape>.total|Area");
    assert_def!(fx, MAIN, "shapes: Shape|List" => SHAPES, "typealias Shape|List");
}

#[test]
fn bare_names_in_extensions_are_receiver_members() {
    let mut fx = fx();
    assert_def!(fx, EXT, "= ra|dius * 2" => SHAPES, "class Circle(val ra|dius");
    assert_def!(fx, EXT, "\"$na|me has" => SHAPES, "open class Shape(val na|me");
    assert_def!(fx, EXT, "${ar|ea()}" => SHAPES, "open fun ar|ea");
}

#[test]
fn extension_receiver_type_is_not_its_own_member() {
    let mut fx = fx();
    // Not Mode.List: that is a nested type, and receivers don't bring
    // nested types into scope.
    assert_def!(fx, EXT, "fun Li|st<Shape>" => none);
    assert_def!(fx, EXT, "List<Sh|ape>" => SHAPES, "open class Sh|ape(");
}

// ---------------------------------------------------------------------------
// Imports and packages
// ---------------------------------------------------------------------------

#[test]
fn imports() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "ye|ll(VERSION)" => STRINGS, "fun sh|out(s");
    assert_def!(fx, MAIN, "yell(VER|SION)" => STRINGS, "const val VER|SION");
    assert_def!(fx, MAIN, "val inner: In|ner" => SHAPES, "class In|ner {");
    // Star import of com.example.model, not the decoy in com.example.other.
    assert_def!(fx, MAIN, "= Cir|cle(2.0)" => SHAPES, "class Cir|cle(val radius");
}

#[test]
fn import_paths_resolve_by_qualified_name() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "util.sh|out as" => STRINGS, "fun sh|out(s");
    assert_def!(fx, MAIN, "as ye|ll" => STRINGS, "fun sh|out(s");
    assert_def!(fx, MAIN, "model.Outer.In|ner\n" => SHAPES, "class In|ner {");
    assert_def!(fx, MAIN, "import com.example.model.Ou|ter" => SHAPES, "class Ou|ter");
    // Packages are not declarations.
    assert_def!(fx, MAIN, "import com.example.mo|del.*" => none);
    assert_def!(fx, MAIN, "package com.example.a|pp" => none);
    // Companion members are indexed as members of the class.
    assert_def!(fx, MAIN, "Companion.M|AX" => STRINGS, "const val M|AX");
    assert_def!(fx, MAIN, "= M|AX" => STRINGS, "const val M|AX");
    // The `shout` imported from util is not Color.shout.
    assert_eq!(fx.refs(SHAPES, "fun sh|out(): String"), vec![fx.at(MAIN, "RED.sh|out")]);
}

#[test]
fn same_package_beats_unimported() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "    he|lper()" => OTHER, "fun he|lper");
    assert_def!(fx, MAIN, "= makeCir|cle(" => OTHER, "fun makeCir|cle");
}

#[test]
fn named_arguments() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "makeCircle(ra|dius = 3.0" => OTHER, "makeCircle(ra|dius: Double");
    assert_def!(fx, MAIN, "la|bel = \"x\"" => OTHER, "la|bel: String");
}

// ---------------------------------------------------------------------------
// Untyped receivers
// ---------------------------------------------------------------------------

#[test]
fn untyped_receiver_matches_every_member() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(HOLDERS, "class RoundHolder(val sh|ape"),
        fx.at(HOLDERS, "BigRoundHolder(val sh|ape"),
        fx.at(BOXES, "SquareHolder(val sh|ape"),
    ];
    want.sort();
    let mut got = fx.def(MAIN, "-> h.sh|ape");
    got.sort();
    assert_eq!(got, want);
}

#[test]
fn untyped_receiver_type_is_a_majority_vote() {
    let mut fx = fx();
    // Two holders say Circle, one says Square, and Square is the first
    // candidate (Boxes.kt sorts before Holders.kt).
    assert_def!(fx, MAIN, "h.shape.ra|dius" => SHAPES, "class Circle(val ra|dius");
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

#[test]
fn references_of_a_member() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(MAIN, "= c.ra|dius"),
        fx.at(MAIN, "-> shape.ra|dius"),
        fx.at(MAIN, "s.ra|dius"),
        fx.at(MAIN, "h.shape.ra|dius"),
        fx.at(EXT, "= ra|dius * 2"),
        fx.at(SHAPES, "3.14 * ra|dius"),
        fx.at(SHAPES, "* radius * ra|dius"),
    ];
    want.sort();
    assert_eq!(fx.refs(SHAPES, "class Circle(val ra|dius"), want);
}

#[test]
fn references_respect_packages() {
    let mut fx = fx();
    assert_eq!(fx.refs(OTHER, "fun he|lper"), vec![fx.at(MAIN, "    he|lper()")]);
    assert_eq!(fx.refs(STRINGS, "fun he|lper"), Vec::<String>::new());
    let circle = fx.refs(SHAPES, "class Cir|cle(val");
    assert!(!circle.is_empty());
    assert!(circle.iter().all(|r| !r.starts_with("other/")), "decoy package leaked in: {circle:?}");
}

#[test]
fn workspace_symbols() {
    let mut fx = fx();
    let names: Vec<String> = fx.ix.workspace_symbols("circ").into_iter().map(|(_, d)| d.name).collect();
    assert_eq!(names, ["Circle", "Circle", "makeCircle", "makeCircle"]);
}

// ---------------------------------------------------------------------------
// Whole-project snapshot
// ---------------------------------------------------------------------------

#[test]
fn results_do_not_depend_on_hash_seed() {
    common::assert_seed_independent("kotlin");
}

#[test]
fn snapshot_matches() {
    common::assert_snapshot("kotlin");
}
