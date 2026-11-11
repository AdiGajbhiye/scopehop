//! TypeScript and JavaScript on top of tree-sitter-typescript.
//!
//! ES modules fit [`Visibility::Modules`]: every file is a module, and a
//! name is the file's own or imported.
//!
//! - A module is named by its absolute path without the extension, and an
//!   `index` file names its directory, so `"./models"`, `"./models/index"`
//!   and `"./models/index.js"` all reach `models/index.ts`.
//! - Import specifiers are relative paths, `paths` or `baseUrl` mappings
//!   from the nearest `tsconfig.json` or `jsconfig.json` ([`resolve_spec`]),
//!   or package names, which are not indexed. `import { a as b }`, default
//!   imports, `import * as ns`, re-exports (`export { a } from`, `export *
//!   from`) and top-level `require` are imports. A default export is a
//!   re-export of the declaration under the name `default`.
//! - Scopes are blocks (`let`, `const`, classes, functions), functions (
//!   parameters, hoisted `var`), loops, `catch` and type parameters.
//! - Classes, interfaces, enums and object types have members; a class's
//!   `extends` and `implements` and an interface's `extends` are
//!   supertypes. Object literals and inline object types are indexed as
//!   anonymous types named after their position (`object@<offset>/<file
//!   length>`), so `config.port` finds the `port` key.
//! - Type names are kept as written (`ns.Shape`). `Promise<T>` is read as
//!   `T`, since `await` gives the `T` and a promise has no project members.
//!   Arrays and other iterables are written `T[]`, for `for...of` and
//!   indexing.
//!
//! JavaScript files are parsed with the TSX grammar, which reads plain
//! JavaScript and JSX.

use std::path::{Component, Path, PathBuf};

use tree_sitter::Node;

use crate::lang::{named_children, text, Def, DefKind, Expr, Import, Language, Scoped, Summary, Visibility};

/// `.ts`, `.mts`, `.cts`.
pub struct TypeScript;
/// `.tsx` and JavaScript.
pub struct Tsx;

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
        "string"
            | "template_string"
            | "number"
            | "true"
            | "false"
            | "null"
            | "undefined"
            | "regex"
            | "array"
            | "object"
            | "binary_expression"
            | "unary_expression"
            | "update_expression"
            | "predefined_type"
            | "literal_type"
    )
}

fn is_function(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
            | "function_signature"
            | "method_signature"
            | "abstract_method_signature"
    )
}

fn is_class(kind: &str) -> bool {
    matches!(kind, "class_declaration" | "abstract_class_declaration" | "class")
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A type as a name the index can look up: `Circle`, `ns.Circle`, `T[]`
/// for arrays and iterables, `Record<K, V>`, or an anonymous object type's
/// name. `None` when nothing in the project could have its members.
fn type_str(n: Node, src: &str) -> Option<String> {
    match n.kind() {
        "type_annotation" | "parenthesized_type" | "readonly_type" | "opting_type_annotation" => {
            type_str(n.named_child(0)?, src)
        }
        "type_identifier" | "identifier" | "predefined_type" => Some(text(n, src).to_string()),
        "nested_type_identifier" | "member_expression" => {
            Some(text(n, src).chars().filter(|c| !c.is_whitespace()).collect())
        }
        "generic_type" => {
            let name = type_str(field(n, "name")?, src)?;
            let args: Vec<Node> = field(n, "type_arguments").map(named_children).unwrap_or_default();
            let arg = |i: usize| args.get(i).and_then(|a| type_str(*a, src));
            match name.as_str() {
                "Promise" | "PromiseLike" | "Awaited" => arg(0),
                "Array" | "ReadonlyArray" | "Set" | "ReadonlySet" | "Iterable" | "IterableIterator"
                | "AsyncIterable" | "AsyncIterableIterator" | "Generator" | "AsyncGenerator" => {
                    Some(format!("{}[]", arg(0)?))
                }
                "Record" => Some(format!("Record<{}, {}>", arg(0).unwrap_or_default(), arg(1)?)),
                _ => Some(name),
            }
        }
        "array_type" => Some(format!("{}[]", type_str(n.named_child(0)?, src)?)),
        // `T | null | undefined` is a T, and so is `T | Promise<T>`.
        "union_type" => {
            let mut parts = Vec::new();
            union_parts(n, &mut parts);
            let mut names = parts.into_iter().filter(|p| !is_nullish(*p, src)).map(|p| type_str(p, src));
            let first = names.next()??;
            names.all(|t| t.as_deref() == Some(first.as_str())).then_some(first)
        }
        // A function-typed value reads as what calling it gives, as a
        // function's declaration does.
        "function_type" => type_str(field(n, "return_type")?, src),
        "object_type" => Some(anon_type(n, src)),
        _ => None,
    }
}

fn union_parts<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
    for c in named_children(n) {
        if c.kind() == "union_type" {
            union_parts(c, out);
        } else {
            out.push(c);
        }
    }
}

fn is_nullish(n: Node, src: &str) -> bool {
    matches!(text(n, src), "null" | "undefined" | "void")
}

/// What `for...of` or indexing gives for a container type from
/// [`type_str`].
fn container_element(t: &str, _first: bool) -> Option<String> {
    if let Some(elem) = t.strip_suffix("[]") {
        return (!elem.is_empty()).then(|| elem.to_string());
    }
    let args = t.strip_prefix("Record<")?.strip_suffix('>')?;
    let mut depth = 0;
    for (i, c) in args.char_indices() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => return Some(args[i + 1..].trim().to_string()).filter(|v| !v.is_empty()),
            _ => {}
        }
    }
    None
}

/// The name an object literal or inline object type is indexed under. No
/// identifier can spell it, and the source length keeps files apart.
fn anon_type(n: Node, src: &str) -> String {
    format!("object@{}/{}", n.start_byte(), src.len())
}

/// The return type of a function-like node.
fn return_type(f: Node, src: &str) -> Option<String> {
    if let Some(t) = field(f, "return_type") {
        return type_str(t, src);
    }
    // `() => new Circle()`
    let body = field(f, "body").filter(|b| b.kind() != "statement_block")?;
    constructed_type(body, src)
}

/// The type of a value that says it outright: `new T()`, `{...}`, `x as
/// T`, or a function's return type.
fn constructed_type(value: Node, src: &str) -> Option<String> {
    match value.kind() {
        "new_expression" => {
            let c = field(value, "constructor")?;
            matches!(c.kind(), "identifier" | "member_expression").then(|| type_str(c, src))?
        }
        "object" => Some(anon_type(value, src)),
        "as_expression" | "type_assertion" => {
            let t = named_children(value).into_iter().find(|c| c.kind() != "as" && is_type_node(c.kind()))?;
            type_str(t, src)
        }
        "satisfies_expression" | "parenthesized_expression" | "non_null_expression" => {
            constructed_type(value.named_child(0)?, src)
        }
        "arrow_function" | "function_expression" | "generator_function" => return_type(value, src),
        _ => None,
    }
}

