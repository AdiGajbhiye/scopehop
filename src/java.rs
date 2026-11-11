//! Java on top of tree-sitter-java. Same shape as `kotlin.rs`: which
//! identifiers declare things, which scopes they live in, cheap type hints.
//!
//! Java has no top-level functions, extension functions, named arguments or
//! string templates, so those trait methods keep their defaults. Names in
//! type positions are `type_identifier` nodes, everything else `identifier`.

use tree_sitter::Node;

use crate::lang::{child_of_kind, named_children, text, Def, DefKind, Expr, Import, Language, Scoped, Summary};

pub struct Java;

fn is_class_like(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration"
    )
}

fn is_literal(kind: &str) -> bool {
    matches!(
        kind,
        "decimal_integer_literal"
            | "hex_integer_literal"
            | "octal_integer_literal"
            | "binary_integer_literal"
            | "decimal_floating_point_literal"
            | "hex_floating_point_literal"
            | "string_literal"
            | "text_block"
            | "character_literal"
            | "true"
            | "false"
            | "null_literal"
            | "class_literal"
    )
}

fn is_primitive(kind: &str) -> bool {
    matches!(kind, "integral_type" | "floating_point_type" | "boolean_type" | "void_type")
}

fn field<'t>(n: Node<'t>, name: &str) -> Option<Node<'t>> {
    n.child_by_field_name(name)
}

fn is_field(n: Node, name: &str, child: Node) -> bool {
    field(n, name).is_some_and(|f| f.id() == child.id())
}

/// The node naming a type: `Foo` in `Foo`, `a.b.Foo`, `Foo<T>`, `Foo[]`.
fn type_head(t: Node) -> Option<Node> {
    match t.kind() {
        "type_identifier" => Some(t),
        "scoped_type_identifier" => named_children(t).into_iter().last(),
        "generic_type" | "array_type" | "annotated_type" => {
            named_children(t).into_iter().find_map(|c| type_head(c))
        }
        _ => None,
    }
}

