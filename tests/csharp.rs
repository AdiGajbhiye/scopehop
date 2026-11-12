//! Resolution tests against the project in `tests/fixtures/csharp`. See
//! `common` for how cursors work.

mod common;

use common::Fx;

const PROGRAM: &str = "App/Program.cs";
const CANVAS: &str = "App/Canvas.cs";
const SHAPES: &str = "Models/Shapes.cs";
const DESCRIBE: &str = "Models/Circle.Describe.cs";
const STRINGS: &str = "Util/Strings.cs";
const OTHER: &str = "Other/Shapes.cs";
const TOOLS: &str = "Other/Tools.cs";

fn fx() -> Fx {
    Fx::new("csharp")
}

// ---------------------------------------------------------------------------
// Local scopes
// ---------------------------------------------------------------------------

#[test]
fn locals_shadow_fields() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "total += |count;\n            }\n            return" => PROGRAM, "int |count = 1;");
    assert_def!(fx, PROGRAM, "total += |count;\n            }\n            catch" => PROGRAM, "private int |count = 10");
    assert_def!(fx, SHAPES, "this.|radius = radius" => SHAPES, "private double |radius;");
    assert_def!(fx, SHAPES, "this.radius = |radius" => SHAPES, "public Circle(double |radius");
}

#[test]
fn lambdas_and_local_functions() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "=> |x * 2" => PROGRAM, "twice = |x =>");
    assert_def!(fx, PROGRAM, "=> |v + total" => PROGRAM, "Local(int |v)");
    // A local function is visible before its declaration.
    assert_def!(fx, PROGRAM, "twice(|Local(total))" => PROGRAM, "int |Local(int v)");
    assert_def!(fx, PROGRAM, "|twice(Local" => PROGRAM, "Func<int, int> |twice");
}

#[test]
fn block_scoped_declarations() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "total += |i +" => PROGRAM, "for (int |i = 0");
    assert_def!(fx, PROGRAM, "|e.Message" => PROGRAM, "InvalidOperationException |e)");
    assert_def!(fx, PROGRAM, "|reader.Read()" => PROGRAM, "var |reader =");
    assert_def!(fx, CANVAS, "sum += |s.Area()" => CANVAS, "foreach (var |s in");
}

#[test]
fn pattern_and_out_variables() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "&& |circle.Radius" => PROGRAM, "Circle |circle &&");
    assert_def!(fx, PROGRAM, "(int)|circle.Area()" => PROGRAM, "Circle |circle &&");
    // An out variable in an `if` condition leaks into the block.
    assert_def!(fx, PROGRAM, "return |parsed;" => PROGRAM, "out var |parsed");
    assert_def!(fx, PROGRAM, "(int)|pt.X" => PROGRAM, "case Point |pt:");
    assert_def!(fx, PROGRAM, "=> |k.Radius" => PROGRAM, "Circle |k =>");
    assert_def!(fx, PROGRAM, "k.Ra|dius," => SHAPES, "public double Ra|dius");
}

#[test]
fn setter_value_is_implicit() {
    let mut fx = fx();
    assert_eq!(fx.def(SHAPES, "radius = va|lue"), vec![fx.at(SHAPES, "|set => radius")]);
}

#[test]
fn generic_type_parameters_and_constraints() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "|T item" => PROGRAM, "First<|T>");
    assert_def!(fx, PROGRAM, "where |T : IShape" => PROGRAM, "First<|T>");
    // `where T : IShape` makes a T an IShape.
    assert_def!(fx, PROGRAM, "|item;\n        }" => PROGRAM, "T |item =");
}

// ---------------------------------------------------------------------------
// Members, types and inheritance
// ---------------------------------------------------------------------------

#[test]
fn members_through_declared_and_var_types() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "c.Ar|ea() + c.Radius" => SHAPES, "public double Ar|ea() =>");
    assert_def!(fx, PROGRAM, "grown.Ra|dius" => SHAPES, "public double Ra|dius");
    assert_def!(fx, PROGRAM, "made.Gr|ow(" => SHAPES, "public Circle Gr|ow(");
    assert_def!(fx, PROGRAM, "Moved(0.5).|X" => SHAPES, "record Point(double |X");
    assert_def!(fx, PROGRAM, "c.Center.Moved(1).|Y" => SHAPES, "double X, double |Y)");
    // `new()` takes its type from the declaration.
    assert_def!(fx, PROGRAM, "disk.Ra|dius" => SHAPES, "public double Ra|dius");
}

#[test]
fn static_members_and_enums() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "Circle.Un|it.Area()" => SHAPES, "static readonly Circle Un|it");
    assert_def!(fx, PROGRAM, "Circle.Unit.Ar|ea()" => SHAPES, "public double Ar|ea() =>");
    assert_def!(fx, PROGRAM, "Circle.P|i)" => SHAPES, "const double P|i");
    assert_def!(fx, PROGRAM, "Color.R|ed" => SHAPES, "        R|ed,");
}

