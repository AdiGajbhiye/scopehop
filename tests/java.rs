//! Resolution tests against the project in `tests/fixtures/java`. See
//! `common` for how cursors work.

mod common;

use common::Fx;

const MAIN: &str = "app/Main.java";
const OTHER: &str = "app/Other.java";
const CIRCLE: &str = "model/Circle.java";
const SHAPE: &str = "model/Shape.java";
const NAMED: &str = "model/Named.java";
const COLOR: &str = "model/Color.java";
const POINT: &str = "model/Point.java";
const OUTER: &str = "model/Outer.java";
const STRINGS: &str = "util/Strings.java";

fn fx() -> Fx {
    Fx::new("java")
}

// ---------------------------------------------------------------------------
// Local scopes
// ---------------------------------------------------------------------------

#[test]
fn locals_shadow_fields() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "return co|unt;\n    }\n\n    static" => MAIN, "int co|unt = 10");
    assert_def!(fx, MAIN, "x + co|unt" => MAIN, "private int co|unt");
    // In Shout, `count()` is Shout's own method, not Main's field.
    assert_def!(fx, MAIN, "super.area() + co|unt()" => MAIN, "double co|unt()");
}

#[test]
fn lambda_parameters() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "println(it|em)" => MAIN, "forEach(it|em ->");
    assert_def!(fx, MAIN, "-> |x + count" => MAIN, "inc = |x ->");
    assert_def!(fx, MAIN, "-> u + |v" => MAIN, "(u, |v)");
}

#[test]
fn block_scoped_declarations() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "count += |i;" => MAIN, "int |i = 0");
    assert_def!(fx, MAIN, "|e.getMessage" => MAIN, "IOException |e)");
    assert_def!(fx, MAIN, "|reader.read()" => MAIN, "var |reader =");
    assert_def!(fx, MAIN, "Object o : |items" => MAIN, "List<Object> |items");
    assert_def!(fx, MAIN, "|o instanceof" => MAIN, "Object |o :");
}

#[test]
fn pattern_variables() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "&& |circle.radius" => MAIN, "Circle |circle &&");
    assert_def!(fx, MAIN, "|circle.area()" => MAIN, "Circle |circle &&");
    assert_def!(fx, MAIN, "circle.ra|dius > 0" => CIRCLE, "double ra|dius;");
}

#[test]
fn generic_type_parameters() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "|T item" => MAIN, "static <|T>");
    assert_def!(fx, MAIN, "List<|T> list" => MAIN, "static <|T>");
}

#[test]
fn anonymous_class_members() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "|ticks++" => MAIN, "int |ticks = 0");
}

// ---------------------------------------------------------------------------
// Members and inheritance
// ---------------------------------------------------------------------------

#[test]
fn members_through_declared_and_var_types() {
    let mut fx = fx();
    // `var c = new Circle(2.0)`
    assert_def!(fx, MAIN, "= c.ra|dius" => CIRCLE, "double ra|dius;");
    // `private final Circle start`
    assert_def!(fx, MAIN, "start.ar|ea" => CIRCLE, "double ar|ea()");
    assert_def!(fx, MAIN, "this.co|unt" => MAIN, "private int co|unt");
    assert_def!(fx, MAIN, "this.sta|rt = start" => MAIN, "Circle sta|rt;");
}

#[test]
fn inherited_members() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "c.get|Name()" => SHAPE, "String get|Name()");
    assert_def!(fx, MAIN, "c.la|bel()" => SHAPE, "String la|bel()");
    assert_def!(fx, MAIN, "super.ar|ea()" => SHAPE, "double ar|ea()");
    // A bare call to an inherited method, from a nested class.
    assert_def!(fx, MAIN, "return get|Name()" => SHAPE, "String get|Name()");
    assert_def!(fx, MAIN, "Named.DEF|AULT" => NAMED, "String DEF|AULT");
}

#[test]
fn static_members_enums_records_nested_types() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "Circle.un|it()" => CIRCLE, "Circle un|it()");
    assert_def!(fx, MAIN, "Color.R|ED" => COLOR, "R|ED(");
    assert_def!(fx, MAIN, "RED.sh|out" => COLOR, "String sh|out()");
    // Record components are fields and accessors.
    assert_def!(fx, MAIN, "p.|x()" => POINT, "int |x,");
    assert_def!(fx, MAIN, "p.su|m()" => POINT, "int su|m()");
    assert_def!(fx, MAIN, "Outer.In|ner nested" => OUTER, "class In|ner {");
    assert_def!(fx, MAIN, "new Outer.In|ner()" => OUTER, "class In|ner {");
    assert_def!(fx, MAIN, "nested.pi|ng" => OUTER, "int pi|ng()");
}