fn is_type_node(kind: &str) -> bool {
    matches!(
        kind,
        "type_identifier"
            | "generic_type"
            | "nested_type_identifier"
            | "predefined_type"
            | "array_type"
            | "union_type"
            | "object_type"
            | "parenthesized_type"
            | "readonly_type"
    )
}

/// The written type of a parameter, variable, field or property.
fn annotation(decl: Node, src: &str) -> Option<String> {
    type_str(field(decl, "type")?, src)
}

/// The node that owns a declared name: the declarator, parameter, loop or
/// `catch` it binds in, looking through destructuring patterns.
fn binding_owner(ident: Node) -> Option<Node> {
    let mut cur = ident;
    loop {
        let p = cur.parent()?;
        match p.kind() {
            "object_pattern" | "array_pattern" | "rest_pattern" => {}
            "pair_pattern" if is_field(p, "value", cur) => {}
            "assignment_pattern" | "object_assignment_pattern" if is_field(p, "left", cur) => {}
            "variable_declarator" if is_field(p, "name", cur) => return Some(p),
            "required_parameter" | "optional_parameter" if is_field(p, "pattern", cur) => return Some(p),
            "for_in_statement" if is_field(p, "left", cur) && field(p, "kind").is_some() => return Some(p),
            "catch_clause" if is_field(p, "parameter", cur) => return Some(p),
            "arrow_function" if is_field(p, "parameter", cur) => return Some(p),
            _ => return None,
        }
        cur = p;
    }
}

fn decl_type_name(name: Node, src: &str) -> Option<String> {
    let p = name.parent()?;
    match p.kind() {
        k if (is_class(k) || matches!(k, "interface_declaration" | "type_alias_declaration" | "enum_declaration"))
            && is_field(p, "name", name) =>
        {
            Some(text(name, src).to_string())
        }
        k if is_function(k) && is_field(p, "name", name) => return_type(p, src),
        "variable_declarator" if is_field(p, "name", name) => {
            annotation(p, src).or_else(|| constructed_type(field(p, "value")?, src))
        }
        "public_field_definition" | "property_signature" if is_field(p, "name", name) => {
            annotation(p, src).or_else(|| constructed_type(field(p, "value")?, src))
        }
        "pair" if is_field(p, "key", name) => constructed_type(field(p, "value")?, src),
        "required_parameter" | "optional_parameter" if is_field(p, "pattern", name) => annotation(p, src),
        // `...rest: T[]`
        "rest_pattern" => {
            let param = p.parent().filter(|g| matches!(g.kind(), "required_parameter" | "optional_parameter"))?;
            annotation(param, src)
        }
        "for_in_statement" if is_field(p, "left", name) && !has_token(p, "of") => Some("string".into()),
        _ => None,
    }
}

fn decl_initializer(name: Node) -> Option<Node> {
    let p = name.parent()?;
    match p.kind() {
        "variable_declarator" if is_field(p, "name", name) => field(p, "value"),
        "public_field_definition" if is_field(p, "name", name) => field(p, "value"),
        "pair" if is_field(p, "key", name) => field(p, "value"),
        "required_parameter" | "optional_parameter" if is_field(p, "pattern", name) => field(p, "value"),
        _ => None,
    }
}

/// The type an object literal is written against: `const p: Point = {...}`,
/// `{...} as Point`, `{...} satisfies Point`, an element of `const ps:
/// Point[] = [{...}]`, or a function's declared return type. Returns the
/// type's name identifier.
fn contextual_type(object: Node) -> Option<Node> {
    let p = object.parent()?;
    let t = match p.kind() {
        "variable_declarator" if is_field(p, "value", object) => field(p, "type")?.named_child(0)?,
        "as_expression" | "satisfies_expression" => {
            named_children(p).into_iter().find(|c| c.id() != object.id() && is_type_node(c.kind()))?
        }
        "array" => {
            let decl = p.parent().filter(|d| d.kind() == "variable_declarator")?;
            let t = field(decl, "type")?.named_child(0)?;
            match t.kind() {
                "array_type" => t.named_child(0)?,
                "generic_type" => field(t, "type_arguments")?.named_child(0)?,
                _ => return None,
            }
        }
        "return_statement" => {
            let f = ancestors(p).find(|a| is_function(a.kind()))?;
            field(f, "return_type")?.named_child(0)?
        }
        "parenthesized_expression" => return contextual_type(p),
        _ => return None,
    };
    named_type(t)
}

/// The identifier that names a type node: `Point` in `Point`, `ns.Point`,
/// `Point<T>`, `Point | null`.
fn named_type(t: Node) -> Option<Node> {
    match t.kind() {
        "type_identifier" => Some(t),
        "nested_type_identifier" | "generic_type" => named_type(field(t, "name")?),
        "union_type" => {
            let mut parts = Vec::new();
            union_parts(t, &mut parts);
            let real: Vec<Node> = parts.into_iter().filter(|p| p.kind() != "literal_type").collect();
            match real.as_slice() {
                [only] => named_type(*only),
                _ => None,
            }
        }
        "parenthesized_type" => named_type(t.named_child(0)?),
        _ => None,
    }
}

fn ancestors(n: Node) -> impl Iterator<Item = Node> {
    std::iter::successors(n.parent(), |p| p.parent())
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

/// Names a destructuring pattern (or plain identifier) declares.
fn pattern_names<'t>(p: Node<'t>, out: &mut Vec<Node<'t>>) {
    match p.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(p),
        "object_pattern" | "array_pattern" | "rest_pattern" => {
            for c in named_children(p) {
                pattern_names(c, out);
            }
        }
        "pair_pattern" => {
            if let Some(v) = field(p, "value") {
                pattern_names(v, out);
            }
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(l) = field(p, "left") {
                pattern_names(l, out);
            }
        }
        _ => {}
    }
}

fn declarator_names<'t>(decl: Node<'t>, out: &mut Vec<Node<'t>>) {
    for d in named_children(decl).into_iter().filter(|d| d.kind() == "variable_declarator") {
        if let Some(n) = field(d, "name") {
            pattern_names(n, out);
        }
    }
}

/// Names a statement declares in the block it is in.
fn declared_by<'t>(stmt: Node<'t>, out: &mut Vec<Node<'t>>) {
    match stmt.kind() {
        "lexical_declaration" | "variable_declaration" => declarator_names(stmt, out),
        "function_declaration"
        | "generator_function_declaration"
        | "function_signature"
        | "class_declaration"
        | "abstract_class_declaration"
        | "enum_declaration"
        | "interface_declaration"
        | "type_alias_declaration" => out.extend(field(stmt, "name")),
        "export_statement" => {
            if let Some(d) = field(stmt, "declaration") {
                declared_by(d, out);
            }
        }
        _ => {}
    }
}

/// `var` declarations anywhere in a function body, outside nested
/// functions: they belong to the function.
fn hoisted_vars<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
    for c in named_children(n) {
        if is_function(c.kind()) || is_class(c.kind()) {
            continue;
        }
        if c.kind() == "variable_declaration" {
            declarator_names(c, out);
        }
        hoisted_vars(c, out);
    }
}

