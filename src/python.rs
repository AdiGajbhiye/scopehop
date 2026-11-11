//! Python on top of tree-sitter-python.
//!
//! Python differs from the JVM languages in three ways that matter here:
//!
//! - Every file is a module, named from its path ([`module_name`]), and a
//!   file sees only its own names and what it imports. The index handles that
//!   through [`Visibility::Modules`].
//! - Scopes are functions, classes, lambdas and comprehensions, not blocks.
//!   A name assigned anywhere in a function is local to all of it, and every
//!   use goes to the first binding in that scope, so definition and
//!   references agree. Class bodies are not visible from their methods.
//! - Types come from annotations, `self`, `Foo()` calls and return
//!   annotations. Instance attributes are the `self.x = ...` assignments in
//!   methods.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::lang::{child_of_kind, named_children, text, Def, DefKind, Expr, Import, Language, Scoped, Summary, Visibility};

pub struct Python;

fn field<'t>(n: Node<'t>, name: &str) -> Option<Node<'t>> {
    n.child_by_field_name(name)
}

fn is_field(n: Node, name: &str, child: Node) -> bool {
    field(n, name).is_some_and(|f| f.id() == child.id())
}

fn is_literal(kind: &str) -> bool {
    matches!(
        kind,
        "string"
            | "concatenated_string"
            | "integer"
            | "float"
            | "true"
            | "false"
            | "none"
            | "ellipsis"
            | "list"
            | "tuple"
            | "dictionary"
            | "set"
            | "list_comprehension"
            | "dictionary_comprehension"
            | "set_comprehension"
            | "generator_expression"
            | "comparison_operator"
            | "not_operator"
    )
}

fn is_comprehension(kind: &str) -> bool {
    matches!(
        kind,
        "list_comprehension" | "set_comprehension" | "dictionary_comprehension" | "generator_expression"
    )
}

/// Dotted text without the whitespace Python allows inside it.
fn dotted(n: Node, src: &str) -> String {
    text(n, src).split_whitespace().collect()
}