#[test]
fn expressions_with_known_types() {
    let mut fx = fx();
    // `Other.make` returns Circle, `(Circle) any` is a Circle.
    assert_def!(fx, MAIN, "((Circle) any).ra|dius" => CIRCLE, "double ra|dius;");
    assert_def!(fx, MAIN, "this::sha|dow" => MAIN, "int sha|dow()");
}

#[test]
fn receiverless_calls_use_the_return_type() {
    let mut fx = fx();
    // `area` is in four classes; only pick()'s return type narrows it.
    assert_def!(fx, MAIN, "pick().ar|ea()" => CIRCLE, "double ar|ea()");
}

#[test]
fn arrays_are_not_their_element_type() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "all.len|gth" => none);
}

#[test]
fn constructors_are_their_class() {
    let mut fx = fx();
    assert_def!(fx, CIRCLE, "public Cir|cle(double" => CIRCLE, "class Cir|cle extends");
    assert_def!(fx, MAIN, "= new Cir|cle(2.0)" => CIRCLE, "class Cir|cle extends");
}

#[test]
fn library_names_resolve_to_nothing() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "e.getMes|sage" => none);
    assert_def!(fx, MAIN, "typed.hash|Code" => none);
    assert_def!(fx, MAIN, "List<Ob|ject> items" => none);
    assert_def!(fx, MAIN, "reader.re|ad" => none);
}

// ---------------------------------------------------------------------------
// Imports and packages
// ---------------------------------------------------------------------------

#[test]
fn imports() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "    sh|out(VERSION)" => STRINGS, "String sh|out(String");
    assert_def!(fx, MAIN, "shout(VER|SION)" => STRINGS, "String VER|SION");
    assert_def!(fx, MAIN, "In|ner inner" => OUTER, "class In|ner {");
    // Star import of com.example.model, not the decoy in com.example.other.
    assert_def!(fx, MAIN, "Cir|cle typed" => CIRCLE, "class Cir|cle extends");
}

#[test]
fn import_paths_resolve_by_qualified_name() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "Strings.sh|out;" => STRINGS, "String sh|out(String");
    assert_def!(fx, MAIN, "util.Str|ings.VERSION" => STRINGS, "class Str|ings");
    assert_def!(fx, MAIN, "Outer.In|ner;" => OUTER, "class In|ner {");
    assert_def!(fx, MAIN, "import com.example.mo|del.*" => none);
    assert_def!(fx, MAIN, "package com.example.a|pp" => none);
    assert_def!(fx, MAIN, "java.util.Li|st;" => none);
}

#[test]
fn same_package_and_class_qualified_calls() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "Other.hel|per()" => OTHER, "int hel|per()");
    assert_def!(fx, MAIN, "Other.ma|ke(" => OTHER, "Circle ma|ke(");
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

#[test]
fn references_of_a_field() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(MAIN, "= c.ra|dius"),
        fx.at(MAIN, "circle.ra|dius > 0"),
        fx.at(MAIN, "((Circle) any).ra|dius"),
        fx.at(CIRCLE, "this.ra|dius = radius"),
        fx.at(CIRCLE, "3.14 * ra|dius"),
        fx.at(CIRCLE, "* radius * ra|dius"),
    ];
    want.sort();
    assert_eq!(fx.refs(CIRCLE, "double ra|dius;"), want);
}

#[test]
fn references_respect_packages() {
    let mut fx = fx();
    assert_eq!(fx.refs(OTHER, "int hel|per()"), vec![fx.at(MAIN, "Other.hel|per()")]);
    assert_eq!(fx.refs(STRINGS, "int hel|per()"), Vec::<String>::new());
    let circle = fx.refs(CIRCLE, "class Cir|cle extends");
    assert!(!circle.is_empty());
    assert!(circle.iter().all(|r| !r.starts_with("other/")), "decoy package leaked in: {circle:?}");
}

// ---------------------------------------------------------------------------
// Whole-project snapshot
// ---------------------------------------------------------------------------

#[test]
fn results_do_not_depend_on_hash_seed() {
    common::assert_seed_independent("java");
}

#[test]
fn snapshot_matches() {
    common::assert_snapshot("java");
}