fn params<'t>(f: Node<'t>, out: &mut Vec<Node<'t>>) {
    if let Some(p) = field(f, "parameter") {
        out.push(p);
    }
    for p in field(f, "parameters").map(named_children).unwrap_or_default() {
        if let Some(pat) = field(p, "pattern") {
            pattern_names(pat, out);
        }
    }
}

fn type_params<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
    for tp in field(n, "type_parameters").map(named_children).unwrap_or_default() {
        out.extend(field(tp, "name"));
    }
}

/// The names `infer X` declares in a conditional type's `extends` side.
fn infer_names<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
    for c in named_children(n) {
        if c.kind() == "infer_type" {
            out.extend(c.named_child(0).filter(|t| t.kind() == "type_identifier"));
        } else {
            infer_names(c, out);
        }
    }
}

fn find_label<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    ancestors(from)
        .take_while(|a| !is_function(a.kind()))
        .filter(|a| a.kind() == "labeled_statement")
        .find_map(|a| field(a, "label").filter(|l| text(*l, src) == name))
}

fn find_in_scopes<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    if from.kind() == "statement_identifier" {
        return find_label(from, name, src);
    }
    let pick = |cands: Vec<Node<'t>>| cands.into_iter().find(|c| text(*c, src) == name);
    for n in ancestors(from) {
        let mut cands = Vec::new();
        match n.kind() {
            "program" => return None,
            "statement_block" | "switch_body" | "class_static_block" => {
                for s in named_children(n) {
                    declared_by(s, &mut cands);
                }
                if n.kind() == "switch_body" {
                    for case in named_children(n) {
                        for s in named_children(case) {
                            declared_by(s, &mut cands);
                        }
                    }
                }
            }
            "for_statement" => cands.extend(field(n, "initializer").into_iter().flat_map(|i| {
                let mut v = Vec::new();
                declarator_names(i, &mut v);
                v
            })),
            "for_in_statement" if field(n, "kind").is_some() => {
                if let Some(l) = field(n, "left") {
                    pattern_names(l, &mut cands);
                }
            }
            "catch_clause" => {
                if let Some(p) = field(n, "parameter") {
                    pattern_names(p, &mut cands);
                }
            }
            k if is_function(k) => {
                params(n, &mut cands);
                type_params(n, &mut cands);
                // A named function expression sees its own name.
                if matches!(k, "function_expression" | "generator_function") {
                    cands.extend(field(n, "name"));
                }
                if let Some(b) = field(n, "body") {
                    hoisted_vars(b, &mut cands);
                }
            }
            k if is_class(k) => {
                type_params(n, &mut cands);
                if k == "class" {
                    cands.extend(field(n, "name"));
                }
            }
            "interface_declaration" | "type_alias_declaration" => type_params(n, &mut cands),
            "call_signature" | "construct_signature" | "function_type" | "constructor_type" => {
                type_params(n, &mut cands);
                params(n, &mut cands);
            }
            // `T extends { a: infer U } ? U : never`: `U` in the true branch.
            "conditional_type" if field(n, "consequence").is_some_and(|c| c.byte_range().contains(&from.start_byte())) => {
                if let Some(r) = field(n, "right") {
                    infer_names(r, &mut cands);
                }
            }
            // `{ [K in keyof T]: T[K] }`
            "index_signature" => {
                for c in named_children(n).into_iter().filter(|c| c.kind() == "mapped_type_clause") {
                    cands.extend(field(c, "name"));
                }
            }
            _ => {}
        }
        if let Some(d) = pick(cands) {
            return Some(d);
        }
    }
    None
}

fn is_declaration_name(ident: Node) -> bool {
    let Some(p) = ident.parent() else { return false };
    match p.kind() {
        k if (is_function(k)
            || is_class(k)
            || matches!(
                k,
                "interface_declaration"
                    | "type_alias_declaration"
                    | "enum_declaration"
                    | "public_field_definition"
                    | "property_signature"
                    | "type_parameter"
                    | "internal_module"
                    | "enum_assignment"
            ))
            && is_field(p, "name", ident) =>
        {
            true
        }
        "enum_body" => true,
        "infer_type" => true,
        "mapped_type_clause" => is_field(p, "name", ident),
        "labeled_statement" => is_field(p, "label", ident),
        // `export { a as b }`: `b` is the exported name.
        "export_specifier" => is_field(p, "alias", ident),
        // An object literal key is a member of the literal's own type
        // unless the literal is written against one.
        "pair" => is_field(p, "key", ident) && p.parent().and_then(contextual_type).is_none(),
        // `{ port }` uses `port`.
        "object" => false,
        _ => ident.kind() != "shorthand_property_identifier" && binding_owner(ident).is_some(),
    }
}

// ---------------------------------------------------------------------------
// Summaries
// ---------------------------------------------------------------------------

/// Before [`Language::place`], an import path is a specifier and a member
/// joined by this, since a specifier can contain dots.
const SEP: char = '\0';

fn encode(spec: &str, member: Option<&str>) -> String {
    format!("{spec}{SEP}{}", member.unwrap_or(""))
}

fn import_source(stmt: Node, src: &str) -> Option<String> {
    let s = field(stmt, "source")?;
    Some(text(s, src).trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string())
}

fn collect_import(stmt: Node, src: &str, out: &mut Vec<Import>) {
    let Some(spec) = import_source(stmt, src) else { return };
    let Some(clause) = named_children(stmt).into_iter().find(|c| c.kind() == "import_clause") else { return };
    for c in named_children(clause) {
        match c.kind() {
            "identifier" => out.push(Import {
                path: encode(&spec, Some("default")),
                star: false,
                alias: Some(text(c, src).to_string()),
            }),
            "namespace_import" => out.push(Import {
                path: encode(&spec, None),
                star: false,
                alias: c.named_child(0).map(|i| text(i, src).to_string()),
            }),
            "named_imports" => {
                for s in named_children(c).into_iter().filter(|s| s.kind() == "import_specifier") {
                    let Some(name) = field(s, "name") else { continue };
                    out.push(Import {
                        path: encode(&spec, Some(text(name, src))),
                        star: false,
                        alias: field(s, "alias").map(|a| text(a, src).to_string()),
                    });
                }
            }
            _ => {}
        }
    }
}

