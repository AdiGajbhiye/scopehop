//! Resolution tests against the project in `tests/fixtures/kotlin`.
//!
//! A cursor is a snippet with a `|` in it, e.g. `"c.ra|dius"`. The snippet
//! (minus the `|`) must occur exactly once in the file. Results are
//! `file:line:col` strings so failures read like source locations.

use std::path::{Path, PathBuf};

use scopehop::index::{Index, Loc};
use scopehop::text::{self, Encoding};

const MAIN: &str = "app/Main.kt";
const OTHER: &str = "app/Other.kt";
const SHAPES: &str = "model/Shapes.kt";
const EXT: &str = "model/Extensions.kt";
const HOLDERS: &str = "model/Holders.kt";
const BOXES: &str = "model/Boxes.kt";
const STRINGS: &str = "util/Strings.kt";

fn fixture_root() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kotlin")).unwrap()
}

struct Fx {
    ix: Index,
    root: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let root = fixture_root();
        let mut ix = Index::new(vec![root.clone()]);
        ix.refresh();
        Fx { ix, root }
    }

    fn cursor(&self, file: &str, snippet: &str) -> (PathBuf, usize) {
        let path = self.root.join(file);
        let text = &self.ix.files.get(&path).unwrap_or_else(|| panic!("{file} not indexed")).text;
        let caret = snippet.find('|').unwrap_or_else(|| panic!("{snippet:?} has no |"));
        let needle = snippet.replacen('|', "", 1);
        let hits: Vec<usize> = text.match_indices(&needle).map(|(i, _)| i).collect();
        assert_eq!(hits.len(), 1, "{needle:?} occurs {} times in {file}", hits.len());
        (path, hits[0] + caret)
    }

    fn show(&self, loc: &Loc) -> String {
        let fd = &self.ix.files[&loc.path];
        let p = fd.lines.position(&fd.text, loc.start, Encoding::Utf8);
        let rel = loc.path.strip_prefix(&self.root).unwrap();
        format!("{}:{}:{}", rel.display(), p.line + 1, p.character + 1)
    }

    /// Location of the word under the cursor (or the cursor itself when it
    /// is not on a word, e.g. the `{` that declares an implicit `it`).
    fn at(&self, file: &str, snippet: &str) -> String {
        let (path, off) = self.cursor(file, snippet);
        let start = text::word_at(&self.ix.files[&path].text, off).map_or(off, |(s, _)| s);
        self.show(&Loc { path, start, end: start })
    }

    fn def(&mut self, file: &str, snippet: &str) -> Vec<String> {
        let (path, off) = self.cursor(file, snippet);
        let locs = self.ix.definition_at(&path, off);
        locs.iter().map(|l| self.show(l)).collect()
    }

    /// References, without the declaration itself, sorted.
    fn refs(&mut self, file: &str, snippet: &str) -> Vec<String> {
        let (path, off) = self.cursor(file, snippet);
        let locs = self.ix.references_at(&path, off, false, false);
        let mut out: Vec<String> = locs.iter().map(|l| self.show(l)).collect();
        out.sort();
        out
    }
}

/// `def(file, cursor) == [at(file, cursor)]`, with readable failures.
macro_rules! assert_def {
    ($fx:expr, $file:expr, $use:expr => $dfile:expr, $decl:expr) => {{
        let want = vec![$fx.at($dfile, $decl)];
        assert_eq!($fx.def($file, $use), want, "definition of {:?} in {}", $use, $file);
    }};
    ($fx:expr, $file:expr, $use:expr => none) => {{
        assert_eq!($fx.def($file, $use), Vec::<String>::new(), "definition of {:?} in {}", $use, $file);
    }};
}

// ---------------------------------------------------------------------------
// Local scopes
// ---------------------------------------------------------------------------

#[test]
fn locals_shadow_members() {
    let mut fx = Fx::new();
    assert_def!(fx, MAIN, "return co|unt\n" => MAIN, "val co|unt = 10");
    assert_def!(fx, MAIN, "it + co|unt" => MAIN, "private var co|unt");
}

#[test]
fn lambda_parameters() {
    let mut fx = Fx::new();
    // An implicit `it` resolves to the lambda that declares it.
    assert_def!(fx, MAIN, "{ i|t + count" => MAIN, "map |{ it");
    assert_def!(fx, MAIN, "-> sh|ape.radius" => MAIN, "{ sh|ape ->");
}

#[test]
fn block_scoped_declarations() {
    let mut fx = Fx::new();
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
    let mut fx = Fx::new();
    // `val c = Circle(2.0)` and the `start: Circle` constructor parameter.
    assert_def!(fx, MAIN, "= c.ra|dius" => SHAPES, "class Circle(val ra|dius");
    assert_def!(fx, MAIN, "start.ar|ea" => SHAPES, "override fun ar|ea(): Double = 3.14");
    assert_def!(fx, MAIN, "this.co|unt" => MAIN, "private var co|unt");
}

#[test]
fn inherited_members() {
    let mut fx = Fx::new();
    assert_def!(fx, MAIN, "c.na|me" => SHAPES, "open class Shape(val na|me");
    // Shape's override wins over the Named interface it implements.
    assert_def!(fx, MAIN, "c.la|bel" => SHAPES, "override val la|bel");
    assert_def!(fx, MAIN, "super.ar|ea" => SHAPES, "open fun ar|ea");
}

