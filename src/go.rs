//! Go on top of tree-sitter-go.
//!
//! Go fits [`Visibility::Modules`] with one twist: a module (a Go package) is
//! a directory, so several files share one [`Summary::package`], and the
//! index treats them as one module.
//!
//! - A package is named by its import path: the `module` line of the nearest
//!   `go.mod` plus the directory under it ([`import_path`]).
//! - An import binds a package name (`models` for `example.com/shop/models`,
//!   or the alias), which is used as a receiver: `models.Circle`. Dot imports
//!   are star imports.
//! - Methods are declared outside their type (`func (c *Circle) Area()`) and
//!   are members of it, like struct fields and interface methods. Embedded
//!   fields are supertypes, so promoted members resolve.
//! - Scopes are lexical blocks, and a local is visible only after its
//!   declaration. `a, err := f()` reuses an `err` declared earlier in the
//!   same block instead of declaring a new one.
//! - Type names are kept as written (`models.Circle`), and the index reads
//!   the qualifier from the file that wrote it. Container types keep their
//!   element types (`[]models.Circle`, `map[string]Ring`) for `range`,
//!   indexing and `<-ch`. An anonymous `struct{...}` is indexed as a type
//!   named after its position (`struct@<offset>/<file length>`).
//! - Files left out of a plain build by their constraints are marked
//!   [`Summary::excluded`], so a name declared in build-tagged twins has one
//!   answer.

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::lang::{named_children, text, Def, DefKind, Expr, Import, Language, Scoped, Summary, Visibility};

pub struct Go;

fn field<'t>(n: Node<'t>, name: &str) -> Option<Node<'t>> {
    n.child_by_field_name(name)
}

fn fields<'t>(n: Node<'t>, name: &str) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.children_by_field_name(name, &mut c).collect()
}

fn is_field(n: Node, name: &str, child: Node) -> bool {
    fields(n, name).iter().any(|f| f.id() == child.id())
}

fn has_token(n: Node, tok: &str) -> bool {
    let mut c = n.walk();
    n.children(&mut c).any(|k| !k.is_named() && k.kind() == tok)
}

fn is_literal(kind: &str) -> bool {
    matches!(
        kind,
        "int_literal"
            | "float_literal"
            | "imaginary_literal"
            | "rune_literal"
            | "interpreted_string_literal"
            | "raw_string_literal"
            | "true"
            | "false"
            | "nil"
            | "iota"
            | "binary_expression"
            | "func_literal"
            | "slice_type"
            | "array_type"
            | "map_type"
            | "channel_type"
            | "function_type"
            | "struct_type"
            | "interface_type"
    )
}