/// `module` + `.` + `name`, where a relative module may end in dots.
fn join(module: &str, name: &str) -> String {
    if module.is_empty() || module.ends_with('.') {
        format!("{module}{name}")
    } else {
        format!("{module}.{name}")
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The class an annotation names: `Foo`, `models.Foo`, `"Foo"`,
/// `Optional[Foo]`, `Foo | None`. Containers (`list[Foo]`) are their own
/// library type.
fn ann_type(n: Node, src: &str) -> Option<String> {
    match n.kind() {
        "type" => ann_type(n.named_child(0)?, src),
        "identifier" => Some(text(n, src).to_string()),
        "attribute" => field(n, "attribute").map(|a| text(a, src).to_string()),
        "string" => ann_str(text(child_of_kind(n, "string_content")?, src)),
        "generic_type" | "subscript" => {
            let kids = named_children(n);
            let head = ann_type(*kids.first()?, src)?;
            let args: Vec<Node> = match n.kind() {
                "generic_type" => kids
                    .iter()
                    .filter(|k| k.kind() == "type_parameter")
                    .flat_map(|k| named_children(*k))
                    .collect(),
                _ => kids[1..].to_vec(),
            };
            match head.as_str() {
                "Optional" => args.first().and_then(|a| ann_type(*a, src)),
                "Union" => single(args.iter().map(|a| (is_none(*a), *a)).collect(), src),
                _ => Some(head),
            }
        }
        "binary_operator" | "union_type" => {
            let mut parts = Vec::new();
            union_parts(n, &mut parts);
            single(parts.into_iter().map(|p| (is_none(p), p)).collect(), src)
        }
        _ => None,
    }
}

fn is_none(n: Node) -> bool {
    n.kind() == "none" || (n.kind() == "type" && n.named_child(0).is_some_and(|c| c.kind() == "none"))
}

fn union_parts<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
    if matches!(n.kind(), "binary_operator" | "union_type") {
        for c in named_children(n) {
            union_parts(c, out);
        }
    } else if n.kind() == "type" && n.named_child(0).is_some_and(|c| matches!(c.kind(), "binary_operator" | "union_type")) {
        union_parts(n.named_child(0).unwrap(), out);
    } else {
        out.push(n);
    }
}

/// The one non-`None` member of a union, if there is exactly one.
fn single(parts: Vec<(bool, Node)>, src: &str) -> Option<String> {
    let rest: Vec<Node> = parts.into_iter().filter(|(none, _)| !none).map(|(_, n)| n).collect();
    match rest.as_slice() {
        [one] => ann_type(*one, src),
        _ => None,
    }
}

/// A string annotation (`"Foo"`, `"models.Foo"`, `"Optional[Foo]"`).
fn ann_str(s: &str) -> Option<String> {
    let s = s.trim();
    let s = s.strip_prefix("Optional[").and_then(|r| r.strip_suffix(']')).unwrap_or(s);
    let parts: Vec<&str> = s.split('|').map(str::trim).filter(|p| *p != "None").collect();
    let [one] = parts.as_slice() else { return None };
    let head = one.split('[').next()?.rsplit('.').next()?.trim();
    (!head.is_empty() && head.chars().all(|c| c.is_alphanumeric() || c == '_')).then(|| head.to_string())
}

/// For a function directly in a class body: the class, and the decorated
/// definition wrapping the function if there is one.
fn method_class(func: Node) -> Option<(Node, Option<Node>)> {
    let mut up = func.parent()?;
    let decorated = (up.kind() == "decorated_definition").then_some(up);
    if decorated.is_some() {
        up = up.parent()?;
    }
    let class = up.parent()?;
    (up.kind() == "block" && class.kind() == "class_definition").then_some((class, decorated))
}

fn has_decorator(decorated: Option<Node>, names: &[&str], src: &str) -> bool {
    let Some(d) = decorated else { return false };
    named_children(d).into_iter().filter(|c| c.kind() == "decorator").any(|c| {
        let t = text(c, src).trim_start_matches('@').trim();
        let last = t.rsplit('.').next().unwrap_or(t);
        names.contains(&last)
    })
}

/// `self` (or `cls`): the first parameter of a method that is not static.
fn self_type(params: Node, ident: Node, src: &str) -> Option<String> {
    if params.named_child(0)?.id() != ident.id() {
        return None;
    }
    let func = params.parent()?;
    if func.kind() != "function_definition" {
        return None;
    }
    let (class, decorated) = method_class(func)?;
    if has_decorator(decorated, &["staticmethod"], src) {
        return None;
    }
    field(class, "name").map(|n| text(n, src).to_string())
}

fn decl_type_name(name: Node, src: &str) -> Option<String> {
    let p = name.parent()?;
    match p.kind() {
        "class_definition" if is_field(p, "name", name) => Some(text(name, src).to_string()),
        "function_definition" if is_field(p, "name", name) => ann_type(field(p, "return_type")?, src),
        "typed_parameter" => ann_type(field(p, "type")?, src),
        "typed_default_parameter" if is_field(p, "name", name) => ann_type(field(p, "type")?, src),
        "parameters" => self_type(p, name, src),
        "assignment" if is_field(p, "left", name) => ann_type(field(p, "type")?, src),
        // `self.x: Foo = ...`
        "attribute" if is_field(p, "attribute", name) => {
            let a = p.parent()?;
            (a.kind() == "assignment" && is_field(a, "left", p)).then_some(())?;
            ann_type(field(a, "type")?, src)
        }
        _ => None,
    }
}

/// The value assigned in `a = b = value`.
fn assigned_value(assignment: Node) -> Option<Node> {
    let mut v = field(assignment, "right")?;
    while v.kind() == "assignment" {
        v = field(v, "right")?;
    }
    Some(v)
}

fn decl_initializer(name: Node) -> Option<Node> {
    let p = name.parent()?;
    match p.kind() {
        "assignment" if is_field(p, "left", name) => assigned_value(p),
        "attribute" if is_field(p, "attribute", name) => {
            let a = p.parent()?;
            (a.kind() == "assignment" && is_field(a, "left", p)).then_some(())?;
            assigned_value(a)
        }
        "named_expression" if is_field(p, "name", name) => field(p, "value"),
        // `with open() as f`, `except E as e`
        "as_pattern_target" => p.parent()?.named_child(0),
        _ => None,
    }
}

/// `Foo(...)` or `m.Foo(...)` with a capitalized callee: a Foo, as far as
/// the summary can tell without resolving anything.
fn constructed_type(value: Node, src: &str) -> Option<String> {
    if value.kind() != "call" {
        return None;
    }
    let f = field(value, "function")?;
    let name = match f.kind() {
        "identifier" => f,
        "attribute" => field(f, "attribute")?,
        _ => return None,
    };
    let t = text(name, src);
    t.starts_with(|c: char| c.is_uppercase()).then(|| t.to_string())
}

// ---------------------------------------------------------------------------
// Bindings and scopes
// ---------------------------------------------------------------------------

/// Parameter names of a `parameters` or `lambda_parameters` node.
fn param_names<'t>(params: Node<'t>) -> Vec<Node<'t>> {
    let splat_name = |n: Node<'t>| child_of_kind(n, "identifier");
    let mut out = Vec::new();
    for p in named_children(params) {
        match p.kind() {
            "identifier" => out.push(p),
            "typed_parameter" => match p.named_child(0) {
                Some(c) if c.kind() == "identifier" => out.push(c),
                Some(c) => out.extend(splat_name(c)),
                None => {}
            },
            "default_parameter" | "typed_default_parameter" => out.extend(field(p, "name")),
            "list_splat_pattern" | "dictionary_splat_pattern" => out.extend(splat_name(p)),
            _ => {}
        }
    }
    out
}

