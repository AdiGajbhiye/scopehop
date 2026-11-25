//! Outlines (blocks with their extents) and the context of a region, over
//! the fixture projects. See `common` for how cursors work.

mod common;

use std::fmt::Write;
use std::path::PathBuf;

use common::Fx;
use scopehop::index::Index;
use scopehop::outline::{self, Block, Region};
use scopehop::text::Encoding;

/// `kind name first-last` per block, lines 1-based.
fn blocks(fx: &Fx, file: &str) -> Vec<String> {
    let fd = &fx.ix.files[&fx.root.join(file)];
    let line = |o: usize| fd.lines.position(&fd.text, o, Encoding::Utf8).line + 1;
    outline::outline(fd)
        .iter()
        .map(|b: &Block| format!("{} {} {}-{}", b.kind.as_str(), b.name, line(b.start), line(b.end.saturating_sub(1).max(b.start))))
        .collect()
}

fn has(blocks: &[String], want: &str) {
    assert!(blocks.iter().any(|b| b == want), "no {want:?} in {blocks:#?}");
}

#[test]
fn nesting_names_blocks() {
    let fx = Fx::new("kotlin");
    let b = blocks(&fx, "model/Shapes.kt");
    has(&b, "class Circle 12-18");
    has(&b, "property Circle.radius 12-12");
    has(&b, "method Circle.area 13-13");
    has(&b, "class Outer.Inner 34-36");
    has(&b, "method Outer.Inner.ping 35-35");
    has(&b, "object Mode.List 40-40");
    // Extension functions are top-level functions.
    has(&blocks(&fx, "model/Extensions.kt"), "function diameter 4-4");
}

#[test]
fn go_methods_belong_to_their_receiver() {
    let fx = Fx::new("go");
    let b = blocks(&fx, "models/shapes.go");
    has(&b, "class Circle 37-41");
    has(&b, "method Circle.Area 47-49");
    has(&b, "method Shape.Area 13-13");
    // A const group's names are blocks of their own.
    has(&b, "property Red 60-60");
    has(&b, "property Green 61-61");
}

#[test]
fn python_attributes_stay_in_their_method() {
    let fx = Fx::new("python");
    let b = blocks(&fx, "models/shapes.py");
    // `self.radius = ...` is a member, but not a block: it is inside `__init__`.
    has(&b, "method Circle.__init__ 21-24");
    assert!(!b.iter().any(|b| b.contains("Circle.radius")), "{b:#?}");
    // Decorators belong to the block.
    has(&b, "property Circle.center 32-34");
}

#[test]
fn typescript_exports_and_enums() {
    let fx = Fx::new("typescript");
    let b = blocks(&fx, "src/models/color.ts");
    has(&b, "class Color 1-4");
    has(&b, "enumEntry Color.Red 2-2");
    has(&b, "enumEntry Color.Green 3-3");
    // `module.exports = { Counter, run }` declares nothing to read by block.
    let old = blocks(&fx, "src/legacy/old.js");
    assert!(!old.iter().any(|b| b.contains('@')), "{old:#?}");
}

#[test]
fn typescript_constructors() {
    let fx = Fx::new("typescript");
    let b = blocks(&fx, "src/models/shapes.ts");
    has(&b, "method Circle.constructor 18-18");
    has(&b, "method Ring.constructor 42-44");
    // A parameter property is declared in the constructor, as `this.x = ...` is.
    assert!(!b.iter().any(|b| b.contains("Circle.radius")), "{b:#?}");
}

#[test]
fn test_cases_are_blocks() {
    let fx = Fx::new("typescript");
    let b = blocks(&fx, "test/shapes.test.ts");
    assert_eq!(
        b,
        [
            "test Circle > has an area 4-6",
            "test Circle > grow > doubles the radius 9-11",
            "test parses #1 15-18",
            "function helper 22-24",
        ]
    );
    let fd = &fx.ix.files[&fx.root.join("test/shapes.test.ts")];
    let (s, e, block) = Region::Block("parses #1".into()).range(fd).unwrap();
    assert_eq!(block.unwrap().name, "parses #1");
    assert!(fd.text[s..e].starts_with("test(\"parses #1\""));
}

#[test]
fn context_of_a_test_case() {
    let mut fx = Fx::new("typescript");
    let c = context(&mut fx, "test/shapes.test.ts#Circle > grow > doubles the radius");
    assert_eq!(
        c.uses,
        [
            "Circle src/models/shapes.ts#Circle",
            "grow src/models/shapes.ts#Circle.grow",
            "radius src/models/shapes.ts#Circle.constructor",
        ]
    );
    assert_eq!(c.unresolved, ["it", "only", "expect", "toBe"]);
}

#[test]
fn csharp_extension_methods_stay_in_their_class() {
    let fx = Fx::new("csharp");
    let b = blocks(&fx, "Util/Strings.cs");
    has(&b, "class StringExt 5-18");
    has(&b, "method StringExt.Shout 7-7");
}

#[test]
fn regions_parse() {
    assert_eq!(Region::parse("src/a.ts#Ledger.post"), ("src/a.ts", Region::Block("Ledger.post".into())));
    assert_eq!(Region::parse("src/a.ts:12-30"), ("src/a.ts", Region::Lines(12, 30)));
    assert_eq!(Region::parse("src/a.ts:12"), ("src/a.ts", Region::Lines(12, 12)));
    assert_eq!(Region::parse("src/a.ts"), ("src/a.ts", Region::Whole));
    // Test names may hold `#`; paths are taken not to.
    assert_eq!(Region::parse("a.test.ts#parses #1"), ("a.test.ts", Region::Block("parses #1".into())));
    assert_eq!(Region::parse("src/a.ts:30-12"), ("src/a.ts:30-12", Region::Whole));
}

