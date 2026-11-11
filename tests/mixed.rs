//! Kotlin and Java in one project (`tests/fixtures/mixed`). Both languages
//! share the index and the package + import model, so each can see the
//! other's classes. See `common` for how cursors work.

mod common;

use common::Fx;

const APP: &str = "app/App.kt";
const BADGE: &str = "app/Badge.kt";
const GREETER: &str = "lib/Greeter.java";

fn fx() -> Fx {
    Fx::new("mixed")
}

#[test]
fn kotlin_uses_java() {
    let mut fx = fx();
    assert_def!(fx, APP, "import com.example.lib.Gree|ter" => GREETER, "class Gree|ter {");
    assert_def!(fx, APP, "= Gree|ter(\"x\")" => GREETER, "class Gree|ter {");
    assert_def!(fx, APP, "g.gre|et()" => GREETER, "String gre|et()");
    assert_def!(fx, APP, "Greeter.o|f(" => GREETER, "Greeter o|f(");
    // `of` returns Greeter, so `.greet()` is Greeter's.
    assert_def!(fx, APP, "of(\"y\").gre|et()" => GREETER, "String gre|et()");
    assert_def!(fx, APP, "Greeter.HEL|LO" => GREETER, "String HEL|LO");
}

#[test]
fn java_uses_kotlin() {
    let mut fx = fx();
    assert_def!(fx, GREETER, "import com.example.app.Bad|ge" => BADGE, "class Bad|ge(");
    assert_def!(fx, GREETER, "new Bad|ge(name)" => BADGE, "class Bad|ge(");
    assert_def!(fx, GREETER, ".lab|el()" => BADGE, "fun lab|el()");
}

#[test]
fn references_cross_languages() {
    let mut fx = fx();
    let mut want = vec![fx.at(APP, "g.gre|et()"), fx.at(APP, "of(\"y\").gre|et()")];
    want.sort();
    assert_eq!(fx.refs(GREETER, "String gre|et()"), want);
    let mut want = vec![fx.at(GREETER, ".lab|el()"), fx.at(APP, "b.lab|el()")];
    want.sort();
    assert_eq!(fx.refs(BADGE, "fun lab|el()"), want);
}

#[test]
fn results_do_not_depend_on_hash_seed() {
    common::assert_seed_independent("mixed");
}

#[test]
fn snapshot_matches() {
    common::assert_snapshot("mixed");
}