/// `export ... from`, `export { a as b }` and `export default x`.
fn collect_export(stmt: Node, src: &str, imports: &mut Vec<Import>, defs: &mut Vec<Def>) {
    let spec = import_source(stmt, src);
    let default = has_token(stmt, "default");
    if let Some(d) = field(stmt, "declaration") {
        declaration(d, src, None, defs);
        if default && let Some(name) = field(d, "name") {
            imports.push(Import { path: encode("", Some(text(name, src))), star: false, alias: Some("default".into()) });
        }
        return;
    }
    if let Some(v) = field(stmt, "value") {
        if default && v.kind() == "identifier" {
            imports.push(Import { path: encode("", Some(text(v, src))), star: false, alias: Some("default".into()) });
        }
        return;
    }
    let clause = named_children(stmt).into_iter().find(|c| matches!(c.kind(), "export_clause" | "namespace_export"));
    match (clause, spec) {
        (None, Some(spec)) => imports.push(Import { path: encode(&spec, None), star: true, alias: None }),
        (Some(ns), Some(spec)) if ns.kind() == "namespace_export" => imports.push(Import {
            path: encode(&spec, None),
            star: false,
            alias: ns.named_child(0).map(|n| text(n, src).to_string()),
        }),
        (Some(clause), spec) => {
            for s in named_children(clause).into_iter().filter(|s| s.kind() == "export_specifier") {
                let Some(name) = field(s, "name") else { continue };
                let alias = field(s, "alias").map(|a| text(a, src).to_string());
                // `export { a }` of a local adds nothing.
                if spec.is_none() && alias.is_none() {
                    continue;
                }
                imports.push(Import {
                    path: encode(spec.as_deref().unwrap_or(""), Some(text(name, src))),
                    star: false,
                    alias,
                });
            }
        }
        _ => {}
    }
}

/// `const x = require("./m")` and `const { a, b: c } = require("./m")`.
fn collect_require(decl: Node, src: &str, imports: &mut Vec<Import>) {
    for d in named_children(decl).into_iter().filter(|d| d.kind() == "variable_declarator") {
        let Some(spec) = field(d, "value").and_then(|v| require_spec(v, src)) else { continue };
        let Some(name) = field(d, "name") else { continue };
        match name.kind() {
            "identifier" => {
                imports.push(Import { path: encode(&spec, None), star: false, alias: Some(text(name, src).to_string()) })
            }
            "object_pattern" => {
                for p in named_children(name) {
                    let (member, alias) = match p.kind() {
                        "shorthand_property_identifier_pattern" => (p, None),
                        "pair_pattern" => match (field(p, "key"), field(p, "value")) {
                            (Some(k), Some(v)) if v.kind() == "identifier" => (k, Some(text(v, src).to_string())),
                            _ => continue,
                        },
                        _ => continue,
                    };
                    imports.push(Import { path: encode(&spec, Some(text(member, src))), star: false, alias });
                }
            }
            _ => {}
        }
    }
}

/// `x` in `const x = require("m")`, or `a` / `b` in `const { a, k: b } =
/// require("m")`: the module, or the member it takes.
fn required_name(n: Node, src: &str) -> Option<String> {
    let p = n.parent()?;
    let (member, decl) = match p.kind() {
        "variable_declarator" if is_field(p, "name", n) => (None, p),
        "object_pattern" if n.kind() == "shorthand_property_identifier_pattern" => (Some(n), p.parent()?),
        "pair_pattern" => (Some(field(p, "key")?), p.parent()?.parent()?),
        _ => return None,
    };
    (decl.kind() == "variable_declarator").then_some(())?;
    let spec = require_spec(field(decl, "value")?, src)?;
    Some(encode(&spec, member.map(|m| text(m, src))))
}