fn is_function(kind: &str) -> bool {
    matches!(kind, "function_declaration" | "method_declaration" | "func_literal")
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The identifier naming the type in a type expression: `Circle` in
/// `Circle`, `*models.Circle` or `Box[int]`. `None` for unnamed types.
fn named_type_ident(t: Node) -> Option<Node> {
    match t.kind() {
        "type_identifier" => Some(t),
        "qualified_type" => field(t, "name"),
        "generic_type" => field(t, "type").and_then(named_type_ident),
        "pointer_type" | "parenthesized_type" | "type_elem" => t.named_child(0).and_then(named_type_ident),
        _ => None,
    }
}

/// The type a type expression names, as written: `Circle`, `models.Circle`
/// (pointers and type arguments dropped). Unnamed types get a placeholder
/// that no project type has, so nothing is a member of them. A result list
/// gives its first type: `(T, error)` is a `T`.
fn type_str(n: Node, src: &str) -> Option<String> {
    match n.kind() {
        "type_identifier" | "identifier" => Some(text(n, src).to_string()),
        "qualified_type" => Some(format!("{}.{}", text(field(n, "package")?, src), text(field(n, "name")?, src))),
        "pointer_type" | "parenthesized_type" | "type_elem" => type_str(n.named_child(0)?, src),
        "generic_type" => type_str(field(n, "type")?, src),
        "parameter_list" => {
            let first = named_children(n).into_iter().find(|p| p.kind() == "parameter_declaration")?;
            type_str(field(first, "type")?, src)
        }
        // Containers keep their element types for `range` and `xs[i]`
        // ([`container_element`]).
        "slice_type" | "array_type" | "implicit_length_array_type" => {
            Some(format!("[]{}", field(n, "element").and_then(|e| type_str(e, src)).unwrap_or_default()))
        }
        "map_type" => {
            let part = |f| field(n, f).and_then(|t| type_str(t, src)).unwrap_or_default();
            Some(format!("map[{}]{}", part("key"), part("value")))
        }
        "channel_type" => Some(format!("chan {}", field(n, "value").and_then(|v| type_str(v, src)).unwrap_or_default())),
        "function_type" => Some("func()".into()),
        "struct_type" => Some(anon_struct(n, src)),
        _ => None,
    }
}

/// What ranging over or indexing a container type from [`type_str`] gives:
/// the first range variable when `first` is set (a slice's index, a map's
/// key, a channel's value), the second or `xs[i]` otherwise. A channel gives
/// its value either way, which also types `<-ch`.
fn container_element(t: &str, first: bool) -> Option<String> {
    let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
    if let Some(elem) = t.strip_prefix("[]") {
        return if first { Some("int".into()) } else { some(elem) };
    }
    if let Some(elem) = t.strip_prefix("chan ") {
        return some(elem);
    }
    let rest = t.strip_prefix("map[")?;
    let mut depth = 0;
    for (i, c) in rest.char_indices() {
        match c {
            '[' => depth += 1,
            ']' if depth == 0 => return if first { some(&rest[..i]) } else { some(&rest[i + 1..]) },
            ']' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The name an anonymous `struct{...}` is indexed under. No identifier can
/// spell it, and the source length keeps two files of one package apart.
fn anon_struct(n: Node, src: &str) -> String {
    format!("struct@{}/{}", n.start_byte(), src.len())
}

/// What a container type holds: a slice's element, a map's key or value, a
/// channel's value.
fn element_type(t: Node, key: bool) -> Option<Node> {
    match t.kind() {
        "slice_type" | "array_type" | "implicit_length_array_type" => field(t, "element"),
        "map_type" => field(t, if key { "key" } else { "value" }),
        "channel_type" => field(t, "value"),
        "pointer_type" | "parenthesized_type" => element_type(t.named_child(0)?, key),
        _ => None,
    }
}

/// `v` in `for k, v := range xs`: the element type, when `xs` is a literal,
/// or a local, parameter or package-level variable of this file whose type
/// is written or is a literal.
fn range_type(name: Node, src: &str) -> Option<String> {
    let list = name.parent()?;
    let clause = list.parent()?;
    (clause.kind() == "range_clause" && is_field(clause, "left", list)).then_some(())?;
    let i = named_children(list).iter().position(|n| n.id() == name.id())?;
    let over = field(clause, "right")?;
    let (kind, t) = match over.kind() {
        // `range []T{...}`
        "composite_literal" => ("", field(over, "type")?),
        "identifier" => {
            let decl = find_in_scopes(over, text(over, src), src).or_else(|| file_var(over, text(over, src), src))?;
            let p = decl.parent()?;
            // Written (`var xs []T`), or a literal (`xs := []T{...}`).
            let t = field(p, "type").or_else(|| {
                let v = decl_initializer(decl).filter(|v| v.kind() == "composite_literal")?;
                field(v, "type")
            })?;
            (p.kind(), t)
        }
        _ => return None,
    };
    let elem = match (kind, t.kind(), i) {
        // `xs ...T` is a []T.
        ("variadic_parameter_declaration", _, 1) => t,
        ("variadic_parameter_declaration", _, _) => return None,
        (_, "map_type", _) => element_type(t, i == 0)?,
        (_, "channel_type", 0) => element_type(t, false)?,
        (_, _, 1) => element_type(t, false)?,
        _ => return None,
    };
    type_str(elem, src)
}

/// A package-level `var name` in the file of `from`. Other files of the
/// package are the index's business, but ranging over a table declared next
/// to its loop is common enough to type here.
fn file_var<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let mut root = from;
    while let Some(p) = root.parent() {
        root = p;
    }
    let decls = named_children(root).into_iter().filter(|d| d.kind() == "var_declaration");
    decls
        .flat_map(named_children)
        .flat_map(|c| if c.kind() == "var_spec_list" { named_children(c) } else { vec![c] })
        .filter(|s| s.kind() == "var_spec")
        .flat_map(|s| fields(s, "name"))
        .find(|n| text(*n, src) == name)
}

fn decl_type_name(name: Node, src: &str) -> Option<String> {
    let p = name.parent()?;
    match p.kind() {
        "type_spec" | "type_alias" if is_field(p, "name", name) => Some(text(name, src).to_string()),
        "function_declaration" | "method_declaration" | "method_elem" if is_field(p, "name", name) => {
            type_str(field(p, "result")?, src)
        }
        // `xs ...T` is a []T.
        "variadic_parameter_declaration" => Some(format!("[]{}", type_str(field(p, "type")?, src)?)),
        "var_spec" if is_field(p, "name", name) => {
            field(p, "type").and_then(|t| type_str(t, src)).or_else(|| anon_literal(name, src))
        }
        "parameter_declaration" | "const_spec" | "field_declaration" if is_field(p, "name", name) => {
            type_str(field(p, "type")?, src)
        }
        "expression_list" => range_type(name, src).or_else(|| anon_literal(name, src)),
        _ => None,
    }
}

/// `x := struct{...}{...}`: an anonymous struct has no name for type
/// inference to look up, so its type is taken from the literal here.
fn anon_literal(name: Node, src: &str) -> Option<String> {
    constructed_type(decl_initializer(name)?, src).filter(|t| t.starts_with("struct@"))
}

/// The value given to `name` in `var a, b = x, y` or `a, b := x, y`. With
/// one value for several names (`c, err := f()`), only the first name gets
/// it: the first result is the one that is usually not an error.
fn decl_initializer(name: Node) -> Option<Node> {
    let p = name.parent()?;
    let (names, values) = match p.kind() {
        "var_spec" | "const_spec" if is_field(p, "name", name) => (fields(p, "name"), field(p, "value")?),
        "expression_list" => {
            let g = p.parent()?;
            (g.kind() == "short_var_declaration" && is_field(g, "left", p)).then_some(())?;
            (named_children(p), field(g, "right")?)
        }
        _ => return None,
    };
    let values = named_children(values);
    let i = names.iter().position(|n| n.id() == name.id())?;
    if values.len() == names.len() {
        values.get(i).copied()
    } else {
        values.first().copied().filter(|_| i == 0)
    }
}

/// The type of `T{...}`, `&T{...}` or `new(T)`, without resolving anything.
fn constructed_type(value: Node, src: &str) -> Option<String> {
    match value.kind() {
        "composite_literal" => type_str(field(value, "type")?, src),
        "unary_expression" => constructed_type(field(value, "operand")?, src),
        "call_expression" => {
            let f = field(value, "function")?;
            (f.kind() == "identifier" && text(f, src) == "new").then_some(())?;
            type_str(field(value, "arguments")?.named_child(0)?, src)
        }
        _ => None,
    }
}

/// The type a composite literal's `{...}` builds, including elided ones:
/// in `[]Point{{X: 1}}` the inner `{X: 1}` is a `Point`.
fn literal_type(body: Node) -> Option<Node> {
    let p = body.parent()?;
    match p.kind() {
        "composite_literal" => field(p, "type"),
        "literal_element" => {
            let outer = p.parent()?;
            let (list, key) = match outer.kind() {
                "keyed_element" => (outer.parent()?, is_field(outer, "key", p)),
                _ => (outer, false),
            };
            element_type(literal_type(list)?, key)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

/// Names a statement declares in the scope it is in.
fn declared_by<'t>(stmt: Node<'t>, out: &mut Vec<Node<'t>>) {
    let left = |out: &mut Vec<Node<'t>>| {
        let names = field(stmt, "left").map(named_children).unwrap_or_default();
        out.extend(names.into_iter().filter(|n| n.kind() == "identifier"));
    };
    match stmt.kind() {
        "short_var_declaration" => left(out),
        "range_clause" | "receive_statement" if has_token(stmt, ":=") => left(out),
        "var_declaration" | "const_declaration" | "type_declaration" | "var_spec_list" => {
            for c in named_children(stmt) {
                declared_by(c, out);
            }
        }
        "var_spec" | "const_spec" => out.extend(fields(stmt, "name")),
        "type_spec" | "type_alias" => out.extend(field(stmt, "name")),
        "labeled_statement" => {
            for c in named_children(stmt).into_iter().filter(|c| c.kind() != "label_name") {
                declared_by(c, out);
            }
        }
        "for_clause" => {
            if let Some(i) = field(stmt, "initializer") {
                declared_by(i, out);
            }
        }
        _ => {}
    }
}

fn param_names(list: Option<Node>) -> Vec<Node> {
    let Some(list) = list else { return Vec::new() };
    named_children(list)
        .into_iter()
        .filter(|p| matches!(p.kind(), "parameter_declaration" | "variadic_parameter_declaration"))
        .flat_map(|p| fields(p, "name"))
        .collect()
}

fn type_param_names(decl: Node) -> Vec<Node> {
    let Some(list) = field(decl, "type_parameters") else { return Vec::new() };
    named_children(list)
        .into_iter()
        .filter(|p| p.kind() == "type_parameter_declaration")
        .flat_map(|p| fields(p, "name"))
        .collect()
}

/// `T` in `func (b *Box[T]) Get() T`: the receiver declares it.
fn receiver_type_params(method: Node) -> Vec<Node> {
    let mut out = Vec::new();
    if method.kind() != "method_declaration" {
        return out;
    }
    let recv = field(method, "receiver").and_then(|r| r.named_child(0));
    let mut t = recv.and_then(|p| field(p, "type"));
    while let Some(n) = t.filter(|n| matches!(n.kind(), "pointer_type" | "parenthesized_type")) {
        t = n.named_child(0);
    }
    let args = t.filter(|n| n.kind() == "generic_type").and_then(|g| field(g, "type_arguments"));
    for arg in args.map(named_children).unwrap_or_default() {
        out.extend(arg.named_child(0).filter(|n| n.kind() == "type_identifier"));
    }
    out
}

/// For a name being declared with `:=` (or `range`, or `case v := <-ch`),
/// the scope it lands in.
fn short_decl_scope(from: Node) -> Option<Node> {
    if from.kind() != "identifier" {
        return None;
    }
    let list = from.parent()?;
    let stmt = list.parent()?;
    if !is_field(stmt, "left", list) {
        return None;
    }
    match stmt.kind() {
        "short_var_declaration" => {}
        "range_clause" | "receive_statement" if has_token(stmt, ":=") => {}
        _ => return None,
    }
    let mut scope = stmt.parent()?;
    while scope.kind() == "labeled_statement" {
        scope = scope.parent()?;
    }
    Some(scope)
}

fn find_label<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let mut f = from.parent();
    while let Some(n) = f.filter(|n| !is_function(n.kind())) {
        f = n.parent();
    }
    let mut stack = vec![field(f?, "body")?];
    while let Some(n) = stack.pop() {
        if n.kind() == "labeled_statement"
            && let Some(l) = field(n, "label").filter(|l| text(*l, src) == name)
        {
            return Some(l);
        }
        stack.extend(named_children(n).into_iter().filter(|c| c.kind() != "func_literal"));
    }
    None
}

fn find_in_scopes<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    if name == "_" {
        return None;
    }
    if from.kind() == "label_name" {
        return find_label(from, name, src);
    }
    let pos = from.start_byte();
    let short = short_decl_scope(from);
    let before = |c: Node| c.end_byte() <= pos;
    let mut prev = from;
    let mut cur = from.parent();
    while let Some(s) = cur {
        let mut decls = Vec::new();
        match s.kind() {
            "source_file" => return None,
            "statement_list" => {
                for c in named_children(s) {
                    // A local type is in scope inside its own declaration.
                    if before(c) || (c.kind() == "type_declaration" && c.start_byte() <= pos) {
                        declared_by(c, &mut decls);
                    }
                }
            }
            "if_statement" | "expression_switch_statement" | "type_switch_statement" | "for_clause" => {
                decls_if_before(field(s, "initializer"), pos, &mut decls);
                // `switch v := x.(type)`: `v` exists in the cases only.
                if s.kind() == "type_switch_statement" && matches!(prev.kind(), "type_case" | "default_case") {
                    decls.extend(field(s, "alias").map(named_children).unwrap_or_default());
                }
            }
            "for_statement" => {
                let clause = named_children(s).into_iter().find(|c| matches!(c.kind(), "for_clause" | "range_clause"));
                decls_if_before(clause, pos, &mut decls);
            }
            "communication_case" => decls_if_before(field(s, "communication"), pos, &mut decls),
            k if is_function(k) => {
                decls.extend(type_param_names(s));
                decls.extend(receiver_type_params(s));
                // Parameters exist in the body, not in the signature.
                if field(s, "body").is_some_and(|b| b.id() == prev.id()) {
                    decls.extend(param_names(field(s, "receiver")));
                    decls.extend(param_names(field(s, "parameters")));
                    decls.extend(param_names(field(s, "result").filter(|r| r.kind() == "parameter_list")));
                }
            }
            "type_spec" | "type_alias" => decls.extend(type_param_names(s)),
            _ => {}
        }
        if let Some(d) = decls.into_iter().find(|n| text(*n, src) == name) {
            return Some(d);
        }
        if short.is_some_and(|sc| sc.id() == s.id()) {
            return Some(from);
        }
        prev = s;
        cur = s.parent();
    }
    None
}

fn decls_if_before<'t>(stmt: Option<Node<'t>>, pos: usize, out: &mut Vec<Node<'t>>) {
    if let Some(st) = stmt.filter(|st| st.end_byte() <= pos) {
        declared_by(st, out);
    }
}

fn is_declaration_name(ident: Node) -> bool {
    let Some(p) = ident.parent() else { return false };
    match p.kind() {
        "function_declaration" | "method_declaration" | "type_spec" | "type_alias" | "method_elem" | "var_spec"
        | "const_spec" | "parameter_declaration" | "variadic_parameter_declaration" | "field_declaration"
        | "type_parameter_declaration" => is_field(p, "name", ident),
        "labeled_statement" => is_field(p, "label", ident),
        "expression_list" => p
            .parent()
            .is_some_and(|g| g.kind() == "type_switch_statement" && is_field(g, "alias", p)),
        "type_elem" => {
            let mut m = Some(p);
            while let Some(n) = m.filter(|n| n.kind() != "method_declaration" && n.kind() != "source_file") {
                m = n.parent();
            }
            m.is_some_and(|m| receiver_type_params(m).iter().any(|t| t.id() == ident.id()))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

fn summarize(root: Node, src: &str) -> Summary {
    let mut s = Summary::default();
    for n in named_children(root) {
        match n.kind() {
            "package_clause" => {
                s.package = n.named_child(0).map(|p| text(p, src).to_string()).unwrap_or_default();
            }
            "import_declaration" => collect_imports(n, src, &mut s.imports),
            "function_declaration" => {
                if let Some(name) = field(n, "name") {
                    s.defs.push(def(name, src, DefKind::Function, None, decl_type_name(name, src)));
                }
            }
            "method_declaration" => {
                let recv = field(n, "receiver")
                    .and_then(|r| r.named_child(0))
                    .and_then(|p| field(p, "type"))
                    .and_then(named_type_ident)
                    .map(|t| text(t, src));
                if let Some(name) = field(n, "name") {
                    s.defs.push(def(name, src, DefKind::Function, recv, decl_type_name(name, src)));
                }
            }
            "type_declaration" => {
                for spec in named_children(n) {
                    type_defs(spec, src, &mut s.defs);
                }
            }
            "var_declaration" | "const_declaration" => value_defs(n, src, &mut s.defs),
            _ => {}
        }
    }
    anon_struct_defs(root, src, &mut s.defs);
    s.excluded = !build_line_ok(src);
    s
}

fn def(name: Node, src: &str, kind: DefKind, container: Option<&str>, type_name: Option<String>) -> Def {
    Def {
        name: text(name, src).to_string(),
        kind,
        start: name.start_byte(),
        end: name.end_byte(),
        container: container.map(str::to_string),
        type_name,
        supertypes: Vec::new(),
    }
}

/// Top-level `var` and `const` names. In a `const (...)` group a spec with
/// neither type nor value repeats the one before it (`B` after `A Color =
/// iota`), so it has that type too.
fn value_defs(decl: Node, src: &str, out: &mut Vec<Def>) {
    let mut specs = Vec::new();
    for c in named_children(decl) {
        match c.kind() {
            "var_spec_list" => specs.extend(named_children(c)),
            _ => specs.push(c),
        }
    }
    let mut last_type = None;
    for spec in specs.into_iter().filter(|s| matches!(s.kind(), "var_spec" | "const_spec")) {
        let written = field(spec, "type").and_then(|t| type_str(t, src));
        if written.is_some() || field(spec, "value").is_some() {
            last_type = written.clone();
        }
        for name in fields(spec, "name").into_iter().filter(|n| text(*n, src) != "_") {
            let ty = written
                .clone()
                .or_else(|| decl_initializer(name).and_then(|v| constructed_type(v, src)))
                .or_else(|| last_type.clone().filter(|_| field(spec, "value").is_none()));
            out.push(def(name, src, DefKind::Property, None, ty));
        }
    }
}

/// A named type, then its members: struct fields (an embedded field is
/// named after its type, and its type is a supertype) or interface methods
/// (an embedded interface is a supertype).
fn type_defs(spec: Node, src: &str, out: &mut Vec<Def>) {
    if !matches!(spec.kind(), "type_spec" | "type_alias") {
        return;
    }
    let Some(name) = field(spec, "name") else { return };
    let cname = text(name, src);
    let body = field(spec, "type");
    let mut supers = Vec::new();
    let mut members = Vec::new();
    if spec.kind() == "type_alias" || body.is_some_and(|b| !matches!(b.kind(), "struct_type" | "interface_type")) {
        // `type A = B`: A has B's members. `type Routes []Route` ranges like
        // a `[]Route`.
        supers.extend(body.and_then(|b| type_str(b, src)));
    }
    match body.map(|b| (b.kind(), b)) {
        Some(("struct_type", b)) => struct_members(b, cname, src, &mut supers, &mut members),
        Some(("interface_type", b)) => {
            for m in named_children(b) {
                match m.kind() {
                    "method_elem" => {
                        if let Some(n) = field(m, "name") {
                            members.push(def(n, src, DefKind::Function, Some(cname), decl_type_name(n, src)));
                        }
                    }
                    "type_elem" => {
                        let only = named_children(m);
                        if let [t] = only.as_slice()
                            && matches!(t.kind(), "type_identifier" | "qualified_type")
                        {
                            supers.extend(type_str(*t, src));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let mut class = def(name, src, DefKind::Class, None, Some(cname.to_string()));
    class.supertypes = supers;
    out.push(class);
    out.extend(members);
}

/// A struct's fields as members of `cname`. An embedded field is named
/// after its type, and its type is a supertype.
fn struct_members(b: Node, cname: &str, src: &str, supers: &mut Vec<String>, members: &mut Vec<Def>) {
    let list = named_children(b).into_iter().find(|c| c.kind() == "field_declaration_list");
    for f in list.map(named_children).unwrap_or_default() {
        if f.kind() != "field_declaration" {
            continue;
        }
        let Some(t) = field(f, "type") else { continue };
        let ty = type_str(t, src);
        let names = fields(f, "name");
        if names.is_empty()
            && let Some(ident) = named_type_ident(t)
        {
            supers.extend(ty.clone());
            members.push(def(ident, src, DefKind::Property, Some(cname), ty.clone()));
        }
        for n in names.into_iter().filter(|n| text(*n, src) != "_") {
            members.push(def(n, src, DefKind::Property, Some(cname), ty.clone()));
        }
    }
}

/// Every `struct{...}` that is not the body of a named type, as a class of
/// its own (see [`anon_struct`]), so `tt.name` in a table-driven test finds
/// the field. The class's location is the `struct` keyword.
fn anon_struct_defs(root: Node, src: &str, out: &mut Vec<Def>) {
    let mut c = root.walk();
    'walk: loop {
        let n = c.node();
        // `parent()` walks down from the root, so ask it only of structs.
        if n.kind() == "struct_type" && !n.parent().is_some_and(|p| matches!(p.kind(), "type_spec" | "type_alias")) {
            let cname = anon_struct(n, src);
            let mut class = Def {
                name: cname.clone(),
                kind: DefKind::Class,
                start: n.start_byte(),
                end: n.start_byte() + "struct".len(),
                container: None,
                type_name: Some(cname.clone()),
                supertypes: Vec::new(),
            };
            let mut members = Vec::new();
            struct_members(n, &cname, src, &mut class.supertypes, &mut members);
            out.push(class);
            out.extend(members);
        }
        if c.goto_first_child() {
            continue;
        }
        while !c.goto_next_sibling() {
            if !c.goto_parent() {
                break 'walk;
            }
        }
    }
}

/// The fields a keyed literal `T{Name: ...}` can name, for the type's
/// `type_spec` or an anonymous `struct_type`.
fn struct_fields(decl: Node) -> Vec<Node> {
    let body = if decl.kind() == "struct_type" { Some(decl) } else { field(decl, "type") };
    let Some(b) = body.filter(|b| b.kind() == "struct_type") else { return Vec::new() };
    let list = named_children(b).into_iter().find(|c| c.kind() == "field_declaration_list");
    let mut out = Vec::new();
    for f in list.map(named_children).unwrap_or_default() {
        let names = fields(f, "name");
        if names.is_empty() {
            out.extend(field(f, "type").and_then(named_type_ident));
        }
        out.extend(names);
    }
    out
}

// ---------------------------------------------------------------------------
// Build constraints
// ---------------------------------------------------------------------------

const GOOS: &[&str] = &[
    "aix", "android", "darwin", "dragonfly", "freebsd", "hurd", "illumos", "ios", "js", "linux", "nacl", "netbsd",
    "openbsd", "plan9", "solaris", "wasip1", "windows", "zos",
];
const GOARCH: &[&str] = &[
    "386", "amd64", "amd64p32", "arm", "armbe", "arm64", "arm64be", "loong64", "mips", "mipsle", "mips64",
    "mips64le", "mips64p32", "mips64p32le", "ppc", "ppc64", "ppc64le", "riscv", "riscv64", "s390", "s390x",
    "sparc", "sparc64", "wasm",
];
const UNIX: &[&str] = &[
    "aix", "android", "darwin", "dragonfly", "freebsd", "hurd", "illumos", "ios", "linux", "netbsd", "openbsd",
    "solaris",
];

/// The platform files are read for, like `go build` with no flags: this
/// machine's OS and architecture.
fn host() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "powerpc64" => "ppc64",
        "loongarch64" => "loong64",
        arch => arch,
    };
    (os, arch)
}

/// Whether a build tag holds with no `-tags`: the host's OS and
/// architecture, `unix` on Unix systems, the gc toolchain with cgo, and
/// every `go1.N` release. Custom tags (`nomsgpack`, `ignore`) do not.
fn tag_ok(tag: &str, (os, arch): (&str, &str)) -> bool {
    tag == os
        || tag == arch
        || (tag == "unix" && UNIX.contains(&os))
        || matches!(tag, "gc" | "cgo")
        || tag.strip_prefix("go1.").is_some_and(|v| v.chars().all(|c| c.is_ascii_digit()))
}

/// Evaluates a `//go:build` expression (`linux && !nomsgpack`). A line that
/// does not parse counts as satisfied.
pub fn build_expr_ok(expr: &str, platform: (&str, &str)) -> bool {
    let mut toks = Vec::new();
    let mut rest = expr.trim();
    while !rest.is_empty() {
        let n = if rest.starts_with("&&") || rest.starts_with("||") {
            2
        } else if rest.starts_with(['!', '(', ')']) {
            1
        } else {
            rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).unwrap_or(rest.len()).max(1)
        };
        toks.push(&rest[..n]);
        rest = rest[n..].trim_start();
    }
    // or := and ('||' and)*; and := not ('&&' not)*; not := '!' not | atom
    fn or(t: &[&str], i: &mut usize, p: (&str, &str)) -> Option<bool> {
        let mut v = and(t, i, p)?;
        while t.get(*i) == Some(&"||") {
            *i += 1;
            v |= and(t, i, p)?;
        }
        Some(v)
    }
    fn and(t: &[&str], i: &mut usize, p: (&str, &str)) -> Option<bool> {
        let mut v = not(t, i, p)?;
        while t.get(*i) == Some(&"&&") {
            *i += 1;
            v &= not(t, i, p)?;
        }
        Some(v)
    }
    fn not(t: &[&str], i: &mut usize, p: (&str, &str)) -> Option<bool> {
        let tok = *t.get(*i)?;
        *i += 1;
        match tok {
            "!" => not(t, i, p).map(|v| !v),
            "(" => {
                let v = or(t, i, p)?;
                (t.get(*i) == Some(&")")).then(|| *i += 1)?;
                Some(v)
            }
            "&&" | "||" | ")" => None,
            tag => Some(tag_ok(tag, p)),
        }
    }
    let mut i = 0;
    match or(&toks, &mut i, platform) {
        Some(v) if i == toks.len() => v,
        _ => true,
    }
}

/// The file's `//go:build` line, which sits above the package clause.
fn build_line_ok(src: &str) -> bool {
    src.lines()
        .map(str::trim)
        .take_while(|l| !l.starts_with("package "))
        .find_map(|l| l.strip_prefix("//go:build "))
        .is_none_or(|e| build_expr_ok(e, host()))
}

/// Constraints in a file name: `x_windows.go`, `x_linux_arm64_test.go`.
pub fn file_name_ok(path: &Path) -> bool {
    file_name_ok_for(path, host())
}

fn file_name_ok_for(path: &Path, (os, arch): (&str, &str)) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { return true };
    let stem = stem.strip_suffix("_test").unwrap_or(stem);
    let parts: Vec<&str> = stem.split('_').skip(1).collect();
    match parts.as_slice() {
        [.., a, b] if GOOS.contains(a) && GOARCH.contains(b) => *a == os && *b == arch,
        [.., a] if GOOS.contains(a) => *a == os,
        [.., a] if GOARCH.contains(a) => *a == arch,
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Packages and imports
// ---------------------------------------------------------------------------

fn import_path_text(spec: Node, src: &str) -> Option<String> {
    let p = field(spec, "path")?;
    Some(text(p, src).trim_matches(|c| c == '"' || c == '`').to_string())
}

fn collect_imports(decl: Node, src: &str, out: &mut Vec<Import>) {
    let mut specs = Vec::new();
    for c in named_children(decl) {
        match c.kind() {
            "import_spec_list" => specs.extend(named_children(c)),
            _ => specs.push(c),
        }
    }
    for spec in specs.into_iter().filter(|s| s.kind() == "import_spec") {
        let Some(path) = import_path_text(spec, src) else { continue };
        match field(spec, "name") {
            Some(n) if n.kind() == "blank_identifier" => {}
            Some(n) if n.kind() == "dot" => out.push(Import { path, star: true, alias: None }),
            Some(n) => out.push(Import { path, star: false, alias: Some(text(n, src).to_string()) }),
            None => {
                let alias = Some(package_name(&path));
                out.push(Import { path, star: false, alias });
            }
        }
    }
}

/// The name an import path binds when the import has no alias: the last
/// element, skipping a major-version suffix (`.../v2`, `yaml.v3`) and the
/// usual `go-` prefix or `-go` suffix of a repository name. The real name
/// is in the imported package's `package` clause, which may differ.
pub fn package_name(path: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').collect();
    let mut last = parts.pop().unwrap_or(path);
    let is_version = |s: &str| s.len() > 1 && s.starts_with('v') && s[1..].bytes().all(|b| b.is_ascii_digit());
    if is_version(last) {
        last = parts.pop().unwrap_or(last);
    }
    let last = last.split('.').next().unwrap_or(last);
    let last = last.strip_prefix("go-").unwrap_or(last);
    let last = last.strip_suffix("-go").unwrap_or(last);
    last.replace('-', "_")
}

/// The import path of the package a file is in: the `module` path of the
/// nearest `go.mod` plus the file's directory under it (`vendor/x/y` is
/// `x/y`). Without a `go.mod`, the directory under the workspace root, or
/// the root's own name for files directly in it.
pub fn import_path(file: &Path, roots: &[PathBuf]) -> String {
    let Some(dir) = file.parent() else { return String::new() };
    let parts = |base: &Path| -> Vec<String> {
        dir.strip_prefix(base)
            .map(|r| r.components().filter_map(|c| c.as_os_str().to_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if let Some(module) = std::fs::read_to_string(d.join("go.mod")).ok().as_deref().and_then(module_line) {
            let rel = parts(d);
            if rel.first().is_some_and(|f| f == "vendor") {
                return rel[1..].join("/");
            }
            return std::iter::once(module).chain(rel).collect::<Vec<_>>().join("/");
        }
        cur = d.parent();
    }
    let dir_name = |d: &Path| d.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    match roots.iter().filter(|r| file.starts_with(r)).max_by_key(|r| r.components().count()) {
        Some(root) => {
            let rel = parts(root);
            if rel.is_empty() { dir_name(root) } else { rel.join("/") }
        }
        None => dir_name(dir),
    }
}

fn module_line(gomod: &str) -> Option<String> {
    gomod.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("module")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let m = rest.split("//").next()?.trim().trim_matches('"');
        (!m.is_empty()).then(|| m.to_string())
    })
}

/// For the package name or path in an import spec, the imported path. For
/// the name in the package clause, `""`: the file's own package (see
/// `absolute_import`).
fn qualified_path(n: Node, src: &str) -> Option<String> {
    let mut p = n.parent()?;
    if p.kind() == "package_clause" {
        return Some(String::new());
    }
    if p.kind() == "interpreted_string_literal" {
        p = p.parent()?;
        return (p.kind() == "import_spec").then(|| import_path_text(p, src)).flatten();
    }
    (p.kind() == "import_spec" && is_field(p, "name", n)).then(|| import_path_text(p, src)).flatten()
}

// ---------------------------------------------------------------------------
// The Language impl
// ---------------------------------------------------------------------------

impl Language for Go {
    fn name(&self) -> &'static str {
        "Go"
    }

    fn grammar(&self) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn family(&self) -> &'static str {
        "go"
    }

    fn visibility(&self) -> Visibility {
        Visibility::Modules
    }

    fn summarize(&self, root: Node, src: &str) -> Summary {
        summarize(root, src)
    }

    /// The package becomes its import path. An external test package
    /// (`package models_test`) is a module of its own next to `models`.
    fn place(&self, summary: &mut Summary, path: &Path, roots: &[PathBuf]) {
        let test = summary.package.ends_with("_test");
        summary.excluded |= !file_name_ok(path);
        summary.package = import_path(path, roots);
        if test {
            summary.package.push_str("_test");
        }
    }

    fn absolute_import(&self, path: &str, _file: &Path, summary: &Summary) -> String {
        if path.is_empty() { summary.package.clone() } else { path.to_string() }
    }

    fn name_node<'t>(&self, root: Node<'t>, src: &str, start: usize, end: usize) -> Option<Node<'t>> {
        let node = root.descendant_for_byte_range(start, end)?;
        // The blank identifier names nothing.
        if text(node, src) == "_" {
            return None;
        }
        if self.is_identifier(node) && node.start_byte() == start && node.end_byte() == end {
            return Some(node);
        }
        // The package name at the end of an import path: `"a/b/models"`.
        let in_import = node.kind() == "interpreted_string_literal_content"
            && node.parent().and_then(|p| p.parent()).is_some_and(|s| s.kind() == "import_spec");
        let last_part = end == node.end_byte()
            && (start == node.start_byte() || src.as_bytes().get(start.wrapping_sub(1)) == Some(&b'/'));
        (in_import && last_part).then_some(node)
    }

    fn is_identifier(&self, n: Node) -> bool {
        matches!(
            n.kind(),
            "identifier" | "field_identifier" | "type_identifier" | "package_identifier" | "label_name"
        )
    }

    fn is_declaration_name(&self, ident: Node) -> bool {
        is_declaration_name(ident)
    }

    /// `Radius` in `Circle{Radius: 2}`, or in an elided `{Radius: 2}` inside
    /// `[]Circle{...}`. The "call" is the literal's type. Keys of map
    /// literals are ordinary expressions.
    fn named_argument_call<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        let key = ident.parent()?;
        let elem = key.parent()?;
        (ident.kind() == "identifier" && key.kind() == "literal_element" && elem.kind() == "keyed_element")
            .then_some(())?;
        is_field(elem, "key", key).then_some(())?;
        let t = literal_type(elem.parent()?)?;
        (named_type_ident(t).is_some() || t.kind() == "struct_type").then_some(t)
    }

    fn member_receiver<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        let p = ident.parent()?;
        match p.kind() {
            "selector_expression" if is_field(p, "field", ident) => field(p, "operand"),
            "qualified_type" if is_field(p, "name", ident) => field(p, "package"),
            _ => None,
        }
    }

    fn type_qualifier<'t>(&self, _ident: Node<'t>) -> Option<Node<'t>> {
        None
    }

    fn in_type_position(&self, _n: Node) -> bool {
        false
    }

    fn qualified_path(&self, ident: Node, src: &str) -> Option<String> {
        qualified_path(ident, src)
    }

    fn find_in_scopes<'t>(&self, from: Node<'t>, name: &str, src: &str) -> Option<Scoped<'t>> {
        find_in_scopes(from, name, src).map(Scoped::Decl)
    }

    fn decl_type_name(&self, name: Node, src: &str) -> Option<String> {
        decl_type_name(name, src)
    }

    fn decl_initializer<'t>(&self, name: Node<'t>) -> Option<Node<'t>> {
        decl_initializer(name)
    }

    fn range_element<'t>(&self, name: Node<'t>) -> Option<(Node<'t>, bool)> {
        let list = name.parent()?;
        let clause = list.parent()?;
        (clause.kind() == "range_clause" && is_field(clause, "left", list)).then_some(())?;
        let i = named_children(list).iter().position(|n| n.id() == name.id())?;
        (i < 2).then(|| field(clause, "right").map(|r| (r, i == 0)))?
    }

    fn element_type(&self, container: &str, first: bool) -> Option<String> {
        container_element(container, first)
    }

    fn classify_expr<'t>(&self, expr: Node<'t>, src: &str) -> Expr<'t> {
        let wrap = |n: Option<Node<'t>>| n.map_or(Expr::Unknown, Expr::Wrapper);
        match expr.kind() {
            k if is_literal(k) => Expr::Literal,
            "identifier" | "type_identifier" | "package_identifier" => Expr::Name(expr),
            "call_expression" => match field(expr, "function") {
                // `new(T)` is a *T; `make(T, n)` is a T.
                Some(f) if f.kind() == "identifier" && matches!(text(f, src), "new" | "make") => {
                    wrap(field(expr, "arguments").and_then(|a| a.named_child(0)))
                }
                Some(f) => Expr::Call(f),
                None => Expr::Unknown,
            },
            // `T{...}`: the same as converting to T.
            "composite_literal" => field(expr, "type").map_or(Expr::Unknown, Expr::Call),
            "selector_expression" => field(expr, "field").map_or(Expr::Unknown, Expr::Member),
            "qualified_type" => field(expr, "name").map_or(Expr::Unknown, Expr::Member),
            "parenthesized_expression" | "parenthesized_type" | "pointer_type" => wrap(expr.named_child(0)),
            "generic_type" => wrap(field(expr, "type")),
            "type_assertion_expression" | "type_conversion_expression" => wrap(field(expr, "type")),
            "slice_expression" => wrap(field(expr, "operand")),
            "index_expression" => field(expr, "operand").map_or(Expr::Unknown, Expr::Index),
            "unary_expression" => match field(expr, "operator").map(|o| text(o, src)) {
                Some("&" | "*") => wrap(field(expr, "operand")),
                Some("<-") => field(expr, "operand").map_or(Expr::Unknown, Expr::Index),
                _ => Expr::Literal,
            },
            _ => Expr::Unknown,
        }
    }

    /// For Go the "call" is a composite literal's type (see
    /// [`Language::named_argument_call`]).
    fn callee_name<'t>(&self, call: Node<'t>) -> Option<Node<'t>> {
        match call.kind() {
            "composite_literal" => field(call, "type").and_then(named_type_ident),
            "call_expression" => {
                let f = field(call, "function")?;
                match f.kind() {
                    "identifier" => Some(f),
                    "selector_expression" => field(f, "field"),
                    _ => None,
                }
            }
            // An anonymous struct is its own declaration.
            "struct_type" => Some(call),
            _ => named_type_ident(call),
        }
    }

    fn params_of<'t>(&self, decl_name: Node<'t>, _src: &str) -> Vec<Node<'t>> {
        if decl_name.kind() == "struct_type" {
            return struct_fields(decl_name);
        }
        let Some(decl) = decl_name.parent() else { return Vec::new() };
        match decl.kind() {
            "type_spec" => struct_fields(decl),
            k if is_function(k) => param_names(field(decl, "parameters")),
            _ => Vec::new(),
        }
    }

    fn enclosing_class<'t>(&self, _n: Node<'t>) -> Option<Node<'t>> {
        None
    }

    fn class_name(&self, _class: Node, _src: &str) -> Option<String> {
        None
    }

    fn class_supertypes(&self, _class: Node, _src: &str) -> Vec<String> {
        Vec::new()
    }

    /// Exported functions are capitalized too, so case says nothing.
    fn looks_like_type_name(&self, _name: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_from_paths() {
        assert_eq!(package_name("fmt"), "fmt");
        assert_eq!(package_name("example.com/shop/models"), "models");
        assert_eq!(package_name("github.com/go-chi/chi/v5"), "chi");
        assert_eq!(package_name("gopkg.in/yaml.v3"), "yaml");
        assert_eq!(package_name("github.com/mattn/go-isatty"), "isatty");
    }

    #[test]
    fn build_constraints() {
        let linux = ("linux", "amd64");
        assert!(build_expr_ok("!nomsgpack", linux));
        assert!(!build_expr_ok("nomsgpack", linux));
        assert!(build_expr_ok("linux && (amd64 || arm64)", linux));
        assert!(!build_expr_ok("darwin || windows", linux));
        assert!(build_expr_ok("unix && go1.21 && !ignore", linux));
        assert!(!build_expr_ok("ignore", linux));
        // Unparsable lines do not hide a file.
        assert!(build_expr_ok("linux &&", linux));
    }

    #[test]
    fn file_name_constraints() {
        let linux = ("linux", "amd64");
        let ok = |n: &str| file_name_ok_for(Path::new(n), linux);
        assert!(ok("x_linux.go") && ok("x_amd64.go") && ok("x_linux_amd64_test.go") && ok("x.go"));
        assert!(!ok("x_windows.go") && !ok("x_arm64.go") && !ok("x_linux_arm64.go") && !ok("x_darwin_test.go"));
        // `linux` alone as the whole name is not a constraint.
        assert!(ok("linux.go"));
    }

    #[test]
    fn module_lines() {
        assert_eq!(module_line("// x\nmodule example.com/shop // c\n\ngo 1.22\n").as_deref(), Some("example.com/shop"));
        assert_eq!(module_line("modulex foo\n"), None);
    }
}
