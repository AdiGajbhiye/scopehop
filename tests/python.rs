//! Resolution tests against the project in `tests/fixtures/python`. See
//! `common` for how cursors work.
//!
//! The fixture has two packages with a `Circle` class (`models.shapes` and
//! the decoy `other.shapes`), a namespace-package script (`scripts/run.py`,
//! no `__init__.py`), re-exports through `models/__init__.py`, and a Java
//! `Circle` that Python lookups must never return.

mod common;

use common::Fx;

const MAIN: &str = "app/main.py";
const OTHER: &str = "app/other.py";
const SHAPES: &str = "models/shapes.py";
const COLOR: &str = "models/color.py";
const DECOY: &str = "other/shapes.py";
const STRINGS: &str = "util/strings.py";
const RUN: &str = "scripts/run.py";

fn fx() -> Fx {
    Fx::new("python")
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

#[test]
fn function_scope_goes_to_first_binding() {
    let mut fx = fx();
    // `total += ...` and later `total` uses all go to `total = 0`.
    assert_def!(fx, MAIN, "    |total += i" => MAIN, "    |total = 0");
    assert_def!(fx, MAIN, "double(|total)\n    except" => MAIN, "    |total = 0");
    assert_def!(fx, MAIN, "i + |item" => MAIN, "for i, |item in");
    assert_def!(fx, MAIN, "|limit:\n        return size" => MAIN, "scopes(items, |limit=");
}

#[test]
fn block_statements_bind_in_the_function() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "|fh.read()" => MAIN, "as |fh:");
    assert_def!(fx, MAIN, "print(|err)" => MAIN, "as |err:");
    assert_def!(fx, MAIN, "return |size\n" => MAIN, "(|size :=");
}

#[test]
fn lambdas_and_comprehensions_have_their_own_scope() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "|v * 2" => MAIN, "lambda |v:");
    assert_def!(fx, MAIN, "[|n * n" => MAIN, "for |n in items");
    assert_def!(fx, MAIN, "if |n]" => MAIN, "for |n in items");
}

#[test]
fn global_and_nonlocal() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "    |counter += 1" => MAIN, "|counter = 0");
    assert_def!(fx, MAIN, "        |total = 1" => MAIN, "    |total = 0");
    assert_def!(fx, MAIN, "total + |limit" => MAIN, "scopes(items, |limit=");
}

#[test]
fn defaults_are_evaluated_outside_the_function() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "limit=|LIMIT" => MAIN, "|LIMIT = 10");
}

#[test]
fn class_body_is_not_visible_from_methods() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "TWICE = |LIMIT" => MAIN, "    |LIMIT = 5");
    assert_def!(fx, MAIN, "print(|LIMIT)" => MAIN, "|LIMIT = 10");
}

#[test]
fn match_captures() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "return |r\n" => MAIN, "radius=|r)");
    assert_def!(fx, MAIN, "return |px +" => MAIN, "x=|px,");
    assert_def!(fx, MAIN, "px + |pt.y" => MAIN, "as |pt:");
    assert_def!(fx, MAIN, "return first, |rest" => MAIN, "*|rest]");
}

// ---------------------------------------------------------------------------
// Imports and modules
// ---------------------------------------------------------------------------

#[test]
fn from_imports_follow_re_exports() {
    let mut fx = fx();
    // `models/__init__.py` re-exports these from `.shapes` and `.color`.
    assert_def!(fx, MAIN, "from models import |Circle" => SHAPES, "class |Circle(Shape)");
    assert_def!(fx, MAIN, "|make_circle(3)" => SHAPES, "def |make_circle");
    assert_def!(fx, MAIN, "|RED.name" => COLOR, "|RED = ");
    assert_def!(fx, MAIN, "|Colour(\"blue\")" => COLOR, "class |Color:");
}

#[test]
fn star_and_relative_imports() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "|shout(\"y\")" => STRINGS, "def |shout");
    assert_def!(fx, MAIN, "|count_words(" => STRINGS, "def |count_words");
    assert_def!(fx, MAIN, "|helper(Config())" => OTHER, "def |helper");
    assert_def!(fx, MAIN, "helper(|Config())" => OTHER, "class |Config:");
}

#[test]
fn modules_as_receivers() {
    let mut fx = fx();
    let shapes_module = fx.at(SHAPES, "|from dataclasses");
    assert_eq!(fx.def(MAIN, "|ms.make_circle"), vec![shapes_module]);
    assert_def!(fx, MAIN, "ms.|make_circle" => SHAPES, "def |make_circle");
    assert_def!(fx, MAIN, "ms.DEF|AULT" => SHAPES, "|DEFAULT = ");
    assert_def!(fx, MAIN, "util.strings.|shout" => STRINGS, "def |shout");
    assert_def!(fx, RUN, "s.|shout" => STRINGS, "def |shout");
}