fn require_spec(call: Node, src: &str) -> Option<String> {
    (call.kind() == "call_expression").then_some(())?;
    let f = field(call, "function")?;
    (f.kind() == "identifier" && text(f, src) == "require").then_some(())?;
    let arg = field(call, "arguments")?.named_child(0)?;
    (arg.kind() == "string").then(|| text(arg, src).trim_matches(|c| c == '"' || c == '\'').to_string())
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

/// A declaration and its members. `container` is set inside a namespace.
fn declaration(n: Node, src: &str, container: Option<&str>, out: &mut Vec<Def>) {
    let name = field(n, "name");
    match n.kind() {
        "function_declaration" | "generator_function_declaration" | "function_signature" => {
            if let Some(name) = name {
                out.push(def(name, src, DefKind::Function, container, return_type(n, src)));
            }
        }
        k if is_class(k) => {
            let Some(name) = name else { return };
            let cname = text(name, src);
            let mut class = def(name, src, DefKind::Class, container, Some(cname.to_string()));
            class.supertypes = class_supertypes(n, src);
            out.push(class);
            if let Some(body) = field(n, "body") {
                class_members(body, cname, src, out);
            }
        }
        "interface_declaration" => {
            let Some(name) = name else { return };
            let cname = text(name, src);
            let mut class = def(name, src, DefKind::Class, container, Some(cname.to_string()));
            class.supertypes = interface_supertypes(n, src);
            out.push(class);
            if let Some(body) = field(n, "body") {
                type_members(body, cname, src, out);
            }
        }
        // `type P = {...}` has members; `type Ps = P[]` or `type A = B`
        // has what it is declared as for a supertype.
        "type_alias_declaration" => {
            let Some(name) = name else { return };
            let cname = text(name, src);
            let mut class = def(name, src, DefKind::Class, container, Some(cname.to_string()));
            let value = field(n, "value");
            match value {
                Some(v) if v.kind() == "object_type" => {
                    out.push(class);
                    type_members(v, cname, src, out);
                }
                _ => {
                    class.supertypes.extend(value.and_then(|v| type_str(v, src)));
                    out.push(class);
                }
            }
        }
        "enum_declaration" => {
            let Some(name) = name else { return };
            let cname = text(name, src);
            out.push(def(name, src, DefKind::Class, container, Some(cname.to_string())));
            for m in field(n, "body").map(named_children).unwrap_or_default() {
                let member = match m.kind() {
                    "property_identifier" => Some(m),
                    "enum_assignment" => field(m, "name"),
                    _ => None,
                };
                if let Some(m) = member {
                    out.push(def(m, src, DefKind::EnumEntry, Some(cname), Some(cname.to_string())));
                }
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            // `require` results are imports (see `collect_require`).
            let mut names = Vec::new();
            for d in named_children(n).into_iter().filter(|d| d.kind() == "variable_declarator") {
                if field(d, "value").and_then(|v| require_spec(v, src)).is_none()
                    && let Some(name) = field(d, "name")
                {
                    pattern_names(name, &mut names);
                }
            }
            for name in names {
                out.push(def(name, src, DefKind::Property, container, decl_type_name(name, src)));
            }
        }
        // `namespace N { export function f() {} }`
        "internal_module" | "module" => {
            let Some(name) = name.filter(|n| n.kind() == "identifier") else { return };
            let cname = text(name, src);
            out.push(def(name, src, DefKind::Object, container, Some(cname.to_string())));
            for s in field(n, "body").map(named_children).unwrap_or_default() {
                let d = if s.kind() == "export_statement" { field(s, "declaration") } else { Some(s) };
                if let Some(d) = d {
                    declaration(d, src, Some(cname), out);
                }
            }
        }
        "ambient_declaration" => {
            for c in named_children(n) {
                declaration(c, src, container, out);
            }
        }
        _ => {}
    }
}

fn class_supertypes(class: Node, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(h) = named_children(class).into_iter().find(|c| c.kind() == "class_heritage") else { return out };
    for clause in named_children(h) {
        match clause.kind() {
            "extends_clause" => out.extend(field(clause, "value").and_then(|v| type_str(v, src))),
            "implements_clause" => out.extend(named_children(clause).into_iter().filter_map(|t| type_str(t, src))),
            _ => {}
        }
    }
    out
}

fn interface_supertypes(iface: Node, src: &str) -> Vec<String> {
    named_children(iface)
        .into_iter()
        .filter(|c| c.kind() == "extends_type_clause")
        .flat_map(|c| fields(c, "type"))
        .filter_map(|t| type_str(t, src))
        .collect()
}

fn class_members(body: Node, cname: &str, src: &str, out: &mut Vec<Def>) {
    let first = out.len();
    let mut methods = Vec::new();
    for m in named_children(body) {
        if m.kind() == "method_definition" {
            methods.extend(field(m, "body"));
        }
        let name = field(m, "name").filter(|n| n.kind() == "property_identifier");
        match m.kind() {
            "method_definition" | "method_signature" | "abstract_method_signature" => {
                let Some(name) = name else { continue };
                if text(name, src) == "constructor" {
                    parameter_properties(m, cname, src, out);
                    continue;
                }
                // A getter reads like a field of its return type.
                out.push(def(name, src, DefKind::Function, Some(cname), return_type(m, src)));
            }
            "public_field_definition" => {
                if let Some(name) = name {
                    out.push(def(name, src, DefKind::Property, Some(cname), decl_type_name(name, src)));
                }
            }
            _ => {}
        }
    }
    // JavaScript declares fields by assigning them: `this.count = 0`.
    let mut assigned = Vec::new();
    for b in methods {
        this_assignments(b, &mut assigned);
    }
    for (prop, value) in assigned {
        let name = text(prop, src);
        if !out[first..].iter().any(|d| d.name == name) {
            out.push(def(prop, src, DefKind::Property, Some(cname), constructed_type(value, src)));
        }
    }
}

/// `this.x = value` in a method body, outside nested `function`s and
/// classes (arrow functions share the method's `this`).
fn this_assignments<'t>(n: Node<'t>, out: &mut Vec<(Node<'t>, Node<'t>)>) {
    for c in named_children(n) {
        if matches!(c.kind(), "function_expression" | "function_declaration" | "generator_function") || is_class(c.kind())
        {
            continue;
        }
        if c.kind() == "assignment_expression"
            && let (Some(left), Some(right)) = (field(c, "left"), field(c, "right"))
            && left.kind() == "member_expression"
            && field(left, "object").is_some_and(|o| o.kind() == "this")
            && let Some(prop) = field(left, "property").filter(|p| p.kind() == "property_identifier")
        {
            out.push((prop, right));
        }
        this_assignments(c, out);
    }
}

/// `constructor(private radius: number)` declares a field.
fn parameter_properties(ctor: Node, cname: &str, src: &str, out: &mut Vec<Def>) {
    for p in field(ctor, "parameters").map(named_children).unwrap_or_default() {
        let modified = named_children(p).iter().any(|c| c.kind() == "accessibility_modifier")
            || has_token(p, "readonly")
            || has_token(p, "override");
        let Some(name) = field(p, "pattern").filter(|n| n.kind() == "identifier") else { continue };
        if modified {
            out.push(def(name, src, DefKind::Property, Some(cname), annotation(p, src)));
        }
    }
}

/// Members of an interface body or object type.
fn type_members(body: Node, cname: &str, src: &str, out: &mut Vec<Def>) {
    for m in named_children(body) {
        let Some(name) = field(m, "name").filter(|n| n.kind() == "property_identifier") else { continue };
        match m.kind() {
            "property_signature" => out.push(def(name, src, DefKind::Property, Some(cname), annotation(m, src))),
            "method_signature" => out.push(def(name, src, DefKind::Function, Some(cname), return_type(m, src))),
            _ => {}
        }
    }
}

/// Every object literal and inline object type, as a class of its own (see
/// [`anon_type`]). Its location is the `{`.
fn anon_type_defs(root: Node, src: &str, out: &mut Vec<Def>) {
    let mut c = root.walk();
    'walk: loop {
        let n = c.node();
        // `parent()` walks down from the root, so ask it only of objects.
        let anon = match n.kind() {
            "object" => true,
            "object_type" => !n.parent().is_some_and(|p| p.kind() == "type_alias_declaration"),
            _ => false,
        };
        if anon {
            let cname = anon_type(n, src);
            out.push(Def {
                name: cname.clone(),
                kind: DefKind::Class,
                start: n.start_byte(),
                end: n.start_byte() + 1,
                container: None,
                type_name: Some(cname.clone()),
                supertypes: Vec::new(),
            });
            if n.kind() == "object_type" {
                type_members(n, &cname, src, out);
            } else {
                object_members(n, &cname, src, out);
            }
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

fn object_members(obj: Node, cname: &str, src: &str, out: &mut Vec<Def>) {
    for m in named_children(obj) {
        match m.kind() {
            "pair" => {
                let Some(key) = field(m, "key").filter(|k| k.kind() == "property_identifier") else { continue };
                let value = field(m, "value");
                let kind = match value.map(|v| v.kind()) {
                    Some("arrow_function" | "function_expression" | "generator_function") => DefKind::Function,
                    _ => DefKind::Property,
                };
                out.push(def(key, src, kind, Some(cname), value.and_then(|v| constructed_type(v, src))));
            }
            "method_definition" => {
                if let Some(name) = field(m, "name").filter(|n| n.kind() == "property_identifier") {
                    out.push(def(name, src, DefKind::Function, Some(cname), return_type(m, src)));
                }
            }
            "shorthand_property_identifier" => out.push(def(m, src, DefKind::Property, Some(cname), None)),
            _ => {}
        }
    }
}

fn summarize(root: Node, src: &str) -> Summary {
    let mut s = Summary::default();
    for n in named_children(root) {
        match n.kind() {
            "import_statement" => collect_import(n, src, &mut s.imports),
            "export_statement" => collect_export(n, src, &mut s.imports, &mut s.defs),
            "lexical_declaration" | "variable_declaration" => {
                collect_require(n, src, &mut s.imports);
                declaration(n, src, None, &mut s.defs);
            }
            _ => declaration(n, src, None, &mut s.defs),
        }
    }
    anon_type_defs(root, src, &mut s.defs);
    // Overloads (`f(a: string): void;` before `f(a) {...}`), and a getter
    // with its setter, are one declaration: the first.
    let mut seen = std::collections::HashSet::new();
    s.defs.retain(|d| d.kind != DefKind::Function || seen.insert((d.container.clone(), d.name.clone())));
    s
}

// ---------------------------------------------------------------------------
// Modules
// ---------------------------------------------------------------------------

const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// A module's name: its absolute path without the extension (`.d.ts`
/// included), with an `index` file standing for its directory.
pub fn module_name(path: &Path) -> String {
    let mut p = path.to_path_buf();
    if p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e)) {
        p.set_extension("");
        if p.extension().is_some_and(|e| e == "d") {
            p.set_extension("");
        }
    }
    if p.file_name().is_some_and(|n| n == "index") {
        p.pop();
    }
    p.to_string_lossy().into_owned()
}

/// `a/./b/../c` is `a/c`, without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// What the nearest `tsconfig.json` or `jsconfig.json` says about
/// non-relative imports.
#[derive(Default)]
pub struct Config {
    /// `baseUrl`, made absolute.
    base_url: Option<PathBuf>,
    /// The directory `paths` targets are relative to.
    paths_base: PathBuf,
    /// `paths`: a pattern with at most one `*`, and its first target.
    paths: Vec<(String, String)>,
}

/// The config that applies to `file`, from the nearest directory above it
/// that has one.
pub fn find_config(file: &Path) -> Option<Config> {
    for dir in file.ancestors().skip(1) {
        for name in ["tsconfig.json", "jsconfig.json"] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(read_config(&p, 0));
            }
        }
    }
    None
}

