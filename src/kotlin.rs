//! Kotlin on top of tree-sitter-kotlin-ng: which identifiers declare things,
//! which scopes they live in, and cheap type hints. Everything here is reached
//! through the [`Language`] impl at the bottom.

use tree_sitter::Node;

use crate::lang::{child_of_kind, named_children, text, Def, DefKind, Expr, Import, Language, Scoped, Summary};

pub struct Kotlin;

fn is_type_node(kind: &str) -> bool {
    matches!(
        kind,
        "user_type" | "nullable_type" | "function_type" | "parenthesized_type" | "type"
    )
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

/// Identifiers declared by a `variable_declaration` or destructuring declaration.
fn var_names<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
    match n.kind() {
        "variable_declaration" => out.extend(child_of_kind(n, "identifier")),
        "multi_variable_declaration" => {
            for c in named_children(n) {
                var_names(c, out);
            }
        }
        _ => {}
    }
}

fn property_names<'t>(prop: Node<'t>) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    for c in named_children(prop) {
        var_names(c, &mut out);
    }
    out
}

/// Name identifiers introduced by a declaration node.
fn decl_names<'t>(decl: Node<'t>) -> Vec<Node<'t>> {
    match decl.kind() {
        "class_declaration" | "object_declaration" | "function_declaration" => {
            decl.child_by_field_name("name").into_iter().collect()
        }
        "companion_object" => decl
            .child_by_field_name("name")
            .or_else(|| child_of_kind(decl, "identifier"))
            .into_iter()
            .collect(),
        "type_alias" => decl.child_by_field_name("type").into_iter().collect(),
        "property_declaration" => property_names(decl),
        "enum_entry" => child_of_kind(decl, "identifier").into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Parameter names of a function-like node (incl. type parameters).
fn param_names<'t>(func: Node<'t>) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    for c in named_children(func) {
        match c.kind() {
            "function_value_parameters" | "type_parameters" => {
                for p in named_children(c) {
                    if matches!(p.kind(), "parameter" | "type_parameter") {
                        out.extend(child_of_kind(p, "identifier"));
                    }
                }
            }
            // setter(value)
            "parameter" | "parameter_with_optional_type" => {
                out.extend(child_of_kind(c, "identifier"))
            }
            _ => {}
        }
    }
    out
}