#[test]
fn partial_classes() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "c.Desc|ribe()" => DESCRIBE, "public string Desc|ribe()");
    // A bare name in one part is a member of another.
    assert_def!(fx, DESCRIBE, "{Rad|ius}" => SHAPES, "public double Rad|ius");
    // Circle's own Name beats the IShape default it implements.
    assert_def!(fx, DESCRIBE, "{Na|me}" => SHAPES, "public string Na|me => \"circle\"");
    // Go to definition picks the part that names the supertypes.
    assert_def!(fx, PROGRAM, "new Cir|cle(2)" => SHAPES, "partial class Cir|cle : IShape");
    let mut refs = fx.refs(DESCRIBE, "partial class Cir|cle");
    refs.retain(|r| r.starts_with("App/Program.cs:16:"));
    assert_eq!(refs, vec![fx.at(PROGRAM, "new Cir|cle(2)")]);
}

#[test]
fn nested_types_and_object_initializers() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "Circle.Buil|der {" => DESCRIBE, "public class Buil|der");
    assert_def!(fx, PROGRAM, "{ Si|ze = 4 }" => DESCRIBE, "public double Si|ze");
    assert_def!(fx, PROGRAM, "}.Bui|ld()" => DESCRIBE, "public Circle Bui|ld()");
    assert_def!(fx, PROGRAM, "built.Rad|ius" => SHAPES, "public double Rad|ius");
}

#[test]
fn inherited_members() {
    let mut fx = fx();
    assert_def!(fx, CANVAS, "=> Tot|al() * 2" => CANVAS, "public double Tot|al()");
    assert_def!(fx, CANVAS, "* 2 + Cou|nt;" => CANVAS, "public int Cou|nt");
}

#[test]
fn named_arguments() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "Grow(b|y: 1)" => SHAPES, "Grow(double b|y)");
    assert_def!(fx, PROGRAM, "new Circle(rad|ius: 2" => SHAPES, "public Circle(double rad|ius");
    assert_def!(fx, PROGRAM, "cen|ter: p)" => SHAPES, "Point? cen|ter = null");
}

// ---------------------------------------------------------------------------
// Namespaces and usings
// ---------------------------------------------------------------------------

#[test]
fn same_named_classes_in_other_namespaces() {
    let mut fx = fx();
    // `using Other = Shop.Other;`
    assert_def!(fx, PROGRAM, "new Other.Cir|cle" => OTHER, "public class Cir|cle");
    assert_def!(fx, PROGRAM, "{ Rad|ius = 1 }" => OTHER, "public double Rad|ius { get; set; }");
    assert_def!(fx, PROGRAM, "d.Ar|ea()" => OTHER, "public double Ar|ea()");
    // `using Disk = Shop.Models.Circle;`
    assert_def!(fx, PROGRAM, "Di|sk disk" => SHAPES, "partial class Cir|cle : IShape");
}

#[test]
fn global_and_project_usings() {
    let mut fx = fx();
    // `global using Shop.Models;` in App/Globals.cs.
    assert_def!(fx, CANVAS, "List<ISh|ape> shapes" => SHAPES, "public interface ISh|ape");
    // `<Using Include="Shop.Util.Mathx" Static="true" />` in Shop.csproj.
    assert_def!(fx, CANVAS, "Squ|are(3)" => STRINGS, "public static double Squ|are");
    // Other/ is its own project: the global using does not reach it.
    assert_def!(fx, TOOLS, "Unknown(Poi|nt p)" => none);
    assert_def!(fx, TOOLS, "Of(Cir|cle c)" => OTHER, "public class Cir|cle");
    assert_def!(fx, TOOLS, "c.Ar|ea()" => OTHER, "public double Ar|ea()");
}

#[test]
fn using_static_and_extension_methods() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "Console.WriteLine(Squ|are(2))" => STRINGS, "public static double Squ|are");
    assert_def!(fx, PROGRAM, "\"hi\".Sho|ut()" => STRINGS, "public static string Sho|ut");
    assert_def!(fx, PROGRAM, "c.Doub|led()" => STRINGS, "public static double Doub|led");
    assert_def!(fx, PROGRAM, "StringExt.CountW|ords" => STRINGS, "public static int CountW|ords");
    // Not called, so not an extension method.
    assert_def!(fx, PROGRAM, "c.Doub|led;" => none);
}

#[test]
fn preprocessor_branches() {
    let mut fx = fx();
    // `#if FANCY : System.IComparable #endif` in the middle of a class header.
    assert_def!(fx, OTHER, "=> Rad|ius;" => OTHER, "public double Rad|ius { get; set; }");
}