fn read_config(path: &Path, depth: u32) -> Config {
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let value = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&strip_jsonc(&t)).ok());
    let Some(value) = value else { return Config { paths_base: dir, ..Default::default() } };
    let value: serde_json::Value = value;
    let opts = value.get("compilerOptions");
    let base_url = opts.and_then(|o| o.get("baseUrl")).and_then(|b| b.as_str()).map(|b| normalize(&dir.join(b)));
    let paths: Vec<(String, String)> = opts
        .and_then(|o| o.get("paths"))
        .and_then(|p| p.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_array()?.first()?.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    // Settings missing here may come from `extends` (a relative path).
    let parent = value
        .get("extends")
        .and_then(|e| e.as_str())
        .filter(|e| e.starts_with('.') && depth < 4 && (base_url.is_none() || paths.is_empty()))
        .map(|e| {
            let p = dir.join(e);
            read_config(&if p.extension().is_some() { p } else { p.with_extension("json") }, depth + 1)
        });
    let paths_base = base_url.clone().unwrap_or_else(|| dir.clone());
    match parent {
        Some(parent) if paths.is_empty() => Config {
            base_url: base_url.or(parent.base_url),
            paths_base: if parent.paths.is_empty() { paths_base } else { parent.paths_base },
            paths: parent.paths,
        },
        Some(parent) => Config { base_url: base_url.or(parent.base_url), paths_base, paths },
        None => Config { base_url, paths_base, paths },
    }
}

/// JSON with comments and trailing commas, as tsconfig files are, made
/// into JSON.
fn strip_jsonc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas: `,` followed by only whitespace and `}` or `]`.
    let b: Vec<char> = out.chars().collect();
    let mut clean = String::with_capacity(out.len());
    let mut in_str = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            clean.push(c);
            if c == '\\' && i + 1 < b.len() {
                clean.push(b[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            clean.push(c);
        } else if c == ',' && b[i + 1..].iter().find(|c| !c.is_whitespace()).is_some_and(|c| matches!(c, '}' | ']')) {
        } else {
            clean.push(c);
        }
        i += 1;
    }
    clean
}

/// A path as an import target: a file with any source extension, or a
/// directory with an `index` file.
fn exists_as_module(p: &Path) -> bool {
    let with_ext = |base: &Path| {
        EXTENSIONS.iter().any(|e| {
            let mut s = base.as_os_str().to_owned();
            s.push(".");
            s.push(e);
            Path::new(&s).is_file()
        })
    };
    p.is_file() || with_ext(p) || with_ext(&p.join("index"))
}

/// The module an import specifier names, seen from `file` (whose module is
/// `own`): a relative path, a `paths` pattern, a path under `baseUrl`, or
/// else the specifier itself (a package, which is not indexed). The empty
/// specifier is the file's own module.
pub fn resolve_spec(spec: &str, file: &Path, own: &str, cfg: Option<&Config>) -> String {
    if spec.is_empty() {
        return own.to_string();
    }
    if spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../") {
        let dir = file.parent().unwrap_or(Path::new(""));
        return module_name(&normalize(&dir.join(spec)));
    }
    let Some(cfg) = cfg else { return spec.to_string() };
    for (pattern, target) in &cfg.paths {
        let star = match pattern.split_once('*') {
            Some((pre, post)) => spec
                .strip_prefix(pre)
                .and_then(|r| r.strip_suffix(post))
                .map(|s| s.to_string()),
            None => (spec == pattern).then(String::new),
        };
        if let Some(star) = star {
            let t = target.replacen('*', &star, 1);
            return module_name(&normalize(&cfg.paths_base.join(t)));
        }
    }
    if let Some(base) = &cfg.base_url {
        let p = normalize(&base.join(spec));
        if exists_as_module(&p) {
            return module_name(&p);
        }
    }
    spec.to_string()
}

/// An import path from [`encode`] made absolute: `<module>.<member>`, or
/// the module alone.
fn absolute(path: &str, file: &Path, own: &str, cfg: Option<&Config>) -> String {
    let (spec, member) = path.split_once(SEP).unwrap_or((path, ""));
    let module = resolve_spec(spec, file, own, cfg);
    if member.is_empty() { module } else { format!("{module}.{member}") }
}

/// The string in `from "..."`, `require("...")` or `import("...")`.
fn module_string<'t>(n: Node<'t>, src: &str) -> Option<Node<'t>> {
    let s = if n.kind() == "string_fragment" { n.parent()? } else { n };
    (s.kind() == "string").then_some(())?;
    let p = s.parent()?;
    match p.kind() {
        "import_statement" | "export_statement" if is_field(p, "source", s) => Some(s),
        "arguments" => {
            let f = field(p.parent()?, "function")?;
            (f.kind() == "import" || (f.kind() == "identifier" && text(f, src) == "require")).then_some(s)
        }
        _ => None,
    }
}