#[test]
fn enums_companions_and_nested_types() {
    let mut fx = Fx::new();
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
    let mut fx = Fx::new();
    assert_def!(fx, MAIN, "e.mes|sage" => none);
    assert_def!(fx, MAIN, "catch (e: Ex|ception)" => none);
}

// ---------------------------------------------------------------------------
// Extension functions
// ---------------------------------------------------------------------------

#[test]
fn extension_functions() {
    let mut fx = Fx::new();
    assert_def!(fx, MAIN, "c.dia|meter" => EXT, "fun Circle.dia|meter");
    // Through a typealias: ShapeList has no members of its own.
    assert_def!(fx, MAIN, "shapes.total|Area" => EXT, "fun List<Shape>.total|Area");
    assert_def!(fx, MAIN, "shapes: Shape|List" => SHAPES, "typealias Shape|List");
}

#[test]
fn bare_names_in_extensions_are_receiver_members() {
    let mut fx = Fx::new();
    assert_def!(fx, EXT, "= ra|dius * 2" => SHAPES, "class Circle(val ra|dius");
    assert_def!(fx, EXT, "\"$na|me has" => SHAPES, "open class Shape(val na|me");
    assert_def!(fx, EXT, "${ar|ea()}" => SHAPES, "open fun ar|ea");
}

#[test]
fn extension_receiver_type_is_not_its_own_member() {
    let mut fx = Fx::new();
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
    let mut fx = Fx::new();
    assert_def!(fx, MAIN, "ye|ll(VERSION)" => STRINGS, "fun sh|out(s");
    assert_def!(fx, MAIN, "yell(VER|SION)" => STRINGS, "const val VER|SION");
    assert_def!(fx, MAIN, "val inner: In|ner" => SHAPES, "class In|ner {");
    // Star import of com.example.model, not the decoy in com.example.other.
    assert_def!(fx, MAIN, "= Cir|cle(2.0)" => SHAPES, "class Cir|cle(val radius");
}

#[test]
fn import_paths_resolve_by_qualified_name() {
    let mut fx = Fx::new();
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
    let mut fx = Fx::new();
    assert_def!(fx, MAIN, "    he|lper()" => OTHER, "fun he|lper");
    assert_def!(fx, MAIN, "= makeCir|cle(" => OTHER, "fun makeCir|cle");
}

#[test]
fn named_arguments() {
    let mut fx = Fx::new();
    assert_def!(fx, MAIN, "makeCircle(ra|dius = 3.0" => OTHER, "makeCircle(ra|dius: Double");
    assert_def!(fx, MAIN, "la|bel = \"x\"" => OTHER, "la|bel: String");
}

// ---------------------------------------------------------------------------
// Untyped receivers
// ---------------------------------------------------------------------------

#[test]
fn untyped_receiver_matches_every_member() {
    let mut fx = Fx::new();
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
    let mut fx = Fx::new();
    // Two holders say Circle, one says Square, and Square is the first
    // candidate (Boxes.kt sorts before Holders.kt).
    assert_def!(fx, MAIN, "h.shape.ra|dius" => SHAPES, "class Circle(val ra|dius");
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

#[test]
fn references_of_a_member() {
    let mut fx = Fx::new();
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
    let mut fx = Fx::new();
    assert_eq!(fx.refs(OTHER, "fun he|lper"), vec![fx.at(MAIN, "    he|lper()")]);
    assert_eq!(fx.refs(STRINGS, "fun he|lper"), Vec::<String>::new());
    let circle = fx.refs(SHAPES, "class Cir|cle(val");
    assert!(!circle.is_empty());
    assert!(circle.iter().all(|r| !r.starts_with("other/")), "decoy package leaked in: {circle:?}");
}

#[test]
fn workspace_symbols() {
    let mut fx = Fx::new();
    let names: Vec<String> = fx.ix.workspace_symbols("circ").into_iter().map(|(_, d)| d.name).collect();
    assert_eq!(names, ["Circle", "Circle", "makeCircle", "makeCircle"]);
}

// ---------------------------------------------------------------------------
// Whole-project snapshot
// ---------------------------------------------------------------------------

fn snapshot() -> String {
    let root = fixture_root();
    let mut ix = Index::new(vec![root.clone()]);
    scopehop::snapshot::snapshot(&mut ix, &root, true)
}

#[test]
fn results_do_not_depend_on_hash_seed() {
    // Each Index gets differently seeded HashMaps. Ties between candidates
    // in different files (`h.shape`, `it.area()`) would come out in a
    // different order under some of these seeds.
    let first = snapshot();
    for _ in 0..7 {
        assert!(snapshot() == first, "snapshot changed between two indexes of the same project");
    }
}

/// Every identifier in the fixture project, with its definitions and (for
/// declarations) references. Run with `UPDATE_SNAPSHOTS=1` to accept changes,
/// then review the diff of `tests/snapshots/kotlin.txt`.
#[test]
fn snapshot_matches() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/kotlin.txt");
    let got = snapshot();
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    if got != want {
        let diff: Vec<String> = want
            .lines()
            .filter(|l| !got.lines().any(|g| g == *l))
            .map(|l| format!("- {l}"))
            .chain(got.lines().filter(|l| !want.lines().any(|w| w == *l)).map(|l| format!("+ {l}")))
            .take(40)
            .collect();
        panic!(
            "snapshot changed (UPDATE_SNAPSHOTS=1 cargo test to accept):\n{}",
            diff.join("\n")
        );
    }
}