#[test]
fn names_in_import_statements() {
    let mut fx = fx();
    let strings_module = fx.at(STRINGS, "|PREFIX = ");
    assert_eq!(fx.def(MAIN, "import util.|strings\n"), vec![strings_module]);
    assert_def!(fx, OTHER, "import Circle as |Round" => SHAPES, "class |Circle(Shape)");
    assert_def!(fx, RUN, "import Circle as |Decoy" => DECOY, "class |Circle:");
}

// ---------------------------------------------------------------------------
// Members and types
// ---------------------------------------------------------------------------

#[test]
fn self_and_instance_attributes() {
    let mut fx = fx();
    assert_def!(fx, SHAPES, "self.radius * self.ra|dius" => SHAPES, "self.|radius = radius");
    assert_def!(fx, MAIN, "self.circle.ra|dius" => SHAPES, "self.|radius = radius");
    assert_def!(fx, MAIN, "len(self.|points)" => MAIN, "self.|points = []");
    assert_def!(fx, MAIN, "return self.|size()" => MAIN, "def |size(self)");
}

#[test]
fn types_from_annotations_calls_and_returns() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "c.|area()" => SHAPES, "def |area(self) -> float:\n        return 3.14");
    assert_def!(fx, MAIN, "grown.|radius" => SHAPES, "self.|radius = radius");
    assert_def!(fx, MAIN, "made.|area()" => SHAPES, "def |area(self) -> float:\n        return 3.14");
    // `center` is a property returning Optional["Point"].
    assert_def!(fx, MAIN, "made.center.|y" => SHAPES, "    |y: float = 0.0");
    assert_def!(fx, MAIN, "moved(1.0).|x" => SHAPES, "    |x: float");
    assert_def!(fx, MAIN, "Circle.unit().|radius" => SHAPES, "self.|radius = radius");
    assert_def!(fx, MAIN, "ms.DEFAULT.|area()" => SHAPES, "def |area(self) -> float:\n        return 3.14");
    assert_def!(fx, MAIN, "Colour(\"blue\").|name" => COLOR, "self.|name = name");
}

#[test]
fn inheritance_and_super() {
    let mut fx = fx();
    assert_def!(fx, MAIN, "super().|run()" => MAIN, "def |run(self):\n        c =");
    assert_def!(fx, MAIN, "self.circle.|describe()" => SHAPES, "def |describe");
    assert_def!(fx, MAIN, "{self.circle.|name}" => SHAPES, "self.|name = name");
    assert_def!(fx, SHAPES, "super().|__init__" => SHAPES, "def |__init__(self, name: str)");
}

#[test]
fn keyword_arguments() {
    let mut fx = fx();
    // A dataclass's fields, `__init__`'s parameters, a method's parameters.
    assert_def!(fx, MAIN, "Point(|x=1.0" => SHAPES, "    |x: float");
    assert_def!(fx, MAIN, "Circle(|radius=2.0)" => SHAPES, "__init__(self, |radius: float");
    assert_def!(fx, MAIN, "moved(|dx=0.5)" => SHAPES, "moved(self, |dx: float)");
}

#[test]
fn same_named_classes_stay_apart() {
    let mut fx = fx();
    assert_def!(fx, RUN, "Circle(1.0).|area()" => SHAPES, "def |area(self) -> float:\n        return 3.14");
    assert_def!(fx, RUN, "Decoy().|area()" => DECOY, "def |area(self):");
    assert_def!(fx, DECOY, "self.|radius = 0" => DECOY, "self.|radius = 0");
}

#[test]
fn unknown_receivers_only_match_python() {
    let mut fx = fx();
    let got = fx.def(MAIN, "shape.|area()");
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(got.iter().all(|l| l.ends_with(".py") || l.contains(".py:")), "{got:?}");
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

#[test]
fn references_across_modules() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(SHAPES, "return |shout(self.name)"),
        fx.at(SHAPES, "import |shout"),
        fx.at(MAIN, "util.strings.|shout"),
        fx.at(MAIN, "|shout(\"y\")"),
        fx.at(RUN, "s.|shout"),
    ];
    want.sort();
    assert_eq!(fx.refs(STRINGS, "def |shout"), want);
}

#[test]
fn references_of_a_local() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(MAIN, "    |total += i"),
        fx.at(MAIN, "double(|total)\n    except"),
        fx.at(MAIN, "nonlocal |total"),
        fx.at(MAIN, "        |total = 1"),
        fx.at(MAIN, "return |total + limit"),
        fx.at(MAIN, "double(|total) + len"),
    ];
    want.sort();
    assert_eq!(fx.refs(MAIN, "    |total = 0"), want);
}

#[test]
fn results_do_not_depend_on_hash_seed() {
    common::assert_seed_independent("python");
}

#[test]
fn snapshot_matches() {
    common::assert_snapshot("python");
}