/// Names in import and export statements resolve by path: the specifier
/// string, `{ a as b }` (both names), `* as ns`, a default import, and the
/// names `require` binds.
fn qualified_path(n: Node, src: &str) -> Option<String> {
    if let Some(s) = module_string(n, src) {
        let spec = text(s, src).trim_matches(|c| c == '"' || c == '\'' || c == '`');
        return Some(encode(spec, None));
    }
    if let Some(path) = required_name(n, src) {
        return Some(path);
    }
    let p = n.parent()?;
    let stmt = ancestors(n).find(|a| matches!(a.kind(), "import_statement" | "export_statement"))?;
    let spec = import_source(stmt, src);
    match p.kind() {
        "import_specifier" | "export_specifier" => {
            let spec = spec?;
            Some(encode(&spec, Some(text(field(p, "name")?, src))))
        }
        "namespace_import" | "namespace_export" => Some(encode(&spec?, None)),
        "import_clause" => Some(encode(&spec?, Some("default"))),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Parse repair
// ---------------------------------------------------------------------------

/// tree-sitter-typescript needs a `;` between call signatures written on
/// separate lines (`<T>(a: T): R` then `(b: string): R` in an interface),
/// and recovers from the error by losing the rest of the file. This puts
/// one before a line starting with `<` or `(` when the line before ends
/// like a type, in place of the line's first indentation byte. It also
/// blanks the `type` in `export type * from` (TypeScript 5), which the
/// grammar does not know and which re-exports the same names.
fn repair(src: &str) -> Option<String> {
    let mut out = src.as_bytes().to_vec();
    let mut changed = false;
    let mut last: Option<u8> = None;
    let mut in_comment = false;
    let mut start = 0;
    for line in src.split_inclusive('\n') {
        let bytes = line.as_bytes();
        let indent = bytes.iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
        let ends_type = |b: u8| matches!(b, b'>' | b')' | b']' | b'\'' | b'"') || b.is_ascii_alphanumeric() || b == b'_';
        if !in_comment
            && indent > 0
            && matches!(bytes.get(indent), Some(b'<' | b'('))
            && last.is_some_and(ends_type)
        {
            out[start] = b';';
            changed = true;
        }
        if !in_comment && line[indent..].starts_with("export type * ") {
            let at = start + indent + "export ".len();
            out[at..at + "type".len()].copy_from_slice(b"    ");
            changed = true;
        }
        let mut i = 0;
        while i < bytes.len() {
            if in_comment {
                if bytes[i..].starts_with(b"*/") {
                    in_comment = false;
                    i += 1;
                }
            } else if bytes[i..].starts_with(b"//") {
                break;
            } else if bytes[i..].starts_with(b"/*") {
                in_comment = true;
                i += 1;
            } else if !bytes[i].is_ascii_whitespace() {
                last = Some(bytes[i]);
            }
            i += 1;
        }
        start += line.len();
    }
    // Only ASCII bytes were replaced by ASCII, so this stays UTF-8.
    changed.then(|| String::from_utf8(out).ok()).flatten()
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

fn is_identifier(n: Node) -> bool {
    matches!(
        n.kind(),
        "identifier"
            | "property_identifier"
            | "type_identifier"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
            | "statement_identifier"
    )
}

fn name_node<'t>(root: Node<'t>, src: &str, start: usize, end: usize) -> Option<Node<'t>> {
    let node = root.descendant_for_byte_range(start, end)?;
    if is_identifier(node) && node.start_byte() == start && node.end_byte() == end {
        // A property name means something only as a member, a declared
        // member or an object key; a JSX attribute or a destructuring key
        // names nothing the index can find.
        if node.kind() == "property_identifier" {
            let p = node.parent()?;
            let ok = (p.kind() == "member_expression" && is_field(p, "property", node))
                || p.kind() == "pair"
                || is_declaration_name(node);
            return ok.then_some(node);
        }
        return Some(node);
    }
    module_string(node, src).map(|_| node)
}

fn member_receiver(ident: Node) -> Option<Node> {
    let p = ident.parent()?;
    match p.kind() {
        "member_expression" if is_field(p, "property", ident) => field(p, "object"),
        "nested_type_identifier" if is_field(p, "name", ident) => field(p, "module"),
        _ => None,
    }
}

/// For a key in an object literal written against a type, the literal.
fn named_argument_call(ident: Node) -> Option<Node> {
    let p = ident.parent()?;
    (p.kind() == "pair" && is_field(p, "key", ident)).then_some(())?;
    let obj = p.parent()?;
    contextual_type(obj).map(|_| obj)
}

fn classify_expr<'t>(expr: Node<'t>, src: &str) -> Expr<'t> {
    let wrap = |n: Option<Node<'t>>| n.map_or(Expr::Unknown, Expr::Wrapper);
    match expr.kind() {
        // `a ?? b` and `a || b` are usually an `a`.
        "binary_expression" if matches!(field(expr, "operator").map(|o| text(o, src)), Some("??" | "||")) => {
            wrap(field(expr, "left"))
        }
        k if is_literal(k) => Expr::Literal,
        "identifier" | "type_identifier" => Expr::Name(expr),
        "this" => Expr::This,
        "super" => Expr::Super,
        "call_expression" => match field(expr, "function") {
            Some(f) if f.kind() == "identifier" && text(f, src) == "require" => Expr::Unknown,
            Some(f) => Expr::Call(f),
            None => Expr::Unknown,
        },
        "new_expression" => field(expr, "constructor").map_or(Expr::Unknown, Expr::Call),
        "member_expression" => field(expr, "property").map_or(Expr::Unknown, Expr::Member),
        "nested_type_identifier" => field(expr, "name").map_or(Expr::Unknown, Expr::Member),
        "subscript_expression" => field(expr, "object").map_or(Expr::Unknown, Expr::Index),
        "parenthesized_expression" | "non_null_expression" | "await_expression" | "satisfies_expression"
        | "parenthesized_type" => wrap(expr.named_child(0)),
        "as_expression" | "type_assertion" => {
            wrap(named_children(expr).into_iter().find(|c| is_type_node(c.kind())))
        }
        "generic_type" | "array_type" | "object_type" | "union_type" | "readonly_type" => {
            type_str(expr, src).map_or(Expr::Unknown, Expr::TypeName)
        }
        _ => Expr::Unknown,
    }
}

/// See [`Language::destructured`]. Array patterns and rest elements are
/// not followed.
fn destructured<'t>(name: Node<'t>, src: &str) -> Option<(Node<'t>, Vec<String>)> {
    let mut keys = Vec::new();
    if name.kind() == "shorthand_property_identifier_pattern" {
        keys.push(text(name, src).to_string());
    }
    let mut cur = name;
    loop {
        let p = cur.parent()?;
        match p.kind() {
            "object_pattern" => {}
            "pair_pattern" if is_field(p, "value", cur) => keys.push(text(field(p, "key")?, src).to_string()),
            "assignment_pattern" | "object_assignment_pattern" if is_field(p, "left", cur) => {}
            "variable_declarator" if is_field(p, "name", cur) => {
                keys.reverse();
                return (!keys.is_empty()).then(|| field(p, "value").map(|v| (v, keys)))?;
            }
            "required_parameter" | "optional_parameter" if is_field(p, "pattern", cur) => {
                keys.reverse();
                let from = field(p, "type").and_then(|t| t.named_child(0)).or_else(|| field(p, "value"))?;
                return (!keys.is_empty()).then_some((from, keys));
            }
            _ => return None,
        }
        cur = p;
    }
}

fn callee_name(call: Node) -> Option<Node> {
    match call.kind() {
        "call_expression" | "new_expression" => {
            let f = field(call, if call.kind() == "call_expression" { "function" } else { "constructor" })?;
            match f.kind() {
                "identifier" => Some(f),
                "member_expression" => field(f, "property"),
                _ => None,
            }
        }
        "object" => contextual_type(call),
        _ => None,
    }
}