fn function_params(func: Node) -> Vec<Node> {
    field(func, "parameters").map(param_names).unwrap_or_default()
}

/// Names bound by an assignment target: `a`, `a, (b, *c)`, `[a, b]`.
fn targets<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
    match n.kind() {
        "identifier" => out.push(n),
        "pattern_list" | "tuple_pattern" | "list_pattern" | "tuple" | "list" | "parenthesized_expression"
        | "list_splat_pattern" | "list_splat" => {
            for c in named_children(n) {
                targets(c, out);
            }
        }
        _ => {}
    }
}

/// A `dotted_name` that is a single identifier: a capture in a `case`.
fn capture(n: Node) -> Option<Node> {
    let kids = named_children(n);
    (n.kind() == "dotted_name" && kids.len() == 1).then(|| kids[0])
}

/// Calls `f` on every node under `n` that belongs to `n`'s scope, so not on
/// anything inside a nested function, class, lambda or comprehension (but on
/// the nested scope's own node).
fn walk_scope<'t>(n: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    for c in named_children(n) {
        f(c);
        let k = c.kind();
        if !matches!(k, "function_definition" | "class_definition" | "lambda") && !is_comprehension(k) {
            walk_scope(c, f);
        }
    }
}

/// Names bound directly in a scope (a module, or a class or function body),
/// in source order: assignments, `for`, `with ... as`, `except ... as`,
/// walrus, `case` captures, `type X = ...`, and the names of nested
/// functions and classes. Imports are left to the index.
fn bindings(scope: Node) -> Vec<Node> {
    let mut out = Vec::new();
    walk_scope(scope, &mut |n| match n.kind() {
        "function_definition" | "class_definition" => out.extend(field(n, "name")),
        "assignment" | "augmented_assignment" | "for_statement" => {
            if let Some(l) = field(n, "left") {
                targets(l, &mut out);
            }
        }
        "named_expression" => out.extend(field(n, "name")),
        "as_pattern_target" => {
            for c in named_children(n) {
                targets(c, &mut out);
            }
        }
        "type_alias_statement" => out.extend(field(n, "left").and_then(|t| child_of_kind(t, "identifier"))),
        "case_pattern" => out.extend(n.named_child(0).and_then(capture)),
        "keyword_pattern" => out.extend(n.named_child(1).and_then(capture)),
        "splat_pattern" => out.extend(child_of_kind(n, "identifier")),
        // `case Point() as p`: in a pattern the alias is a bare identifier.
        "as_pattern" if n.parent().is_some_and(|p| p.kind() == "case_pattern") => {
            out.extend(named_children(n).into_iter().last().filter(|c| c.kind() == "identifier"))
        }
        _ => {}
    });
    out
}