#[test]
fn blocks_are_found_by_a_unique_last_part() {
    let fx = Fx::new("kotlin");
    let blocks = outline::outline(&fx.ix.files[&fx.root.join("model/Shapes.kt")]);
    assert_eq!(outline::find(&blocks, "Circle.unit").unwrap().name, "Circle.unit");
    assert_eq!(outline::find(&blocks, "unit").unwrap().name, "Circle.unit");
    assert_eq!(outline::find(&blocks, "ping").unwrap().name, "Outer.Inner.ping");
    let err = outline::find(&blocks, "area").unwrap_err();
    assert!(err.contains("Circle.area") && err.contains("Square.area"), "{err}");
    assert!(outline::find(&blocks, "nope").is_err());
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

struct Ctx {
    uses: Vec<String>,
    used_by: Vec<String>,
    unresolved: Vec<String>,
}

/// `uses`: `name file#block`; `used_by`: `file#block names`.
fn context(fx: &mut Fx, spec: &str) -> Ctx {
    let (file, region) = Region::parse(spec);
    let path = fx.root.join(file);
    let (start, end, _) = region.range(&fx.ix.files[&path]).unwrap();
    let ctx = outline::context(&mut fx.ix, &path, start, end, true);
    let rel = |p: &PathBuf| p.strip_prefix(&fx.root).unwrap().display().to_string();
    let at = |p: &PathBuf, b: &Option<Block>| match b {
        Some(b) => format!("{}#{}", rel(p), b.name),
        None => rel(p),
    };
    Ctx {
        uses: ctx.uses.iter().map(|u| format!("{} {}", u.name, at(&u.target.path, &u.block))).collect(),
        used_by: ctx.used_by.iter().map(|c| format!("{} {}", at(&c.path, &c.block), c.names.join(","))).collect(),
        unresolved: ctx.unresolved,
    }
}

#[test]
fn context_of_a_block() {
    let mut fx = Fx::new("kotlin");
    let c = context(&mut fx, "model/Extensions.kt#diameter");
    assert_eq!(c.uses, ["Circle model/Shapes.kt#Circle", "radius model/Shapes.kt#Circle.radius"]);
    assert_eq!(c.unresolved, ["Double"]);
    assert!(c.used_by.iter().any(|u| u == "app/Main.kt#App.run diameter"), "{:#?}", c.used_by);
}

#[test]
fn context_leaves_out_what_the_region_declares() {
    let mut fx = Fx::new("kotlin");
    let c = context(&mut fx, "app/Main.kt#App.run");
    // `c` and the lambda parameters are declared in `run`.
    assert!(!c.uses.iter().any(|u| u.starts_with("c ") || u.starts_with("shape ")), "{:#?}", c.uses);
    assert!(c.uses.iter().any(|u| u == "count app/Main.kt#App.count"), "{:#?}", c.uses);
    assert!(c.uses.iter().any(|u| u == "makeCircle app/Other.kt#makeCircle"), "{:#?}", c.uses);
}

#[test]
fn callers_are_grouped_by_block() {
    let mut fx = Fx::new("kotlin");
    let c = context(&mut fx, "model/Shapes.kt#Circle");
    assert!(c.used_by.iter().any(|u| u == "model/Extensions.kt#diameter Circle,radius"), "{:#?}", c.used_by);
    assert!(c.used_by.iter().any(|u| u == "app/Main.kt#App.pick Circle"), "{:#?}", c.used_by);
    // Uses inside the class itself are not callers.
    assert!(!c.used_by.iter().any(|u| u.contains("#Circle.")), "{:#?}", c.used_by);
}

#[test]
fn context_of_lines_includes_locals_declared_before_them() {
    let mut fx = Fx::new("go");
    let text = &fx.ix.files[&fx.root.join("app/main.go")].text;
    let decl = text.lines().position(|l| l.contains("total := 0")).expect("fixture has `total := 0`") + 1;
    let usage = text.lines().skip(decl).position(|l| l.contains("total")).expect("and uses it") + decl + 1;
    let c = context(&mut fx, &format!("app/main.go:{usage}-{usage}"));
    assert!(c.uses.iter().any(|u| u.starts_with("total app/main.go#")), "{:#?}", c.uses);
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// The outline of every file in every fixture, against
/// `tests/snapshots/outline.txt`.
#[test]
fn outline_snapshot() {
    let mut out = String::new();
    for fixture in ["kotlin", "java", "mixed", "python", "go", "typescript", "csharp"] {
        let root = common::fixture_root(fixture);
        let mut ix = Index::new(vec![root.clone()]);
        ix.refresh();
        let mut paths: Vec<&PathBuf> = ix.files.keys().collect();
        paths.sort();
        for p in paths {
            let fd = &ix.files[p];
            let line = |o: usize| fd.lines.position(&fd.text, o, Encoding::Utf8).line + 1;
            let _ = writeln!(out, "{fixture}/{}", p.strip_prefix(&root).unwrap().display());
            for b in outline::outline(fd) {
                let last = line(b.end.saturating_sub(1).max(b.start));
                let _ = writeln!(out, "  {}-{} {} {}", line(b.start), last, b.kind.as_str(), b.name);
            }
        }
    }
    common::assert_text_snapshot("outline", &out);
}