/// Simple name of a type, or the primitive's keyword. `None` for `var`.
/// Arrays keep their `[]` (`Foo[]`): they are a library type with `length`
/// and `clone()`, not a Foo.
fn type_name_of(t: Node, src: &str) -> Option<String> {
    if is_primitive(t.kind()) {
        return Some(text(t, src).to_string());
    }
    if t.kind() == "array_type" {
        let elem = field(t, "element")?;
        let base = type_head(elem).map_or_else(|| text(elem, src).to_string(), |h| text(h, src).to_string());
        return Some(format!("{base}[]"));
    }
    let name = text(type_head(t)?, src);
    (name != "var").then(|| name.to_string())
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

/// Names declared by a `local_variable_declaration`, `field_declaration` or
/// `constant_declaration`.
fn declarator_names(decl: Node) -> Vec<Node> {
    named_children(decl)
        .into_iter()
        .filter(|c| c.kind() == "variable_declarator")
        .filter_map(|d| field(d, "name"))
        .collect()
}

/// Parameter names of a method, constructor, record or lambda (incl. type
/// parameters).
fn param_names(decl: Node) -> Vec<Node> {
    let mut out = Vec::new();
    if let Some(tps) = field(decl, "type_parameters") {
        out.extend(named_children(tps).into_iter().filter_map(|tp| child_of_kind(tp, "type_identifier")));
    }
    match field(decl, "parameters") {
        // `x -> ...`
        Some(p) if p.kind() == "identifier" => out.push(p),
        // `(a, b) -> ...`
        Some(p) if p.kind() == "inferred_parameters" => out.extend(named_children(p)),
        Some(p) => {
            for param in named_children(p) {
                match param.kind() {
                    "formal_parameter" => out.extend(field(param, "name")),
                    "spread_parameter" => {
                        out.extend(child_of_kind(param, "variable_declarator").and_then(|d| field(d, "name")))
                    }
                    _ => {}
                }
            }
        }
        None => {}
    }
    out
}

fn body_members(class: Node) -> Vec<Node> {
    let Some(body) = field(class, "body").or_else(|| child_of_kind(class, "class_body")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for m in named_children(body) {
        if m.kind() == "enum_body_declarations" {
            out.extend(named_children(m));
        } else {
            out.push(m);
        }
    }
    out
}

/// Names a class (or anonymous class) declares directly: type and record
/// parameters, fields, methods, nested types, enum constants.
fn class_member_names(class: Node) -> Vec<Node> {
    let mut out = Vec::new();
    if let Some(tps) = field(class, "type_parameters") {
        out.extend(named_children(tps).into_iter().filter_map(|tp| child_of_kind(tp, "type_identifier")));
    }
    if class.kind() == "record_declaration" {
        out.extend(param_names(class));
    }
    for m in body_members(class) {
        match m.kind() {
            "field_declaration" | "constant_declaration" => out.extend(declarator_names(m)),
            "method_declaration" | "enum_constant" | "annotation_type_element_declaration" => {
                out.extend(field(m, "name"))
            }
            k if is_class_like(k) => out.extend(field(m, "name")),
            _ => {}
        }
    }
    out
}

fn is_declaration_name(ident: Node) -> bool {
    let Some(p) = ident.parent() else { return false };
    match p.kind() {
        // A constructor's name is a use of its class.
        "constructor_declaration" => false,
        k if is_class_like(k) => is_field(p, "name", ident),
        "method_declaration"
        | "annotation_type_element_declaration"
        | "variable_declarator"
        | "formal_parameter"
        | "catch_formal_parameter"
        | "enum_constant"
        | "enhanced_for_statement"
        | "resource"
        | "instanceof_expression" => is_field(p, "name", ident),
        "lambda_expression" => is_field(p, "parameters", ident),
        "inferred_parameters" => true,
        "type_parameter" => child_of_kind(p, "type_identifier").map(|n| n.id()) == Some(ident.id()),
        _ => false,
    }
}

fn supertypes(class: Node, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in named_children(class) {
        match c.kind() {
            "superclass" => out.extend(named_children(c).into_iter().find_map(|t| type_name_of(t, src))),
            "super_interfaces" | "extends_interfaces" => {
                if let Some(list) = child_of_kind(c, "type_list") {
                    out.extend(named_children(list).into_iter().filter_map(|t| type_name_of(t, src)));
                }
            }
            _ => {}
        }
    }
    out
}

/// Type written on a declared name. `None` for `var`, which leaves the
/// initializer to say.
fn decl_type_name(name: Node, src: &str) -> Option<String> {
    let p = name.parent()?;
    let ty = match p.kind() {
        k if is_class_like(k) => return Some(text(name, src).to_string()),
        "variable_declarator" => {
            let decl = p.parent()?;
            match decl.kind() {
                // `String... rest`
                "spread_parameter" => named_children(decl).into_iter().next()?,
                _ => field(decl, "type")?,
            }
        }
        "formal_parameter" | "enhanced_for_statement" | "resource" => field(p, "type")?,
        "catch_formal_parameter" => named_children(child_of_kind(p, "catch_type")?).into_iter().next()?,
        "instanceof_expression" => field(p, "right")?,
        _ => return None,
    };
    type_name_of(ty, src)
}

fn decl_initializer(name: Node) -> Option<Node> {
    let p = name.parent()?;
    match p.kind() {
        "variable_declarator" | "resource" => field(p, "value"),
        _ => None,
    }
}

fn summarize(root: Node, src: &str) -> Summary {
    let mut s = Summary::default();
    for c in named_children(root) {
        match c.kind() {
            "package_declaration" => {
                if let Some(q) = named_children(c).into_iter().find(|n| matches!(n.kind(), "scoped_identifier" | "identifier")) {
                    s.package = text(q, src).split_whitespace().collect();
                }
            }
            "import_declaration" => {
                let kids = named_children(c);
                if let Some(q) = kids.iter().find(|n| matches!(n.kind(), "scoped_identifier" | "identifier")) {
                    s.imports.push(Import {
                        path: text(*q, src).split_whitespace().collect(),
                        star: kids.iter().any(|n| n.kind() == "asterisk"),
                        alias: None,
                    });
                }
            }
            _ => collect_defs(c, src, None, &mut s.defs),
        }
    }
    s
}

fn collect_defs(n: Node, src: &str, container: Option<&str>, out: &mut Vec<Def>) {
    let push = |out: &mut Vec<Def>, name: Node, kind, type_name, supertypes| {
        out.push(Def {
            name: text(name, src).to_string(),
            kind,
            start: name.start_byte(),
            end: name.end_byte(),
            container: container.map(str::to_string),
            type_name,
            supertypes,
        })
    };
    match n.kind() {
        k if is_class_like(k) => {
            let Some(name) = field(n, "name") else { return };
            let cname = text(name, src).to_string();
            push(out, name, DefKind::Class, Some(cname.clone()), supertypes(n, src));
            if k == "record_declaration" {
                // Components are fields, and `p.x()` calls their accessors.
                for p in param_names(n) {
                    if p.kind() == "identifier" {
                        out.push(Def {
                            name: text(p, src).to_string(),
                            kind: DefKind::Property,
                            start: p.start_byte(),
                            end: p.end_byte(),
                            container: Some(cname.clone()),
                            type_name: decl_type_name(p, src),
                            supertypes: Vec::new(),
                        });
                    }
                }
            }
            for m in body_members(n) {
                if m.kind() == "enum_constant" {
                    if let Some(c) = field(m, "name") {
                        out.push(Def {
                            name: text(c, src).to_string(),
                            kind: DefKind::EnumEntry,
                            start: c.start_byte(),
                            end: c.end_byte(),
                            container: Some(cname.clone()),
                            type_name: Some(cname.clone()),
                            supertypes: Vec::new(),
                        });
                    }
                } else {
                    collect_defs(m, src, Some(&cname), out);
                }
            }
        }
        "method_declaration" | "annotation_type_element_declaration" => {
            let Some(name) = field(n, "name") else { return };
            let generics: Vec<&str> = field(n, "type_parameters")
                .map(|tps| {
                    named_children(tps)
                        .into_iter()
                        .filter_map(|tp| child_of_kind(tp, "type_identifier"))
                        .map(|t| text(t, src))
                        .collect()
                })
                .unwrap_or_default();
            // `<T> T get()` tells us nothing.
            let ret = field(n, "type")
                .and_then(|t| type_name_of(t, src))
                .filter(|t| !generics.contains(&t.as_str()));
            push(out, name, DefKind::Function, ret, Vec::new());
        }
        "field_declaration" | "constant_declaration" => {
            let ty = field(n, "type").and_then(|t| type_name_of(t, src));
            for name in declarator_names(n) {
                push(out, name, DefKind::Property, ty.clone(), Vec::new());
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Scope resolution
// ---------------------------------------------------------------------------

fn find_in_scopes<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let pos = from.start_byte();
    let mut prev = from;
    let mut cur = from.parent();
    while let Some(scope) = cur {
        if let Some(hit) = lookup_scope(scope, prev, name, src, pos) {
            return Some(hit);
        }
        prev = scope;
        cur = scope.parent();
    }
    None
}

/// Pattern variables bound inside `n`: `o instanceof Circle c`.
fn pattern_names(n: Node) -> Vec<Node> {
    let mut out = Vec::new();
    let mut stack = vec![n];
    while let Some(x) = stack.pop() {
        if x.kind() == "instanceof_expression" {
            out.extend(field(x, "name"));
        }
        // Not into lambdas or nested classes: their bindings stay there.
        if !matches!(x.kind(), "lambda_expression" | "class_body") {
            stack.extend(named_children(x));
        }
    }
    out
}

/// `child` is the node on the path from the use up to `scope`.
fn lookup_scope<'t>(scope: Node<'t>, child: Node<'t>, name: &str, src: &str, pos: usize) -> Option<Node<'t>> {
    let hit = |nodes: Vec<Node<'t>>| nodes.into_iter().find(|n| text(*n, src) == name);
    match scope.kind() {
        "block" | "constructor_body" | "switch_block_statement_group" | "program" => {
            let ordered = scope.kind() != "program";
            for st in named_children(scope) {
                let found = match st.kind() {
                    // Locals are only visible after their declaration.
                    "local_variable_declaration" if st.end_byte() <= pos => hit(declarator_names(st)),
                    k if is_class_like(k) && (!ordered || st.start_byte() <= pos) => hit(field(st, "name").into_iter().collect()),
                    _ => None,
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        "method_declaration" | "constructor_declaration" | "lambda_expression" => hit(param_names(scope)),
        "for_statement" => hit(
            named_children(scope)
                .into_iter()
                .filter(|c| c.kind() == "local_variable_declaration")
                .flat_map(declarator_names)
                .collect(),
        ),
        "enhanced_for_statement" => hit(field(scope, "name").into_iter().collect()),
        "catch_clause" => hit(
            child_of_kind(scope, "catch_formal_parameter")
                .and_then(|p| field(p, "name"))
                .into_iter()
                .collect(),
        ),
        "try_with_resources_statement" => hit(
            field(scope, "resources")
                .map(|r| named_children(r).into_iter().filter_map(|res| field(res, "name")).collect())
                .unwrap_or_default(),
        ),
        // `if (o instanceof Circle c) { c... }`
        "if_statement" | "while_statement" => {
            let cond = field(scope, "condition")?;
            if cond.id() == child.id() {
                return None;
            }
            hit(pattern_names(cond))
        }
        // `o instanceof Circle c && c.area() > 0`
        "binary_expression" => {
            let (left, right) = (field(scope, "left")?, field(scope, "right")?);
            if right.id() != child.id() {
                return None;
            }
            hit(pattern_names(left))
        }
        k if is_class_like(k) => hit(class_member_names(scope)),
        // Anonymous class body.
        "object_creation_expression" if child.kind() == "class_body" => hit(class_member_names(scope)),
        _ => None,
    }
}

fn member_receiver(ident: Node) -> Option<Node> {
    let p = ident.parent()?;
    match p.kind() {
        "field_access" if is_field(p, "field", ident) => field(p, "object"),
        "method_invocation" if is_field(p, "name", ident) => field(p, "object"),
        // `Foo::bar`, `this::bar`
        "method_reference" => {
            let first = p.named_child(0)?;
            (first.id() != ident.id()).then_some(first)
        }
        _ => None,
    }
}

fn enclosing_class(n: Node) -> Option<Node> {
    let mut cur = n.parent();
    while let Some(p) = cur {
        if is_class_like(p.kind()) {
            return Some(p);
        }
        cur = p.parent();
    }
    None
}

// ---------------------------------------------------------------------------
// The Language impl
// ---------------------------------------------------------------------------

impl Language for Java {
    fn name(&self) -> &'static str {
        "Java"
    }

    fn grammar(&self) -> tree_sitter::Language {
        tree_sitter_java::LANGUAGE.into()
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn summarize(&self, root: Node, src: &str) -> Summary {
        summarize(root, src)
    }

    fn name_node<'t>(&self, root: Node<'t>, _src: &str, start: usize, end: usize) -> Option<Node<'t>> {
        let node = root.descendant_for_byte_range(start, end)?;
        (self.is_identifier(node) && node.start_byte() == start && node.end_byte() == end).then_some(node)
    }

    fn is_identifier(&self, n: Node) -> bool {
        matches!(n.kind(), "identifier" | "type_identifier")
    }

    fn is_declaration_name(&self, ident: Node) -> bool {
        is_declaration_name(ident)
    }

    fn named_argument_call<'t>(&self, _ident: Node<'t>) -> Option<Node<'t>> {
        None
    }

    fn member_receiver<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        member_receiver(ident)
    }

    fn type_qualifier<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        if ident.parent()?.kind() != "scoped_type_identifier" {
            return None;
        }
        let prev = ident.prev_named_sibling()?;
        match prev.kind() {
            "type_identifier" => Some(prev),
            // `a.b.Outer.Inner`: the qualifier's last part.
            "scoped_type_identifier" => named_children(prev).into_iter().last(),
            _ => None,
        }
    }

    fn in_type_position(&self, n: Node) -> bool {
        n.kind() == "type_identifier"
    }

    fn qualified_path(&self, ident: Node, src: &str) -> Option<String> {
        // `a.b.C`: a scoped_identifier whose `name` is the identifier. The
        // leftmost part (`a`) is a bare identifier.
        let parent = ident.parent()?;
        let node = if parent.kind() == "scoped_identifier" && is_field(parent, "name", ident) {
            parent
        } else {
            ident
        };
        let mut top = node;
        while top.parent()?.kind() == "scoped_identifier" {
            top = top.parent()?;
        }
        matches!(top.parent()?.kind(), "import_declaration" | "package_declaration")
            .then(|| text(node, src).split_whitespace().collect())
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

    fn classify_expr<'t>(&self, expr: Node<'t>) -> Expr<'t> {
        if is_literal(expr.kind()) {
            return Expr::Literal;
        }
        match expr.kind() {
            "this" => Expr::This,
            "super" => Expr::Super,
            "identifier" => Expr::Name(expr),
            // The method's name resolves through its receiver, so its type
            // is the return type.
            "method_invocation" => field(expr, "name").map_or(Expr::Unknown, Expr::Call),
            "field_access" => field(expr, "field").map_or(Expr::Unknown, Expr::Member),
            // `new Foo()` and `(Foo) x` are Foos: resolve the type's name.
            "object_creation_expression" | "cast_expression" => {
                field(expr, "type").and_then(type_head).map_or(Expr::Unknown, Expr::Name)
            }
            "parenthesized_expression" => expr.named_child(0).map_or(Expr::Unknown, Expr::Wrapper),
            // `(a = b)` is an a; `c ? a : b` is (probably) an a.
            "assignment_expression" => field(expr, "left").map_or(Expr::Unknown, Expr::Wrapper),
            "ternary_expression" => field(expr, "consequence").map_or(Expr::Unknown, Expr::Wrapper),
            _ => Expr::Unknown,
        }
    }

    fn callee_name<'t>(&self, call: Node<'t>) -> Option<Node<'t>> {
        field(call, "name")
    }

    fn params_of<'t>(&self, decl_name: Node<'t>) -> Vec<Node<'t>> {
        let Some(decl) = decl_name.parent() else { return Vec::new() };
        match decl.kind() {
            "method_declaration" | "record_declaration" => param_names(decl),
            _ => Vec::new(),
        }
    }

    fn enclosing_class<'t>(&self, n: Node<'t>) -> Option<Node<'t>> {
        enclosing_class(n)
    }

    fn class_name(&self, class: Node, src: &str) -> Option<String> {
        field(class, "name").map(|n| text(n, src).to_string())
    }

    fn class_supertypes(&self, class: Node, src: &str) -> Vec<String> {
        supertypes(class, src)
    }
}