/// `global name` / `nonlocal name` in a function body.
fn declared(body: Node, stmt: &str, name: &str, src: &str) -> bool {
    let mut found = false;
    walk_scope(body, &mut |n| {
        if n.kind() == stmt && named_children(n).iter().any(|c| text(*c, src) == name) {
            found = true;
        }
    });
    found
}

fn comprehension_targets(c: Node) -> Vec<Node> {
    let mut out = Vec::new();
    for clause in named_children(c).into_iter().filter(|k| k.kind() == "for_in_clause") {
        if let Some(l) = field(clause, "left") {
            targets(l, &mut out);
        }
    }
    out
}

fn find_in_scopes<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let hit = |nodes: Vec<Node<'t>>| nodes.into_iter().find(|n| text(*n, src) == name);
    let mut prev = from;
    let mut cur = from.parent();
    // Class bodies are not visible from functions nested in them.
    let mut in_function = false;
    // `global name`: straight to the module.
    let mut global = false;
    while let Some(s) = cur {
        let from_body = field(s, "body").is_some_and(|b| b.id() == prev.id());
        let found = match s.kind() {
            "module" => hit(bindings(s)),
            _ if global => None,
            // Defaults and annotations are evaluated outside the function.
            "function_definition" if from_body => {
                in_function = true;
                let body = field(s, "body");
                if body.is_some_and(|b| declared(b, "global_statement", name, src)) {
                    global = true;
                    None
                } else if body.is_some_and(|b| declared(b, "nonlocal_statement", name, src)) {
                    None
                } else {
                    hit(function_params(s)).or_else(|| hit(bindings(body?)))
                }
            }
            "lambda" if from_body => {
                in_function = true;
                hit(function_params(s))
            }
            k if is_comprehension(k) => {
                in_function = true;
                hit(comprehension_targets(s))
            }
            "class_definition" if from_body && !in_function => hit(bindings(field(s, "body")?)),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        prev = s;
        cur = s.parent();
    }
    None
}