/// The members an object literal written against this type can name.
fn params_of<'t>(decl_name: Node<'t>, src: &str) -> Vec<Node<'t>> {
    let Some(decl) = decl_name.parent() else { return Vec::new() };
    let body = match decl.kind() {
        k if is_class(k) || k == "interface_declaration" => field(decl, "body"),
        "type_alias_declaration" => field(decl, "value").filter(|v| v.kind() == "object_type"),
        _ => None,
    };
    let mut out = Vec::new();
    for m in body.map(named_children).unwrap_or_default() {
        match m.kind() {
            "method_definition" => {
                let Some(name) = field(m, "name").filter(|n| n.kind() == "property_identifier") else { continue };
                if text(name, src) != "constructor" {
                    out.push(name);
                    continue;
                }
                // Parameter properties.
                for p in field(m, "parameters").map(named_children).unwrap_or_default() {
                    if named_children(p).iter().any(|c| c.kind() == "accessibility_modifier") || has_token(p, "readonly")
                    {
                        out.extend(field(p, "pattern").filter(|n| n.kind() == "identifier"));
                    }
                }
            }
            "public_field_definition" | "property_signature" | "method_signature" => {
                out.extend(field(m, "name").filter(|n| n.kind() == "property_identifier"))
            }
            _ => {}
        }
    }
    out
}

fn range_element(name: Node) -> Option<(Node, bool)> {
    let p = name.parent()?;
    (p.kind() == "for_in_statement" && is_field(p, "left", name) && has_token(p, "of")).then_some(())?;
    Some((field(p, "right")?, false))
}

/// Both grammars share everything but the grammar itself.
macro_rules! impl_language {
    ($ty:ty, $name:literal, $grammar:expr, $exts:expr) => {
        impl Language for $ty {
            fn name(&self) -> &'static str {
                $name
            }

            fn grammar(&self) -> tree_sitter::Language {
                $grammar.into()
            }

            fn extensions(&self) -> &'static [&'static str] {
                $exts
            }

            fn family(&self) -> &'static str {
                "js"
            }

            fn visibility(&self) -> Visibility {
                Visibility::Modules
            }

            fn summarize(&self, root: Node, src: &str) -> Summary {
                summarize(root, src)
            }

            fn repair(&self, src: &str) -> Option<String> {
                repair(src)
            }

            fn place(&self, summary: &mut Summary, path: &Path, _roots: &[PathBuf]) {
                summary.package = module_name(path);
                let cfg = find_config(path);
                for imp in &mut summary.imports {
                    imp.path = absolute(&imp.path, path, &summary.package, cfg.as_ref());
                }
            }

            fn absolute_import(&self, path: &str, file: &Path, summary: &Summary) -> String {
                absolute(path, file, &summary.package, find_config(file).as_ref())
            }

            fn name_node<'t>(&self, root: Node<'t>, src: &str, start: usize, end: usize) -> Option<Node<'t>> {
                name_node(root, src, start, end)
            }

            fn is_identifier(&self, n: Node) -> bool {
                is_identifier(n)
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
                range_element(name)
            }

            fn destructured<'t>(&self, name: Node<'t>, src: &str) -> Option<(Node<'t>, Vec<String>)> {
                destructured(name, src)
            }

            fn element_type(&self, container: &str, first: bool) -> Option<String> {
                container_element(container, first)
            }

            fn classify_expr<'t>(&self, expr: Node<'t>, src: &str) -> Expr<'t> {
                classify_expr(expr, src)
            }

            fn callee_name<'t>(&self, call: Node<'t>) -> Option<Node<'t>> {
                callee_name(call)
            }

            fn params_of<'t>(&self, decl_name: Node<'t>, src: &str) -> Vec<Node<'t>> {
                params_of(decl_name, src)
            }

            fn enclosing_class<'t>(&self, n: Node<'t>) -> Option<Node<'t>> {
                ancestors(n).find(|a| is_class(a.kind()))
            }

            fn class_name(&self, class: Node, src: &str) -> Option<String> {
                field(class, "name").map(|n| text(n, src).to_string())
            }

            fn class_supertypes(&self, class: Node, src: &str) -> Vec<String> {
                class_supertypes(class, src)
            }
        }
    };
}

impl_language!(
    TypeScript,
    "typescript",
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
    &["ts", "mts", "cts"]
);
impl_language!(Tsx, "tsx", tree_sitter_typescript::LANGUAGE_TSX, &["tsx", "js", "jsx", "mjs", "cjs"]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_names() {
        assert_eq!(module_name(Path::new("/p/src/shapes.ts")), "/p/src/shapes");
        assert_eq!(module_name(Path::new("/p/src/models/index.tsx")), "/p/src/models");
        assert_eq!(module_name(Path::new("/p/types.d.ts")), "/p/types");
        assert_eq!(module_name(Path::new("/p/a.test.ts")), "/p/a.test");
    }

    #[test]
    fn relative_specifiers() {
        let f = Path::new("/p/src/app/main.ts");
        let own = module_name(f);
        assert_eq!(resolve_spec("../models", f, &own, None), "/p/src/models");
        assert_eq!(resolve_spec("./util.js", f, &own, None), "/p/src/app/util");
        assert_eq!(resolve_spec("../models/index", f, &own, None), "/p/src/models");
        assert_eq!(resolve_spec("react", f, &own, None), "react");
        assert_eq!(resolve_spec("", f, &own, None), "/p/src/app/main");
    }

    #[test]
    fn paths_patterns() {
        let cfg = Config {
            base_url: None,
            paths_base: PathBuf::from("/p"),
            paths: vec![("@/*".into(), "./src/*".into()), ("#config".into(), "./config/main".into())],
        };
        let f = Path::new("/p/src/app/main.ts");
        assert_eq!(resolve_spec("@/models/shapes", f, "", Some(&cfg)), "/p/src/models/shapes");
        assert_eq!(resolve_spec("#config", f, "", Some(&cfg)), "/p/config/main");
        assert_eq!(resolve_spec("lodash", f, "", Some(&cfg)), "lodash");
    }

    #[test]
    fn jsonc() {
        let s = "{ // c\n \"a\": \"//x\", /* b */ \"p\": [1, 2,], }";
        let v: serde_json::Value = serde_json::from_str(&strip_jsonc(s)).unwrap();
        assert_eq!(v["a"], "//x");
        assert_eq!(v["p"][1], 2);
    }

    #[test]
    fn repairs_call_signatures_on_separate_lines() {
        let src = "interface I {\n  <A>(a: A): X<A>\n  // b\n  <B>(b: B): X<B>\n  (c: C): void\n}\n";
        let fixed = repair(src).unwrap();
        assert_eq!(fixed.len(), src.len());
        assert_eq!(fixed, "interface I {\n  <A>(a: A): X<A>\n  // b\n; <B>(b: B): X<B>\n; (c: C): void\n}\n");
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
        assert!(p.parse(src, None).unwrap().root_node().has_error());
        assert!(!p.parse(&fixed, None).unwrap().root_node().has_error());
        assert_eq!(repair("export type * from './t'\n").as_deref(), Some("export      * from './t'\n"));
    }

    #[test]
    fn containers() {
        assert_eq!(container_element("Circle[]", false).as_deref(), Some("Circle"));
        assert_eq!(container_element("Record<string, Circle[]>", false).as_deref(), Some("Circle[]"));
        assert_eq!(container_element("Circle", false), None);
    }
}