// ---------------------------------------------------------------------------
// Overloads and generic arity
// ---------------------------------------------------------------------------

#[test]
fn overloads_by_argument_count() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "StringExt.P|ad(\"a\").Length" => STRINGS, "static string P|ad(string s) =>");
    assert_def!(fx, PROGRAM, "StringExt.P|ad(\"a\", 2)" => STRINGS, "static string P|ad(string s, int n)");
    assert_def!(fx, PROGRAM, "StringExt.P|ad(\"a\", 1, 2)" => STRINGS, "static string P|ad(string s, params");
}

#[test]
fn types_by_generic_arity() {
    let mut fx = fx();
    assert_def!(fx, PROGRAM, "new B|ox<int>()" => SHAPES, "public struct B|ox<T>");
    assert_def!(fx, PROGRAM, "B|ox plain" => SHAPES, "public class B|ox");
    assert_def!(fx, PROGRAM, "box.P|ut(1)" => SHAPES, "public void P|ut");
    assert_def!(fx, PROGRAM, "plain.Si|ze" => SHAPES, "public int Si|ze;");
}

// ---------------------------------------------------------------------------
// Element types, LINQ and async
// ---------------------------------------------------------------------------

#[test]
fn element_types() {
    let mut fx = fx();
    assert_def!(fx, CANVAS, "sum += c.Rad|ius" => SHAPES, "public double Rad|ius");
    assert_def!(fx, CANVAS, "circles[0].Rad|ius" => SHAPES, "public double Rad|ius");
    assert_def!(fx, CANVAS, "byName[\"a\"].Rad|ius" => SHAPES, "public double Rad|ius");
    assert_def!(fx, PROGRAM, "items[i].Ar|ea()" => SHAPES, "double Ar|ea();");
}

#[test]
fn await_and_tasks() {
    let mut fx = fx();
    assert_def!(fx, CANVAS, "loaded.Rad|ius" => SHAPES, "public double Rad|ius");
    assert_def!(fx, CANVAS, "(await LoadAsync()).Ar|ea()" => SHAPES, "public double Ar|ea() =>");
}

#[test]
fn lambdas_take_elements() {
    let mut fx = fx();
    assert_def!(fx, CANVAS, "c => c.Rad|ius > 1" => SHAPES, "public double Rad|ius");
    assert_def!(fx, CANVAS, "big.Ar|ea()" => SHAPES, "public double Ar|ea() =>");
    assert_def!(fx, CANVAS, "f => f.Rad|ius" => SHAPES, "public double Rad|ius");
    assert_def!(fx, CANVAS, "found.Rad|ius" => SHAPES, "public double Rad|ius");
    assert_def!(fx, CANVAS, "?.Gr|ow(1)" => SHAPES, "public Circle Gr|ow(");
    assert_def!(fx, CANVAS, "maybe.Rad|ius" => SHAPES, "public double Rad|ius");
}

#[test]
fn generic_calls_return_their_type_argument() {
    let mut fx = fx();
    assert_def!(fx, CANVAS, "canvas.Tot|al()" => CANVAS, "public double Tot|al()");
}

// ---------------------------------------------------------------------------
// References and determinism
// ---------------------------------------------------------------------------

#[test]
fn references_of_a_member() {
    let mut fx = fx();
    let refs = fx.refs(DESCRIBE, "public Circle Bu|ild()");
    assert_eq!(refs, vec![fx.at(PROGRAM, "}.Bu|ild()")]);
    let refs = fx.refs(STRINGS, "static string Sh|out");
    assert_eq!(refs, vec![fx.at(PROGRAM, "\"hi\".Sh|out()")]);
}

#[test]
fn references_of_a_local() {
    let mut fx = fx();
    let mut want = vec![
        fx.at(PROGRAM, "WriteLine(|c.Area() + c.Radius"),
        fx.at(PROGRAM, "c.Area() + |c.Radius"),
        fx.at(PROGRAM, "c.Radius + |c.Describe()"),
        fx.at(PROGRAM, "var grown = |c.Grow(1)"),
        fx.at(PROGRAM, "Console.WriteLine(|c.Center"),
        fx.at(PROGRAM, "\"hi\".Shout() + |c.Doubled()"),
        fx.at(PROGRAM, "var group = |c.Doubled"),
        fx.at(PROGRAM, "m(|c)"),
    ];
    want.sort();
    assert_eq!(fx.refs(PROGRAM, "var |c = new Circle(2)"), want);
}

#[test]
fn results_do_not_depend_on_hash_seed() {
    common::assert_seed_independent("csharp");
}

#[test]
fn snapshot_matches() {
    common::assert_snapshot("csharp");
}