fn is_declaration_name(ident: Node) -> bool {
    let Some(p) = ident.parent() else { return false };
    match p.kind() {
        "class_definition" | "function_definition" | "default_parameter" | "typed_default_parameter" => {
            is_field(p, "name", ident)
        }
        "parameters" | "lambda_parameters" => true,
        "typed_parameter" => p.named_child(0).is_some_and(|c| c.id() == ident.id()),
        "list_splat_pattern" | "dictionary_splat_pattern" => p
            .parent()
            .is_some_and(|g| matches!(g.kind(), "parameters" | "lambda_parameters" | "typed_parameter")),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

fn supertypes(class: Node, src: &str) -> Vec<String> {
    let Some(args) = field(class, "superclasses") else { return Vec::new() };
    named_children(args)
        .into_iter()
        .filter(|a| a.kind() != "keyword_argument")
        .filter_map(|a| ann_type(a, src))
        .collect()
}

/// Functions directly in a class body, with their decorated wrappers.
fn methods(class: Node) -> Vec<(Node, Option<Node>)> {
    let Some(body) = field(class, "body") else { return Vec::new() };
    named_children(body)
        .into_iter()
        .filter_map(|m| match m.kind() {
            "function_definition" => Some((m, None)),
            "decorated_definition" => field(m, "definition")
                .filter(|d| d.kind() == "function_definition")
                .map(|d| (d, Some(m))),
            _ => None,
        })
        .collect()
}

fn summarize(root: Node, src: &str) -> Summary {
    let mut s = Summary::default();
    collect_imports(root, src, &mut s.imports);
    collect_defs(bindings(root), src, None, &mut s.defs);
    s
}

/// Every import in the file, at any depth: a function-level import still
/// names the same thing.
fn collect_imports(n: Node, src: &str, out: &mut Vec<Import>) {
    match n.kind() {
        "import_statement" => {
            let mut c = n.walk();
            for name in n.children_by_field_name("name", &mut c) {
                match name.kind() {
                    // `import a.b` binds `a`; `a.b` is reached as a member.
                    "dotted_name" => {
                        let first = name.named_child(0).map_or_else(|| dotted(name, src), |f| text(f, src).to_string());
                        out.push(Import { path: first, star: false, alias: None });
                    }
                    "aliased_import" => out.push(Import {
                        path: field(name, "name").map(|d| dotted(d, src)).unwrap_or_default(),
                        star: false,
                        alias: field(name, "alias").map(|a| text(a, src).to_string()),
                    }),
                    _ => {}
                }
            }
        }
        "import_from_statement" => {
            let module = field(n, "module_name").map(|m| dotted(m, src)).unwrap_or_default();
            if child_of_kind(n, "wildcard_import").is_some() {
                out.push(Import { path: module.clone(), star: true, alias: None });
            }
            let mut c = n.walk();
            for name in n.children_by_field_name("name", &mut c) {
                let (path, alias) = match name.kind() {
                    "dotted_name" => (dotted(name, src), None),
                    "aliased_import" => (
                        field(name, "name").map(|d| dotted(d, src)).unwrap_or_default(),
                        field(name, "alias").map(|a| text(a, src).to_string()),
                    ),
                    _ => continue,
                };
                out.push(Import { path: join(&module, &path), star: false, alias });
            }
        }
        _ => {
            for c in named_children(n) {
                collect_imports(c, src, out);
            }
        }
    }
}

/// Declarations for the first binding of each name in `names` (the scope
/// walk resolves every use to that one too), plus class members.
fn collect_defs(names: Vec<Node>, src: &str, container: Option<&str>, out: &mut Vec<Def>) {
    let mut seen = HashSet::new();
    for name in names {
        let t = text(name, src);
        if !seen.insert(t) {
            continue;
        }
        let def = |kind, type_name, supertypes| Def {
            name: t.to_string(),
            kind,
            start: name.start_byte(),
            end: name.end_byte(),
            container: container.map(str::to_string),
            type_name,
            supertypes,
        };
        let Some(p) = name.parent() else { continue };
        match p.kind() {
            "class_definition" => {
                out.push(def(DefKind::Class, Some(t.to_string()), supertypes(p, src)));
                collect_members(p, t, src, out);
            }
            "function_definition" => {
                let property = method_class(p)
                    .is_some_and(|(_, d)| has_decorator(d, &["property", "cached_property"], src));
                let kind = if property { DefKind::Property } else { DefKind::Function };
                out.push(def(kind, decl_type_name(name, src), Vec::new()));
            }
            _ => {
                let ty = decl_type_name(name, src).or_else(|| decl_initializer(name).and_then(|v| constructed_type(v, src)));
                out.push(def(DefKind::Property, ty, Vec::new()));
            }
        }
    }
}

/// A class's body names, then its instance attributes: `self.x = ...` in
/// any method, unless the body already declares `x`.
fn collect_members(class: Node, cname: &str, src: &str, out: &mut Vec<Def>) {
    let Some(body) = field(class, "body") else { return };
    let names = bindings(body);
    let mut seen: HashSet<String> = names.iter().map(|n| text(*n, src).to_string()).collect();
    collect_defs(names, src, Some(cname), out);
    for (func, decorated) in methods(class) {
        if has_decorator(decorated, &["staticmethod"], src) {
            continue;
        }
        let params = function_params(func);
        let Some(this) = params.first().filter(|p| p.kind() == "identifier") else { continue };
        let this = text(*this, src);
        let Some(fbody) = field(func, "body") else { continue };
        let mut attrs = Vec::new();
        walk_scope(fbody, &mut |n| {
            if !matches!(n.kind(), "assignment" | "augmented_assignment") {
                return;
            }
            let mut stack: Vec<Node> = field(n, "left").into_iter().collect();
            while let Some(t) = stack.pop() {
                match t.kind() {
                    "attribute" => {
                        let obj = field(t, "object").filter(|o| o.kind() == "identifier");
                        if obj.is_some_and(|o| text(o, src) == this) {
                            attrs.extend(field(t, "attribute"));
                        }
                    }
                    "pattern_list" | "tuple_pattern" | "list_pattern" | "tuple" | "list" => {
                        stack.extend(named_children(t).into_iter().rev())
                    }
                    _ => {}
                }
            }
        });
        attrs.sort_by_key(|a| a.start_byte());
        for attr in attrs {
            let t = text(attr, src);
            if !seen.insert(t.to_string()) {
                continue;
            }
            let ty = decl_type_name(attr, src).or_else(|| {
                let v = decl_initializer(attr)?;
                // `self.repo = repo` with `repo: Repo` a parameter.
                if v.kind() == "identifier" {
                    let p = params.iter().find(|p| text(**p, src) == text(v, src))?;
                    return decl_type_name(*p, src);
                }
                constructed_type(v, src)
            });
            out.push(Def {
                name: t.to_string(),
                kind: DefKind::Property,
                start: attr.start_byte(),
                end: attr.end_byte(),
                container: Some(cname.to_string()),
                type_name: ty,
                supertypes: Vec::new(),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------

/// The dotted module name of a file. Inside a package (a directory with
/// `__init__.py`) it is the package chain (`src/pkg/sub/mod.py` ->
/// `pkg.sub.mod`). Otherwise it is the path under the workspace root, minus
/// a leading `src` (namespace packages, scripts).
pub fn module_name(path: &Path, roots: &[PathBuf]) -> String {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { return String::new() };
    let mut dirs: Vec<String> = Vec::new();
    let in_package = |d: &Path| d.join("__init__.py").is_file();
    let parent = path.parent();
    if parent.is_some_and(in_package) {
        let mut dir = parent;
        while let Some(d) = dir.filter(|d| in_package(d)) {
            dirs.extend(d.file_name().and_then(|n| n.to_str()).map(str::to_string));
            dir = d.parent();
        }
        dirs.reverse();
    } else if let Some(root) = roots.iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.components().count()) {
        let rel = parent.and_then(|p| p.strip_prefix(root).ok());
        dirs = rel
            .into_iter()
            .flat_map(|r| r.components())
            .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
            .collect();
        if dirs.first().is_some_and(|d| d == "src") {
            dirs.remove(0);
        }
    }
    if stem != "__init__" {
        dirs.push(stem.to_string());
    }
    dirs.join(".")
}

/// `.models` in `pkg/mod.py` is `pkg.models`; `..x` goes one package up. In
/// `pkg/__init__.py`, `.` is `pkg` itself.
fn absolute(path: &str, file: &Path, module: &str) -> String {
    let dots = path.bytes().take_while(|b| *b == b'.').count();
    if dots == 0 {
        return path.to_string();
    }
    let mut base: Vec<&str> = module.split('.').filter(|p| !p.is_empty()).collect();
    if file.file_stem().and_then(|s| s.to_str()) != Some("__init__") {
        base.pop();
    }
    for _ in 1..dots {
        base.pop();
    }
    let rest = &path[dots..];
    if !rest.is_empty() {
        base.push(rest);
    }
    base.join(".")
}

/// For a name in an import, the import path up to and including it.
fn qualified_path(ident: Node, src: &str) -> Option<String> {
    let p = ident.parent()?;
    // `import a.b as m` / `from x import y as m`: `m` is the whole path.
    if p.kind() == "aliased_import" && is_field(p, "alias", ident) {
        let name = dotted(field(p, "name")?, src);
        return in_statement(p.parent()?, &name, src);
    }
    if p.kind() != "dotted_name" {
        return None;
    }
    let upto: String = src[p.start_byte()..ident.end_byte()].split_whitespace().collect();
    let g = p.parent()?;
    match g.kind() {
        "relative_import" => {
            let prefix = child_of_kind(g, "import_prefix").map_or("", |n| text(n, src));
            Some(format!("{prefix}{upto}"))
        }
        "aliased_import" => in_statement(g.parent()?, &upto, src),
        "import_from_statement" if is_field(g, "module_name", p) => Some(upto),
        "import_statement" | "import_from_statement" => in_statement(g, &upto, src),
        _ => None,
    }
}

/// `name` imported by `stmt`, as an absolute-or-relative path.
fn in_statement(stmt: Node, name: &str, src: &str) -> Option<String> {
    match stmt.kind() {
        "import_statement" => Some(name.to_string()),
        "import_from_statement" => Some(join(&field(stmt, "module_name").map(|m| dotted(m, src))?, name)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The Language impl
// ---------------------------------------------------------------------------

impl Language for Python {
    fn name(&self) -> &'static str {
        "Python"
    }

    fn grammar(&self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["py", "pyi"]
    }

    fn family(&self) -> &'static str {
        "python"
    }

    fn visibility(&self) -> Visibility {
        Visibility::Modules
    }

    fn summarize(&self, root: Node, src: &str) -> Summary {
        summarize(root, src)
    }

    fn place(&self, summary: &mut Summary, path: &Path, roots: &[PathBuf]) {
        summary.package = module_name(path, roots);
        for imp in &mut summary.imports {
            imp.path = absolute(&imp.path, path, &summary.package);
        }
    }

    fn absolute_import(&self, path: &str, file: &Path, summary: &Summary) -> String {
        absolute(path, file, &summary.package)
    }

    fn name_node<'t>(&self, root: Node<'t>, _src: &str, start: usize, end: usize) -> Option<Node<'t>> {
        let node = root.descendant_for_byte_range(start, end)?;
        (self.is_identifier(node) && node.start_byte() == start && node.end_byte() == end).then_some(node)
    }

    fn is_identifier(&self, n: Node) -> bool {
        n.kind() == "identifier"
    }

    fn is_declaration_name(&self, ident: Node) -> bool {
        is_declaration_name(ident)
    }

    fn named_argument_call<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        let p = ident.parent()?;
        if p.kind() != "keyword_argument" || !is_field(p, "name", ident) {
            return None;
        }
        let call = p.parent()?.parent()?;
        (call.kind() == "call").then_some(call)
    }

    fn member_receiver<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        let p = ident.parent()?;
        (p.kind() == "attribute" && is_field(p, "attribute", ident))
            .then(|| field(p, "object"))
            .flatten()
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

    fn classify_expr<'t>(&self, expr: Node<'t>, src: &str) -> Expr<'t> {
        if is_literal(expr.kind()) {
            return Expr::Literal;
        }
        match expr.kind() {
            "identifier" => Expr::Name(expr),
            "call" => match field(expr, "function") {
                Some(f) if f.kind() == "identifier" && text(f, src) == "super" => Expr::Super,
                Some(f) => Expr::Call(f),
                None => Expr::Unknown,
            },
            "attribute" => field(expr, "attribute").map_or(Expr::Unknown, Expr::Member),
            "parenthesized_expression" | "await" | "conditional_expression" => {
                expr.named_child(0).map_or(Expr::Unknown, Expr::Wrapper)
            }
            "named_expression" => field(expr, "value").map_or(Expr::Unknown, Expr::Wrapper),
            _ => Expr::Unknown,
        }
    }

    fn callee_name<'t>(&self, call: Node<'t>) -> Option<Node<'t>> {
        let f = field(call, "function")?;
        match f.kind() {
            "identifier" => Some(f),
            "attribute" => field(f, "attribute"),
            _ => None,
        }
    }

    fn params_of<'t>(&self, decl_name: Node<'t>, src: &str) -> Vec<Node<'t>> {
        let Some(decl) = decl_name.parent() else { return Vec::new() };
        match decl.kind() {
            "function_definition" => function_params(decl),
            // `Foo(x=1)`: `__init__`'s parameters, or a dataclass's fields.
            "class_definition" => {
                let init = methods(decl).into_iter().map(|(f, _)| f).find(|f| {
                    field(*f, "name").is_some_and(|n| text(n, src) == "__init__")
                });
                match init {
                    Some(f) => function_params(f).into_iter().skip(1).collect(),
                    None => field(decl, "body")
                        .map(|b| {
                            named_children(b)
                                .into_iter()
                                .filter_map(|st| st.named_child(0).filter(|a| a.kind() == "assignment"))
                                .filter(|a| field(*a, "type").is_some())
                                .filter_map(|a| field(a, "left").filter(|l| l.kind() == "identifier"))
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn enclosing_class<'t>(&self, n: Node<'t>) -> Option<Node<'t>> {
        let mut cur = n.parent();
        while let Some(p) = cur {
            if p.kind() == "class_definition" {
                return Some(p);
            }
            cur = p.parent();
        }
        None
    }

    fn class_name(&self, class: Node, src: &str) -> Option<String> {
        field(class, "name").map(|n| text(n, src).to_string())
    }

    fn class_supertypes(&self, class: Node, src: &str) -> Vec<String> {
        supertypes(class, src)
    }
}