/// Members declared directly in a class/object/companion (params + body).
fn class_member_names<'t>(class: Node<'t>) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    for c in named_children(class) {
        match c.kind() {
            "type_parameters" => out.extend(param_names_of_list(c)),
            "primary_constructor" => {
                if let Some(params) = child_of_kind(c, "class_parameters") {
                    for p in named_children(params) {
                        if p.kind() == "class_parameter" {
                            out.extend(child_of_kind(p, "identifier"));
                        }
                    }
                }
            }
            "class_body" | "enum_class_body" => {
                for m in named_children(c) {
                    if m.kind() == "companion_object" {
                        out.extend(decl_names(m));
                        out.extend(class_member_names(m));
                    } else {
                        out.extend(decl_names(m));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn param_names_of_list<'t>(list: Node<'t>) -> Vec<Node<'t>> {
    named_children(list)
        .into_iter()
        .filter_map(|p| child_of_kind(p, "identifier"))
        .collect()
}

/// True if `ident` is the name being declared (not a use).
fn is_declaration_name(ident: Node) -> bool {
    let Some(p) = ident.parent() else { return false };
    let first_ident = || child_of_kind(p, "identifier").map(|n| n.id()) == Some(ident.id());
    match p.kind() {
        "class_declaration" | "object_declaration" | "function_declaration" => {
            p.child_by_field_name("name").map(|n| n.id()) == Some(ident.id())
        }
        "type_alias" => p.child_by_field_name("type").map(|n| n.id()) == Some(ident.id()),
        "companion_object" | "variable_declaration" | "parameter" | "class_parameter"
        | "enum_entry" | "catch_block" | "type_parameter" | "parameter_with_optional_type" => {
            first_ident()
        }
        _ => false,
    }
}

/// Last identifier of the first `user_type` in a type node, e.g. `List` for
/// `kotlin.collections.List<Foo>?`.
fn type_name_of(t: Node, src: &str) -> Option<String> {
    let user = if t.kind() == "user_type" {
        t
    } else {
        let mut stack = vec![t];
        let mut found = None;
        while let Some(n) = stack.pop() {
            if n.kind() == "user_type" {
                found = Some(n);
                break;
            }
            stack.extend(named_children(n).into_iter().rev());
        }
        found?
    };
    named_children(user)
        .into_iter()
        .filter(|c| c.kind() == "identifier")
        .last()
        .map(|n| text(n, src).to_string())
}

fn constructor_call_type(expr: Node, src: &str) -> Option<String> {
    if expr.kind() != "call_expression" {
        return None;
    }
    let callee = expr.named_child(0)?;
    let name = match callee.kind() {
        "identifier" => text(callee, src),
        // com.example.Foo(...)
        "navigation_expression" => {
            let last = named_children(callee).into_iter().last()?;
            text(last, src)
        }
        _ => return None,
    };
    name.chars()
        .next()
        .filter(|c| c.is_uppercase())
        .map(|_| name.to_string())
}

/// Type hint for a declared name: explicit annotation first, then `= Foo(...)`.
fn decl_type_name(name_node: Node, src: &str) -> Option<String> {
    let p = name_node.parent()?;
    match p.kind() {
        "class_declaration" | "object_declaration" => return Some(text(name_node, src).into()),
        "variable_declaration" | "parameter" | "class_parameter" | "catch_block" => {}
        _ => return None,
    }
    if let Some(t) = named_children(p).into_iter().find(|c| is_type_node(c.kind())) {
        return type_name_of(t, src);
    }
    if p.kind() == "variable_declaration" {
        let prop = p.parent()?;
        if prop.kind() == "property_declaration" {
            let mut after = false;
            for c in named_children(prop) {
                if c.id() == p.id() {
                    after = true;
                } else if after {
                    if let Some(t) = constructor_call_type(c, src) {
                        return Some(t);
                    }
                    if c.kind() == "property_delegate" {
                        break;
                    }
                }
            }
        }
    }
    None
}

fn supertypes(class: Node, src: &str) -> Vec<String> {
    let Some(specs) = child_of_kind(class, "delegation_specifiers") else {
        return Vec::new();
    };
    named_children(specs)
        .into_iter()
        .filter_map(|s| type_name_of(s, src))
        .collect()
}

/// `a.Foo.Companion.bar` -> `a.Foo.bar`. Companion members are indexed as
/// members of the enclosing class, so that is their qualified name.
fn drop_companion(path: &str) -> String {
    let parts: Vec<&str> = path.split('.').filter(|p| *p != "Companion").collect();
    parts.join(".")
}

/// Package, imports and top-level/member declarations of a file.
fn summarize(root: Node, src: &str) -> Summary {
    let mut s = Summary::default();
    for c in named_children(root) {
        match c.kind() {
            "package_header" => {
                if let Some(q) = child_of_kind(c, "qualified_identifier")
                    .or_else(|| child_of_kind(c, "identifier"))
                {
                    s.package = text(q, src).split_whitespace().collect();
                }
            }
            "import" => {
                let raw = text(c, src).trim_start_matches("import").trim();
                let raw = raw.trim_end_matches(';').trim();
                let (path, alias) = match raw.split_once(" as ") {
                    Some((p, a)) => (p.trim(), Some(a.trim().to_string())),
                    None => (raw, None),
                };
                let path: String = path.split_whitespace().collect();
                let star = path.ends_with(".*");
                s.imports.push(Import {
                    path: drop_companion(path.trim_end_matches(".*")),
                    star,
                    alias,
                });
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
        "class_declaration" | "object_declaration" => {
            let Some(name) = n.child_by_field_name("name") else { return };
            let kind = if n.kind() == "object_declaration" { DefKind::Object } else { DefKind::Class };
            let cname = text(name, src).to_string();
            push(out, name, kind, Some(cname.clone()), supertypes(n, src));
            for c in named_children(n) {
                match c.kind() {
                    "primary_constructor" => {
                        if let Some(params) = child_of_kind(c, "class_parameters") {
                            for p in named_children(params) {
                                // Only `val`/`var` parameters become properties, but
                                // plain ones are still worth jumping to.
                                if let Some(id) = child_of_kind(p, "identifier") {
                                    out.push(Def {
                                        name: text(id, src).to_string(),
                                        kind: DefKind::Property,
                                        start: id.start_byte(),
                                        end: id.end_byte(),
                                        container: Some(cname.clone()),
                                        type_name: decl_type_name(id, src),
                                        supertypes: Vec::new(),
                                    });
                                }
                            }
                        }
                    }
                    "class_body" | "enum_class_body" => {
                        for m in named_children(c) {
                            collect_defs(m, src, Some(&cname), out);
                        }
                    }
                    _ => {}
                }
            }
        }
        "companion_object" => {
            if let Some(name) = decl_names(n).into_iter().next() {
                push(out, name, DefKind::Object, container.map(str::to_string), Vec::new());
            }
            if let Some(body) = child_of_kind(n, "class_body") {
                for m in named_children(body) {
                    collect_defs(m, src, container, out);
                }
            }
        }
        "function_declaration" => {
            if let Some(name) = n.child_by_field_name("name") {
                let generics: Vec<String> = child_of_kind(n, "type_parameters")
                    .map(|tp| param_names_of_list(tp).into_iter().map(|i| text(i, src).to_string()).collect())
                    .unwrap_or_default();
                // `fun <T> f(): T` tells us nothing.
                let ret = named_children(n)
                    .into_iter()
                    .skip_while(|c| c.kind() != "function_value_parameters")
                    .find(|c| is_type_node(c.kind()))
                    .and_then(|t| type_name_of(t, src))
                    .filter(|t| !generics.contains(t));
                push(out, name, DefKind::Function, ret, Vec::new());
            }
        }
        "property_declaration" => {
            for name in property_names(n) {
                push(out, name, DefKind::Property, decl_type_name(name, src), Vec::new());
            }
        }
        "enum_entry" => {
            if let Some(name) = child_of_kind(n, "identifier") {
                push(out, name, DefKind::EnumEntry, container.map(str::to_string), Vec::new());
            }
        }
        "type_alias" => {
            if let Some(name) = n.child_by_field_name("type") {
                push(out, name, DefKind::TypeAlias, None, Vec::new());
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Scope resolution
// ---------------------------------------------------------------------------

/// Walks outwards from `from` and returns the node that declares `name`:
/// usually its name identifier, or the lambda itself for an implicit `it`.
fn find_in_scopes<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let pos = from.start_byte();
    let mut cur = from.parent();
    while let Some(scope) = cur {
        if let Some(hit) = lookup_scope(scope, name, src, pos) {
            return Some(hit);
        }
        cur = scope.parent();
    }
    None
}

fn lookup_scope<'t>(scope: Node<'t>, name: &str, src: &str, pos: usize) -> Option<Node<'t>> {
    let hit = |nodes: Vec<Node<'t>>| nodes.into_iter().find(|n| text(*n, src) == name);
    match scope.kind() {
        "block" | "statements" | "source_file" | "lambda_literal" | "control_structure_body" => {
            let ordered = scope.kind() != "source_file";
            for st in named_children(scope) {
                let found = match st.kind() {
                    // Local variables are only visible after their declaration.
                    "property_declaration" if !ordered || st.end_byte() <= pos => hit(property_names(st)),
                    "function_declaration" | "class_declaration" | "object_declaration"
                    | "type_alias" => hit(decl_names(st)),
                    "lambda_parameters" => {
                        let mut names = Vec::new();
                        for c in named_children(st) {
                            var_names(c, &mut names);
                        }
                        hit(names)
                    }
                    _ => None,
                };
                if found.is_some() {
                    return found;
                }
            }
            if scope.kind() == "lambda_literal"
                && name == "it"
                && child_of_kind(scope, "lambda_parameters").is_none()
            {
                return Some(scope);
            }
            None
        }
        "function_declaration" | "anonymous_function" | "secondary_constructor" | "getter"
        | "setter" => hit(param_names(scope)),
        "for_statement" => {
            let mut names = Vec::new();
            for c in named_children(scope) {
                var_names(c, &mut names);
            }
            hit(names)
        }
        "catch_block" => hit(child_of_kind(scope, "identifier").into_iter().collect()),
        "when_expression" => {
            let subject = child_of_kind(scope, "when_subject")?;
            let mut names = Vec::new();
            for c in named_children(subject) {
                var_names(c, &mut names);
            }
            hit(names)
        }
        "class_declaration" | "object_declaration" | "companion_object" | "object_literal" => {
            hit(class_member_names(scope))
        }
        _ => None,
    }
}

/// If `ident` is the member part of `recv.ident`, `recv::ident` or the
/// function of an infix call `recv ident arg`, returns `recv`.
fn member_receiver(ident: Node) -> Option<Node> {
    let p = ident.parent()?;
    match p.kind() {
        "navigation_expression" | "callable_reference" => {
            let first = p.named_child(0)?;
            (first.id() != ident.id()).then_some(first)
        }
        "infix_expression" => {
            let mid = p.named_child(1)?;
            (mid.id() == ident.id()).then(|| p.named_child(0)).flatten()
        }
        _ => None,
    }
}

/// For the label of a named argument (`foo(label = x)`), the call expression.
fn named_argument_call(ident: Node) -> Option<Node> {
    let arg = ident.parent()?;
    if arg.kind() != "value_argument" || arg.named_child(0)?.id() != ident.id() {
        return None;
    }
    if ident.next_sibling()?.kind() != "=" {
        return None;
    }
    let call = arg.parent()?.parent()?;
    (call.kind() == "call_expression").then_some(call)
}

/// `foo` in `val x = foo`, for type inference.
fn decl_initializer(name_node: Node) -> Option<Node> {
    let var = name_node.parent()?;
    if var.kind() != "variable_declaration" {
        return None;
    }
    let prop = var.parent()?;
    if prop.kind() != "property_declaration" {
        return None;
    }
    let mut after = false;
    for c in named_children(prop) {
        if c.id() == var.id() {
            after = true;
        } else if after && !is_type_node(c.kind()) && !matches!(c.kind(), "getter" | "setter") {
            return (c.kind() != "property_delegate").then_some(c);
        }
    }
    None
}

/// Receiver type of the nearest enclosing extension function (`fun Dp.f()`).
fn extension_receiver_type(n: Node, src: &str) -> Option<String> {
    let mut cur = n.parent();
    while let Some(p) = cur {
        match p.kind() {
            "function_declaration" => {
                let name = p.child_by_field_name("name")?;
                let recv = named_children(p)
                    .into_iter()
                    .take_while(|c| c.id() != name.id())
                    .find(|c| is_type_node(c.kind()));
                if let Some(t) = recv {
                    return type_name_of(t, src);
                }
            }
            "class_declaration" | "object_declaration" => return None,
            _ => {}
        }
        cur = p.parent();
    }
    None
}

fn is_literal(kind: &str) -> bool {
    matches!(
        kind,
        "string_literal" | "number_literal" | "float_literal" | "boolean_literal"
            | "character_literal" | "null_literal" | "multiline_string_literal"
    )
}

/// Parameter name nodes of the function or class whose name node is given.
fn params_of(decl_name: Node) -> Vec<Node> {
    let Some(decl) = decl_name.parent() else { return Vec::new() };
    match decl.kind() {
        "function_declaration" => param_names(decl),
        "class_declaration" => child_of_kind(decl, "primary_constructor")
            .and_then(|c| child_of_kind(c, "class_parameters"))
            .map(|ps| param_names_of_list(ps))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn enclosing_class<'t>(n: Node<'t>) -> Option<Node<'t>> {
    let mut cur = n.parent();
    while let Some(p) = cur {
        if matches!(p.kind(), "class_declaration" | "object_declaration") {
            return Some(p);
        }
        cur = p.parent();
    }
    None
}

/// Last named child, e.g. `bar` in the navigation expression `foo.bar`.
fn last_named_child(n: Node) -> Option<Node> {
    n.named_child(n.named_child_count().saturating_sub(1) as _)
}

// ---------------------------------------------------------------------------
// The Language impl
// ---------------------------------------------------------------------------

impl Language for Kotlin {
    fn name(&self) -> &'static str {
        "Kotlin"
    }

    fn grammar(&self) -> tree_sitter::Language {
        tree_sitter_kotlin_ng::LANGUAGE.into()
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["kt", "kts"]
    }

    fn summarize(&self, root: Node, src: &str) -> Summary {
        summarize(root, src)
    }

    fn name_node<'t>(&self, root: Node<'t>, src: &str, start: usize, end: usize) -> Option<Node<'t>> {
        let node = root.descendant_for_byte_range(start, end)?;
        if node.kind() == "identifier" && node.start_byte() == start && node.end_byte() == end {
            return Some(node);
        }
        // "$name" string templates are plain string content in this grammar.
        if node.kind() == "string_content" && start > 0 && src.as_bytes()[start - 1] == b'$' {
            return Some(node);
        }
        None
    }

    fn is_identifier(&self, n: Node) -> bool {
        n.kind() == "identifier"
    }

    fn is_declaration_name(&self, ident: Node) -> bool {
        is_declaration_name(ident)
    }

    fn named_argument_call<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        named_argument_call(ident)
    }

    fn member_receiver<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        member_receiver(ident)
    }

    fn type_qualifier<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        if !self.in_type_position(ident) {
            return None;
        }
        ident.prev_named_sibling().filter(|n| n.kind() == "identifier")
    }

    fn in_type_position(&self, n: Node) -> bool {
        n.parent().is_some_and(|p| p.kind() == "user_type")
    }

    fn qualified_path(&self, ident: Node, src: &str) -> Option<String> {
        let parent = ident.parent()?;
        // `yell` in `import a.b.shout as yell`
        let (path, upto) = match parent.kind() {
            "import" => (child_of_kind(parent, "qualified_identifier")?, None),
            "qualified_identifier" if matches!(parent.parent()?.kind(), "import" | "package_header") => {
                (parent, Some(ident.id()))
            }
            _ => return None,
        };
        let mut parts = Vec::new();
        for seg in named_children(path) {
            parts.push(text(seg, src));
            if Some(seg.id()) == upto {
                break;
            }
        }
        Some(drop_companion(&parts.join(".")))
    }

    fn find_in_scopes<'t>(&self, from: Node<'t>, name: &str, src: &str) -> Option<Scoped<'t>> {
        find_in_scopes(from, name, src).map(|n| match n.kind() {
            "lambda_literal" => Scoped::Implicit(n),
            _ => Scoped::Decl(n),
        })
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
            "this_expression" => Expr::This,
            "super_expression" => Expr::Super,
            "identifier" => Expr::Name(expr),
            "call_expression" => expr.named_child(0).map_or(Expr::Unknown, Expr::Call),
            "navigation_expression" => match last_named_child(expr) {
                Some(last) if last.kind() == "identifier" => Expr::Member(last),
                _ => Expr::Unknown,
            },
            // (x), x!!, and similar wrappers
            _ if expr.named_child_count() == 1 => expr.named_child(0).map_or(Expr::Unknown, Expr::Wrapper),
            _ => Expr::Unknown,
        }
    }

    fn callee_name<'t>(&self, call: Node<'t>) -> Option<Node<'t>> {
        let callee = call.named_child(0)?;
        let callee = match callee.kind() {
            "navigation_expression" => last_named_child(callee)?,
            _ => callee,
        };
        (callee.kind() == "identifier").then_some(callee)
    }

    fn params_of<'t>(&self, decl_name: Node<'t>) -> Vec<Node<'t>> {
        params_of(decl_name)
    }

    fn enclosing_class<'t>(&self, n: Node<'t>) -> Option<Node<'t>> {
        enclosing_class(n)
    }

    fn class_name(&self, class: Node, src: &str) -> Option<String> {
        class.child_by_field_name("name").map(|n| text(n, src).to_string())
    }

    fn class_supertypes(&self, class: Node, src: &str) -> Vec<String> {
        supertypes(class, src)
    }

    fn extension_receiver_type(&self, n: Node, src: &str) -> Option<String> {
        extension_receiver_type(n, src)
    }
}
