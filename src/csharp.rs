//! C# on top of tree-sitter-c-sharp. Same shape as `java.rs`: namespaces
//! are packages, `using N;` is a star import of a namespace, `using static
//! N.C;` a star import of a class's members, and `using A = N.C;` an alias.
//! A namespace also sees its parent namespaces, so those are recorded as
//! star imports too. `global using` directives apply to every file
//! ([`Summary::global_imports`]).
//!
//! Every name is an `identifier` (there is no separate type identifier), so
//! [`in_type_position`] looks at where the identifier sits. Lambda
//! parameters without parentheses are `implicit_parameter` leaves.
//!
//! Type names keep their generic arguments (`List<Circle>`, `Task<Circle>`)
//! so that `foreach`, indexing and `await` can find the element type; the
//! index strips them to find members. Extension methods (`static T F(this X
//! x)`) are recorded without a container, like Kotlin's extension functions,
//! so `x.F()` reaches them. A few LINQ and Entity Framework methods are
//! known: `xs.Where(...)` keeps the type of `xs`, `xs.First()` is an element
//! of it, and a lambda passed to them takes an element.

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::lang::{child_of_kind, named_children, text, Def, DefKind, Expr, Import, Language, Scoped, Summary, Usage};

pub struct CSharp;

fn is_class_like(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration" | "struct_declaration" | "interface_declaration" | "enum_declaration" | "record_declaration"
    )
}

fn is_type_decl(kind: &str) -> bool {
    is_class_like(kind) || kind == "delegate_declaration"
}

fn is_literal(kind: &str) -> bool {
    matches!(
        kind,
        "integer_literal"
            | "real_literal"
            | "string_literal"
            | "verbatim_string_literal"
            | "raw_string_literal"
            | "character_literal"
            | "boolean_literal"
            | "null_literal"
            | "interpolated_string_expression"
            | "typeof_expression"
            | "sizeof_expression"
    )
}

fn field<'t>(n: Node<'t>, name: &str) -> Option<Node<'t>> {
    n.child_by_field_name(name)
}

fn is_field(n: Node, name: &str, child: Node) -> bool {
    field(n, name).is_some_and(|f| f.id() == child.id())
}

/// An anonymous keyword child (`static`, `global`, `this`).
fn has_token(n: Node, token: &str) -> bool {
    let mut c = n.walk();
    n.children(&mut c).any(|k| !k.is_named() && k.kind() == token)
}

fn has_modifier(n: Node, src: &str, m: &str) -> bool {
    named_children(n).into_iter().any(|c| c.kind() == "modifier" && text(c, src) == m)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The identifier naming a type: `Foo` in `Foo`, `A.B.Foo`, `Foo<T>`, `Foo?`.
fn type_head(t: Node) -> Option<Node> {
    match t.kind() {
        "identifier" => Some(t),
        "qualified_name" | "alias_qualified_name" => field(t, "name").and_then(type_head),
        "generic_name" => child_of_kind(t, "identifier"),
        "nullable_type" | "ref_type" | "scoped_type" | "pointer_type" => field(t, "type").and_then(type_head),
        _ => None,
    }
}

/// A type as written: `Circle`, `Models.Circle`, `List<Circle>`,
/// `Circle[]`, `int`. Generic arguments use simple names. `None` for `var`,
/// tuples and function pointers.
fn type_name_of(t: Node, src: &str) -> Option<String> {
    match t.kind() {
        "qualified_name" => {
            let name = type_name_of(field(t, "name")?, src)?;
            Some(format!("{}.{name}", dotted(field(t, "qualifier")?, src)))
        }
        _ => simple_type_name(t, src),
    }
}

fn simple_type_name(t: Node, src: &str) -> Option<String> {
    match t.kind() {
        "predefined_type" | "identifier" => Some(text(t, src).to_string()),
        "qualified_name" | "alias_qualified_name" => simple_type_name(field(t, "name")?, src),
        "generic_name" => {
            let head = text(child_of_kind(t, "identifier")?, src);
            let args: Vec<String> = child_of_kind(t, "type_argument_list")
                .map(|l| named_children(l).into_iter().map(|a| simple_type_name(a, src).unwrap_or_else(|| "?".into())).collect())
                .unwrap_or_default();
            Some(if args.is_empty() { head.to_string() } else { format!("{head}<{}>", args.join(", ")) })
        }
        "array_type" => Some(format!("{}[]", simple_type_name(field(t, "type")?, src)?)),
        "nullable_type" | "ref_type" | "scoped_type" => simple_type_name(field(t, "type")?, src),
        _ => None,
    }
}

/// `A, B<C, D>` split at top-level commas.
fn split_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

/// Generic types whose (last) argument is what `foreach`, `[i]` and `await`
/// give.
const CONTAINERS: &[&str] = &[
    "IEnumerable", "ICollection", "IList", "List", "IReadOnlyList", "IReadOnlyCollection", "HashSet", "ISet",
    "IReadOnlySet", "SortedSet", "Queue", "Stack", "LinkedList", "Collection", "ObservableCollection",
    "ReadOnlyCollection", "IAsyncEnumerable", "IQueryable", "IOrderedEnumerable", "IOrderedQueryable", "DbSet",
    "Span", "ReadOnlySpan", "Memory", "ReadOnlyMemory", "ImmutableArray", "ImmutableList", "ImmutableHashSet",
    "ConcurrentBag", "ConcurrentQueue", "ConcurrentStack", "BlockingCollection", "Task", "ValueTask", "Lazy",
    "Nullable", "IGrouping", "Dictionary", "IDictionary", "IReadOnlyDictionary", "ConcurrentDictionary",
    "SortedDictionary", "SortedList", "ImmutableDictionary", "ILookup",
];

/// What `foreach`, `[i]`, `await` and a lambda passed to a method give for a
/// value of type `container`: the (last) argument of a known container, or
/// the only argument of any other generic type (`IRepository<User>`,
/// `EntityTypeBuilder<Order>`, `AbstractValidator<Command>`).
fn element_type(container: &str, _first: bool) -> Option<String> {
    if let Some(e) = container.strip_suffix("[]") {
        return Some(e.to_string());
    }
    let (head, rest) = container.split_once('<')?;
    let head = head.rsplit('.').next().unwrap_or(head);
    let args = split_args(rest.strip_suffix('>')?);
    if CONTAINERS.contains(&head) {
        return args.last().map(|a| a.to_string());
    }
    match args.as_slice() {
        [one] => Some(one.to_string()),
        _ => None,
    }
}

/// LINQ (and Entity Framework) methods that return their receiver's type,
/// and ones that return an element of it. A lambda passed to any of these
/// or to [`LAMBDA_ELEMENT`] takes an element.
const SAME_SEQUENCE: &[&str] = &[
    "Where", "OrderBy", "OrderByDescending", "ThenBy", "ThenByDescending", "ToList", "ToArray", "ToHashSet",
    "AsEnumerable", "AsQueryable", "Skip", "Take", "SkipWhile", "TakeWhile", "SkipLast", "TakeLast", "Distinct",
    "DistinctBy", "Reverse", "Concat", "Union", "UnionBy", "Intersect", "IntersectBy", "Except", "ExceptBy",
    "AsNoTracking", "AsTracking", "AsSplitQuery", "Include", "IgnoreQueryFilters", "Order", "OrderDescending",
];
const ELEMENT: &[&str] = &[
    "First", "FirstOrDefault", "Single", "SingleOrDefault", "Last", "LastOrDefault", "ElementAt",
    "ElementAtOrDefault", "Find", "MinBy", "MaxBy", "Dequeue", "Peek", "Pop", "FindAsync",
];
const LAMBDA_ELEMENT: &[&str] = &[
    "Select", "SelectMany", "Any", "All", "Count", "LongCount", "Sum", "Average", "Min", "Max", "GroupBy",
    "ToDictionary", "ToLookup", "ForEach", "RemoveAll", "FindAll", "FindIndex", "FindLastIndex", "FindLast",
    "Exists", "TrueForAll", "Aggregate", "Contains",
];

/// The name of a LINQ-like method, without an `Async` suffix
/// (`FirstOrDefaultAsync`, `ToListAsync`).
fn linq_name(name: &str) -> &str {
    name.strip_suffix("Async")
        .filter(|n| SAME_SEQUENCE.contains(n) || ELEMENT.contains(n) || LAMBDA_ELEMENT.contains(n))
        .unwrap_or(name)
}

/// `a?.F` as a callee: the `.F` part.
fn unwrap_conditional(f: Node) -> Node {
    if f.kind() == "conditional_access_expression" {
        return named_children(f).into_iter().last().unwrap_or(f);
    }
    f
}

/// For `recv.Method(...)`, the receiver and the method's name.
fn method_call<'t, 's>(call: Node<'t>, src: &'s str) -> Option<(Node<'t>, &'s str)> {
    if call.kind() != "invocation_expression" {
        return None;
    }
    let f = unwrap_conditional(field(call, "function")?);
    let (recv, name) = match f.kind() {
        "member_access_expression" => (field(f, "expression")?, field(f, "name")?),
        // `a?.Where(...)`
        "member_binding_expression" => (member_receiver(field(f, "name")?)?, field(f, "name")?),
        _ => return None,
    };
    let name = type_head(name).unwrap_or(name);
    Some((recv, text(name, src)))
}

/// `Get<Foo>(...)`: a call with one type argument usually returns it
/// (`GetService<T>`, `Deserialize<T>`), a `Task` of it when the method is
/// `...Async`, and a sequence of it for `Cast`, `OfType` and `Empty`.
fn generic_call_type(call: Node, src: &str) -> Option<String> {
    let f = unwrap_conditional(field(call, "function")?);
    let g = match f.kind() {
        "generic_name" => f,
        "member_access_expression" | "member_binding_expression" => field(f, "name")?,
        _ => return None,
    };
    if g.kind() != "generic_name" {
        return None;
    }
    let args = named_children(child_of_kind(g, "type_argument_list")?);
    let [arg] = args.as_slice() else { return None };
    let t = type_name_of(*arg, src)?;
    let name = text(child_of_kind(g, "identifier")?, src);
    Some(match name {
        "Cast" | "OfType" | "Empty" => format!("IEnumerable<{t}>"),
        "Set" => format!("DbSet<{t}>"),
        n if n.ends_with("Async") => format!("Task<{t}>"),
        _ => t,
    })
}

/// The type parameter `t` names, mapped to its first constraint (`where T :
/// IShape` makes a `T` an `IShape`).
fn constrained(t: &str, from: Node, src: &str) -> Option<String> {
    let mut cur = from.parent();
    while let Some(n) = cur {
        for clause in named_children(n).into_iter().filter(|c| c.kind() == "type_parameter_constraints_clause") {
            let kids = named_children(clause);
            if kids.first().is_some_and(|k| text(*k, src) == t) {
                return kids
                    .iter()
                    .filter(|k| k.kind() == "type_parameter_constraint")
                    .find_map(|k| field(*k, "type").and_then(|ty| type_name_of(ty, src)));
            }
        }
        cur = n.parent();
    }
    None
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

/// Names declared by a `variable_declaration` (incl. `var (a, b) = ...`).
fn declarator_names(decl: Node) -> Vec<Node> {
    let mut out = Vec::new();
    for d in named_children(decl).into_iter().filter(|c| c.kind() == "variable_declarator") {
        match field(d, "name") {
            Some(n) => out.push(n),
            None => {
                if let Some(t) = child_of_kind(d, "tuple_pattern") {
                    out.extend(designations(t));
                }
            }
        }
    }
    out
}

/// The names in `(a, (b, c))`, `var (a, b)`.
fn designations(n: Node) -> Vec<Node> {
    let mut out = Vec::new();
    let mut stack = vec![n];
    while let Some(x) = stack.pop() {
        match x.kind() {
            "identifier" => out.push(x),
            "tuple_pattern" | "parenthesized_variable_designation" | "var_pattern" | "declaration_expression"
            | "tuple_expression" | "argument" => stack.extend(named_children(x).into_iter().rev()),
            _ => {}
        }
    }
    out.retain(|n| n.parent().is_none_or(|p| !matches!(p.kind(), "declaration_expression") || is_field(p, "name", *n)));
    out
}

fn type_param_names(decl: Node) -> Vec<Node> {
    let Some(tps) = field(decl, "type_parameters").or_else(|| child_of_kind(decl, "type_parameter_list")) else {
        return Vec::new();
    };
    named_children(tps).into_iter().filter_map(|tp| field(tp, "name")).collect()
}

/// Parameter names of a method, constructor, local function, lambda,
/// delegate, indexer, record or primary constructor (incl. type
/// parameters).
fn param_names(decl: Node) -> Vec<Node> {
    let mut out = type_param_names(decl);
    let params = field(decl, "parameters").or_else(|| child_of_kind(decl, "parameter_list"));
    match params {
        // `x => ...`
        Some(p) if p.kind() == "implicit_parameter" => out.push(p),
        Some(p) => out.extend(params_of_list(p).into_iter().map(|(name, _)| name)),
        None => {}
    }
    out
}

/// The parameters in a `parameter_list`: name, and whether it is optional
/// (has a default, or is `params`, which sits in the list itself).
fn params_of_list(list: Node) -> Vec<(Node, bool)> {
    let mut out = Vec::new();
    let mut c = list.walk();
    let mut after_params = false;
    for (i, k) in list.children(&mut c).enumerate() {
        let is_name = list.field_name_for_child(i as u32) == Some("name");
        match k.kind() {
            "params" => after_params = true,
            "parameter" => {
                if let Some(name) = field(k, "name") {
                    out.push((name, has_token(k, "=")));
                }
            }
            "identifier" if is_name => out.push((k, after_params)),
            _ => {}
        }
    }
    out
}

/// Required and at most how many arguments a parameter list takes.
fn list_arity(list: Node) -> (usize, Option<usize>) {
    let mut c = list.walk();
    let variadic = list.children(&mut c).any(|k| k.kind() == "params");
    let params = params_of_list(list);
    let required = params.iter().filter(|(_, optional)| !optional).count();
    (required, (!variadic).then_some(params.len()))
}

fn body_members(class: Node) -> Vec<Node> {
    field(class, "body").map(named_children).unwrap_or_default()
}

/// Names a class declares directly: type parameters, primary constructor
/// or record parameters, fields, properties, events, methods, nested types
/// and enum members.
fn class_member_names(class: Node) -> Vec<Node> {
    let mut out = param_names(class);
    for m in body_members(class) {
        match m.kind() {
            "field_declaration" | "event_field_declaration" => {
                out.extend(child_of_kind(m, "variable_declaration").map(declarator_names).unwrap_or_default())
            }
            "method_declaration" | "property_declaration" | "event_declaration" | "enum_member_declaration" => {
                out.extend(field(m, "name"))
            }
            k if is_type_decl(k) => out.extend(field(m, "name")),
            _ => {}
        }
    }
    out
}

fn is_declaration_name(ident: Node) -> bool {
    if ident.kind() == "implicit_parameter" {
        return true;
    }
    let Some(p) = ident.parent() else { return false };
    match p.kind() {
        // A constructor's name is a use of its class.
        "constructor_declaration" | "destructor_declaration" => false,
        k if is_type_decl(k) => is_field(p, "name", ident),
        "method_declaration"
        | "local_function_statement"
        | "property_declaration"
        | "event_declaration"
        | "enum_member_declaration"
        | "variable_declarator"
        | "parameter"
        | "type_parameter"
        | "catch_declaration"
        | "declaration_expression"
        | "declaration_pattern"
        | "recursive_pattern"
        | "from_clause"
        | "list_pattern" => is_field(p, "name", ident),
        "foreach_statement" => is_field(p, "left", ident),
        "tuple_pattern" | "parenthesized_variable_designation" | "var_pattern" => true,
        "let_clause" | "join_clause" | "join_into_clause" => named_children(p).first().is_some_and(|f| f.id() == ident.id()),
        _ => false,
    }
}

/// Supertypes with their generic arguments (`AbstractValidator<Command>`):
/// the index strips them to find the class, and [`element_type`] reads them.
fn supertypes(class: Node, src: &str) -> Vec<String> {
    let Some(list) = child_of_kind(class, "base_list") else { return Vec::new() };
    named_children(list)
        .into_iter()
        .filter_map(|t| {
            let t = if t.kind() == "primary_constructor_base_type" { field(t, "type")? } else { t };
            simple_type_name(t, src)
        })
        .collect()
}

/// Type written on a declared name. `None` for `var`, which leaves the
/// initializer (or the loop) to say.
fn decl_type_name(name: Node, src: &str) -> Option<String> {
    if let Some(t) = lambda_param_type(name, src) {
        return Some(t);
    }
    let p = name.parent()?;
    let ty = match p.kind() {
        k if is_class_like(k) => return Some(text(name, src).to_string()),
        "enum_member_declaration" => return enclosing_class(p).and_then(|e| field(e, "name")).map(|n| text(n, src).to_string()),
        "variable_declarator" => field(p.parent()?, "type")?,
        "method_declaration" => field(p, "returns")?,
        "parameter" | "property_declaration" | "event_declaration" | "local_function_statement"
        | "delegate_declaration" | "catch_declaration" | "declaration_expression" | "declaration_pattern"
        | "recursive_pattern" | "foreach_statement" | "from_clause" => field(p, "type")?,
        _ => return None,
    };
    let t = type_name_of(ty, src)?;
    Some(constrained(&t, name, src).unwrap_or(t))
}

fn decl_initializer(name: Node) -> Option<Node> {
    let p = name.parent()?;
    match p.kind() {
        "variable_declarator" => named_children(p)
            .into_iter()
            .last()
            .filter(|v| v.id() != name.id() && !matches!(v.kind(), "bracketed_argument_list" | "tuple_pattern")),
        "property_declaration" => {
            let v = field(p, "value")?;
            if v.kind() == "arrow_expression_clause" { v.named_child(0) } else { Some(v) }
        }
        _ => None,
    }
}

/// The namespace a declaration sits in, joined across nesting.
fn namespace_of(root: Node, src: &str) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(root);
    while let Some(n) = cur {
        cur = None;
        for c in named_children(n) {
            match c.kind() {
                "file_scoped_namespace_declaration" => {
                    parts.extend(field(c, "name").map(|q| dotted(q, src)));
                }
                "namespace_declaration" => {
                    parts.extend(field(c, "name").map(|q| dotted(q, src)));
                    cur = field(c, "body");
                    break;
                }
                _ => {}
            }
        }
    }
    parts.join(".")
}

fn dotted(n: Node, src: &str) -> String {
    text(n, src).chars().filter(|c| !c.is_whitespace()).collect()
}

fn collect_using(u: Node, src: &str, s: &mut Summary) {
    let alias = field(u, "name");
    let Some(target) = named_children(u)
        .into_iter()
        .find(|c| alias.is_none_or(|a| a.id() != c.id()) && matches!(c.kind(), "qualified_name" | "identifier" | "generic_name" | "alias_qualified_name"))
    else {
        return;
    };
    let path = dotted(target, src);
    // `using X = A.B.C;` binds X; `using A.B;` and `using static A.B.C;`
    // bring in their members.
    let imp = match alias {
        Some(a) => Import { path, star: false, alias: Some(text(a, src).to_string()) },
        None => Import { path, star: true, alias: None },
    };
    if has_token(u, "global") {
        s.global_imports.push(imp);
    } else {
        s.imports.push(imp);
    }
}

fn summarize(root: Node, src: &str) -> Summary {
    let mut s = Summary { package: namespace_of(root, src), ..Summary::default() };
    walk_top(root, src, &mut s);
    // A namespace sees its parents' types.
    let mut ns = s.package.as_str();
    while let Some((parent, _)) = ns.rsplit_once('.') {
        s.imports.push(Import { path: parent.to_string(), star: true, alias: None });
        ns = parent;
    }
    s
}

/// Usings and type declarations at the top level or inside namespaces.
/// Members found there are what is left of a class the parser gave up on
/// (unbalanced preprocessor branches): they go to the class before them.
fn walk_top(n: Node, src: &str, s: &mut Summary) {
    let mut last_class: Option<String> = None;
    for c in named_children(n) {
        match c.kind() {
            k if is_class_like(k) => {
                last_class = field(c, "name").map(|n| text(n, src).to_string());
                collect_defs(c, src, None, &mut s.defs);
            }
            "method_declaration" | "property_declaration" | "field_declaration" | "event_declaration"
            | "event_field_declaration" => {
                if let Some(class) = &last_class {
                    collect_defs(c, src, Some(class), &mut s.defs);
                }
            }
            "using_directive" => collect_using(c, src, s),
            "namespace_declaration" => {
                if let Some(body) = field(c, "body") {
                    walk_top(body, src, s);
                }
            }
            "file_scoped_namespace_declaration" => walk_top(c, src, s),
            _ => collect_defs(c, src, None, &mut s.defs),
        }
    }
}

fn collect_defs(n: Node, src: &str, container: Option<&str>, out: &mut Vec<Def>) {
    let def = |name: Node, kind, container: Option<&str>, type_name, supertypes| Def {
        name: text(name, src).to_string(),
        kind,
        start: name.start_byte(),
        end: name.end_byte(),
        container: container.map(str::to_string),
        type_name,
        supertypes,
    };
    match n.kind() {
        k if is_class_like(k) => {
            let Some(name) = field(n, "name") else { return };
            let cname = text(name, src).to_string();
            out.push(def(name, DefKind::Class, container, Some(cname.clone()), supertypes(n, src)));
            if k == "record_declaration" {
                // Positional parameters are properties.
                for p in param_names(n).into_iter().filter(|p| p.parent().is_some_and(|q| q.kind() == "parameter")) {
                    out.push(def(p, DefKind::Property, Some(&cname), decl_type_name(p, src), Vec::new()));
                }
            }
            for m in body_members(n) {
                if m.kind() == "enum_member_declaration" {
                    if let Some(e) = field(m, "name") {
                        out.push(def(e, DefKind::EnumEntry, Some(&cname), Some(cname.clone()), Vec::new()));
                    }
                } else {
                    collect_defs(m, src, Some(&cname), out);
                }
            }
        }
        "delegate_declaration" => {
            if let Some(name) = field(n, "name") {
                out.push(def(name, DefKind::TypeAlias, container, None, Vec::new()));
            }
        }
        "method_declaration" => {
            let Some(name) = field(n, "name") else { return };
            let generics: Vec<&str> = type_param_names(n).into_iter().map(|t| text(t, src)).collect();
            // `T Get<T>()` tells us nothing.
            let ret = field(n, "returns")
                .and_then(|t| type_name_of(t, src))
                .filter(|t| !generics.contains(&t.as_str()) && t != "void");
            // `static R F(this X x)`: called as `x.F()`.
            let extension = has_modifier(n, src, "static")
                && field(n, "parameters")
                    .and_then(|p| child_of_kind(p, "parameter"))
                    .is_some_and(|p| has_modifier(p, src, "this"));
            let container = if extension { None } else { container };
            out.push(def(name, DefKind::Function, container, ret, Vec::new()));
        }
        "property_declaration" | "event_declaration" => {
            let Some(name) = field(n, "name") else { return };
            let ty = field(n, "type").and_then(|t| type_name_of(t, src));
            out.push(def(name, DefKind::Property, container, ty, Vec::new()));
        }
        "field_declaration" | "event_field_declaration" => {
            let Some(decl) = child_of_kind(n, "variable_declaration") else { return };
            let ty = field(decl, "type").and_then(|t| type_name_of(t, src));
            for name in declarator_names(decl) {
                out.push(def(name, DefKind::Property, container, ty.clone(), Vec::new()));
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Scope resolution
// ---------------------------------------------------------------------------

/// Expression variables bound inside `n` (`o is Circle c`, `out var x`,
/// `case Point p`), not counting nested scopes that keep their own.
fn pattern_names(n: Node) -> Vec<Node> {
    let mut out = Vec::new();
    let mut stack = vec![n];
    while let Some(x) = stack.pop() {
        match x.kind() {
            "declaration_pattern" | "recursive_pattern" | "list_pattern" | "declaration_expression" => {
                out.extend(field(x, "name"));
            }
            "var_pattern" | "parenthesized_variable_designation" => out.extend(designations(x)),
            _ => {}
        }
        if !matches!(
            x.kind(),
            "lambda_expression"
                | "anonymous_method_expression"
                | "block"
                | "local_function_statement"
                | "switch_expression_arm"
                | "switch_section"
                | "query_expression"
        ) || x.id() == n.id()
        {
            stack.extend(named_children(x));
        }
    }
    out
}

/// What a statement in a block declares for the statements after it. `pos`
/// is the use; locals are visible after their declaration, local functions
/// everywhere, and expression variables of an `if` condition or an
/// expression statement leak into the block.
fn statement_names<'t>(st: Node<'t>, pos: usize, out: &mut Vec<Node<'t>>) {
    match st.kind() {
        "local_function_statement" => out.extend(field(st, "name")),
        _ if st.start_byte() > pos => {}
        "local_declaration_statement" => {
            if let Some(d) = child_of_kind(st, "variable_declaration")
                && st.end_byte() <= pos
            {
                out.extend(declarator_names(d));
            }
            out.extend(pattern_names(st));
        }
        "using_statement" if field(st, "body").is_none() => {
            out.extend(child_of_kind(st, "variable_declaration").map(declarator_names).unwrap_or_default())
        }
        "if_statement" => out.extend(field(st, "condition").map(pattern_names).unwrap_or_default()),
        "expression_statement" | "return_statement" | "throw_statement" | "yield_statement" => {
            out.extend(pattern_names(st))
        }
        "global_statement" => {
            if let Some(inner) = st.named_child(0) {
                statement_names(inner, pos, out);
            }
        }
        _ => {}
    }
}

fn is_type_name_decl(n: Node) -> bool {
    n.parent().is_some_and(|p| is_type_decl(p.kind()) || p.kind() == "type_parameter")
}

fn find_in_scopes<'t>(from: Node<'t>, name: &str, src: &str) -> Option<Scoped<'t>> {
    let types_only = in_type_position(from);
    let pos = from.start_byte();
    let mut prev = from;
    let mut cur = from.parent();
    while let Some(scope) = cur {
        // `value` in a setter.
        if scope.kind() == "accessor_declaration" && name == "value" && !types_only {
            let kw = scope.child(0).map(|k| text(k, src));
            if !has_token(scope, "get") && kw.is_some() {
                return Some(Scoped::Implicit(scope));
            }
        }
        let found = lookup_scope(scope, prev, pos)
            .into_iter()
            .filter(|n| !types_only || is_type_name_decl(*n))
            .find(|n| text(*n, src) == name);
        if let Some(hit) = found {
            return Some(Scoped::Decl(hit));
        }
        prev = scope;
        cur = scope.parent();
    }
    None
}

/// Names `scope` declares that are visible at `pos`. `child` is the node on
/// the path from the use up to `scope`.
fn lookup_scope<'t>(scope: Node<'t>, child: Node<'t>, pos: usize) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    match scope.kind() {
        "block" | "compilation_unit" => {
            for st in named_children(scope) {
                statement_names(st, pos, &mut out);
            }
        }
        "switch_section" => {
            for c in named_children(scope) {
                if c.kind().ends_with("_statement") || c.kind() == "block" {
                    statement_names(c, pos, &mut out);
                } else {
                    out.extend(pattern_names(c));
                }
            }
        }
        // The arm's result and `when` see its pattern.
        "switch_expression_arm" => {
            for c in named_children(scope) {
                if c.kind().ends_with("pattern") || c.kind() == "when_clause" {
                    out.extend(pattern_names(c));
                }
            }
        }
        "method_declaration" | "constructor_declaration" | "local_function_statement" | "lambda_expression"
        | "anonymous_method_expression" | "operator_declaration" | "conversion_operator_declaration"
        | "indexer_declaration" | "delegate_declaration" => out = param_names(scope),
        "for_statement" => {
            if let Some(init) = field(scope, "initializer") {
                out.extend(declarator_names(init));
            }
            out.extend(field(scope, "condition").map(pattern_names).unwrap_or_default());
        }
        "foreach_statement" => {
            if let Some(left) = field(scope, "left") {
                if left.kind() == "identifier" { out.push(left) } else { out.extend(designations(left)) }
            }
        }
        "catch_clause" => out.extend(child_of_kind(scope, "catch_declaration").and_then(|d| field(d, "name"))),
        "using_statement" | "fixed_statement" => {
            out.extend(child_of_kind(scope, "variable_declaration").map(declarator_names).unwrap_or_default());
            out.extend(named_children(scope).into_iter().filter(|c| c.kind() != "block").flat_map(pattern_names));
        }
        "while_statement" => out.extend(field(scope, "condition").map(pattern_names).unwrap_or_default()),
        // `o is Circle c && c.Radius > 0`: the right side sees the left's
        // pattern variables.
        "binary_expression" => {
            if let (Some(left), Some(right)) = (field(scope, "left"), field(scope, "right"))
                && right.id() == child.id()
            {
                out.extend(pattern_names(left));
            }
        }
        "conditional_expression" => {
            if let Some(cond) = field(scope, "condition").filter(|c| c.id() != child.id()) {
                out.extend(pattern_names(cond));
            }
        }
        "query_expression" => {
            for c in named_children(scope) {
                match c.kind() {
                    "from_clause" => out.extend(field(c, "name")),
                    "let_clause" | "join_clause" => out.extend(child_of_kind(c, "identifier")),
                    "select_clause" | "group_clause" => {
                        // `into x`
                        out.extend(named_children(c).into_iter().filter(|i| i.kind() == "identifier" && i.next_named_sibling().is_some_and(|n| n.kind() == "query_expression")));
                    }
                    _ => {}
                }
            }
        }
        k if is_class_like(k) => out = class_member_names(scope),
        _ => {}
    }
    out
}

/// The node `x` belongs to in a type: the whole `A.B<C>` it is part of.
fn type_root(mut n: Node) -> Node {
    while let Some(p) = n.parent() {
        let up = match p.kind() {
            "qualified_name" | "alias_qualified_name" => true,
            "generic_name" => child_of_kind(p, "identifier").is_some_and(|h| h.id() == n.id()),
            _ => false,
        };
        if !up {
            break;
        }
        n = p;
    }
    n
}

fn in_type_position(n: Node) -> bool {
    if n.kind() != "identifier" {
        return false;
    }
    let t = type_root(n);
    let Some(p) = t.parent() else { return false };
    let slot = |f: &str| is_field(p, f, t);
    match p.kind() {
        "variable_declaration" | "parameter" | "property_declaration" | "local_function_statement"
        | "delegate_declaration" | "event_declaration" | "indexer_declaration" | "operator_declaration"
        | "conversion_operator_declaration" | "object_creation_expression" | "array_creation_expression"
        | "cast_expression" | "typeof_expression" | "default_expression" | "sizeof_expression"
        | "declaration_pattern" | "declaration_expression" | "recursive_pattern" | "type_pattern"
        | "foreach_statement" | "catch_declaration" | "tuple_element" | "type_parameter_constraint"
        | "primary_constructor_base_type" | "from_clause" | "join_clause" | "stackalloc_expression" => slot("type"),
        "method_declaration" => slot("returns"),
        "attribute" => slot("name"),
        "as_expression" | "is_expression" => slot("right"),
        "array_type" | "nullable_type" | "pointer_type" | "ref_type" | "scoped_type" => true,
        "type_argument_list" | "base_list" | "tuple_type" => true,
        // `T` in `where T : ...`
        "type_parameter_constraints_clause" => false,
        _ => false,
    }
}

fn member_receiver(ident: Node) -> Option<Node> {
    let n = if ident.parent()?.kind() == "generic_name" { ident.parent()? } else { ident };
    let p = n.parent()?;
    match p.kind() {
        "member_access_expression" if is_field(p, "name", n) => field(p, "expression"),
        // `a?.B`: the receiver is the nearest `?.`'s left side.
        "member_binding_expression" if is_field(p, "name", n) => {
            let mut cur = p.parent();
            while let Some(c) = cur {
                if c.kind() == "conditional_access_expression" {
                    let cond = field(c, "condition")?;
                    if !(cond.start_byte() <= p.start_byte() && p.end_byte() <= cond.end_byte()) {
                        return Some(cond);
                    }
                }
                cur = c.parent();
            }
            None
        }
        // `new Circle { Radius = 1 }`
        "assignment_expression" if is_field(p, "left", n) => {
            let init = p.parent().filter(|i| i.kind() == "initializer_expression")?;
            let creation = init.parent()?;
            matches!(creation.kind(), "object_creation_expression" | "implicit_object_creation_expression")
                .then_some(creation)
        }
        // `p with { X = 1 }`
        "with_initializer" if p.named_child(0).is_some_and(|c| c.id() == n.id()) => p.parent()?.named_child(0),
        // `{ Radius: > 0 }` in `o is Circle { ... }`
        "subpattern" if p.named_child(0).is_some_and(|c| c.id() == n.id()) => {
            let rec = p.parent()?.parent().filter(|r| r.kind() == "recursive_pattern")?;
            field(rec, "type")
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

/// The type a target-typed `new(...)` creates.
fn target_type<'t>(creation: Node<'t>, src: &str) -> Option<String> {
    let p = creation.parent()?;
    match p.kind() {
        "variable_declarator" => type_name_of(field(p.parent()?, "type")?, src),
        "arrow_expression_clause" | "equals_value_clause" => {
            let owner = p.parent()?;
            type_name_of(field(owner, "type").or_else(|| field(owner, "returns"))?, src)
        }
        "property_declaration" => type_name_of(field(p, "type")?, src),
        _ => None,
    }
}

/// An untyped lambda parameter's type, from the delegate type the lambda is
/// assigned to: `Func<Circle, double> f = c => ...`.
fn lambda_param_type(name: Node, src: &str) -> Option<String> {
    let p = name.parent()?;
    let (lambda, index) = match p.kind() {
        "lambda_expression" if name.kind() == "implicit_parameter" => (p, 0),
        "parameter" if field(p, "type").is_none() => {
            let list = p.parent()?;
            let index = named_children(list).iter().position(|c| c.id() == p.id())?;
            (list.parent().filter(|l| l.kind() == "lambda_expression")?, index)
        }
        _ => return None,
    };
    let decl = lambda.parent().filter(|d| d.kind() == "variable_declarator")?.parent()?;
    let mut ty = type_name_of(field(decl, "type")?, src)?;
    if let Some(inner) = ty.strip_prefix("Expression<").and_then(|t| t.strip_suffix('>')) {
        ty = inner.to_string();
    }
    let (head, rest) = ty.split_once('<')?;
    let args = split_args(rest.strip_suffix('>')?);
    let params = match head {
        "Func" => &args[..args.len().saturating_sub(1)],
        "Action" | "Predicate" | "Comparison" | "Converter" => &args[..],
        _ => return None,
    };
    params.get(index).map(|a| a.to_string())
}

/// Where a lambda parameter takes its type from, as an element (see
/// [`element_type`]): the receiver of the method the lambda is passed to
/// (`xs.Where(x => ...)`, `builder.Property(o => ...)`), or for a call
/// without one (`RuleFor(x => ...)`) the enclosing class body, which
/// [`Expr::This`] types as the class.
fn lambda_source<'t>(name: Node<'t>, src: &str) -> Option<Node<'t>> {
    let p = name.parent()?;
    let lambda = match p.kind() {
        "lambda_expression" => p,
        "parameter" if field(p, "type").is_none() => {
            let list = p.parent()?;
            (list.named_child(0)?.id() == p.id()).then_some(())?;
            list.parent().filter(|l| l.kind() == "lambda_expression")?
        }
        _ => return None,
    };
    let call = lambda.parent().filter(|a| a.kind() == "argument")?.parent()?.parent()?;
    if call.kind() != "invocation_expression" {
        return None;
    }
    if let Some((recv, _)) = method_call(call, src) {
        return Some(recv);
    }
    let f = field(call, "function")?;
    if matches!(f.kind(), "identifier" | "generic_name") {
        return enclosing_class(call).and_then(|c| field(c, "body"));
    }
    None
}

// ---------------------------------------------------------------------------
// Preprocessor
// ---------------------------------------------------------------------------

/// `src` with only the first branch of each `#if` kept: `#elif` and `#else`
/// branches and every directive line (`#region`, `#pragma` too) become
/// spaces. The grammar parses directives only between whole declarations
/// and statements, so alternatives that each complete a construct (`,
/// IConvertible` in a base list, an `else` before a shared block) break it,
/// and so does a directive on a last line without a line break.
fn keep_first_branches(src: &str) -> Option<String> {
    let mut out = String::with_capacity(src.len());
    // Per open `#if`: whether its current branch is dropped.
    let mut stack: Vec<bool> = Vec::new();
    let mut changed = false;
    for line in src.split_inclusive('\n') {
        let t = line.trim_start();
        let directive = t.strip_prefix('#').map(|d| d.trim_start().split(|c: char| !c.is_alphanumeric()).next().unwrap_or(""));
        let dropped_outside = stack.iter().any(|d| *d);
        let blank = match directive {
            Some("if") => {
                stack.push(false);
                true
            }
            Some("elif" | "else") => {
                if let Some(top) = stack.last_mut() {
                    *top = true;
                }
                true
            }
            Some("endif") => {
                stack.pop();
                true
            }
            Some(_) => true,
            None => dropped_outside,
        };
        if blank {
            changed = true;
            // Same byte length: a multi-byte character becomes several spaces.
            for c in line.chars() {
                match c {
                    '\n' | '\r' => out.push(c),
                    c => out.extend(std::iter::repeat_n(' ', c.len_utf8())),
                }
            }
        } else {
            out.push_str(line);
        }
    }
    changed.then_some(out)
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

/// The nearest directory above `file` with a `.csproj`, and that file.
fn find_project(file: &Path) -> Option<(PathBuf, PathBuf)> {
    file.ancestors().skip(1).find_map(|dir| {
        let csproj = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "csproj"))?;
        Some((dir.to_path_buf(), csproj))
    })
}

/// The value of `name="..."` inside an XML tag.
fn xml_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let len = tag[at..].find('"')?;
    Some(&tag[at..at + len])
}

/// `<Using Include="N" />` items of a project file: global usings.
fn project_usings(csproj: &str) -> Vec<Import> {
    let mut out = Vec::new();
    for (at, _) in csproj.match_indices("<Using ") {
        let tag = &csproj[at..at + csproj[at..].find('>').unwrap_or(0)];
        let Some(path) = xml_attr(tag, "Include") else { continue };
        let alias = xml_attr(tag, "Alias").map(str::to_string);
        out.push(Import { path: path.to_string(), star: alias.is_none(), alias });
    }
    out
}

// ---------------------------------------------------------------------------
// The Language impl
// ---------------------------------------------------------------------------

impl Language for CSharp {
    fn name(&self) -> &'static str {
        "C#"
    }

    fn grammar(&self) -> tree_sitter::Language {
        tree_sitter_c_sharp::LANGUAGE.into()
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["cs"]
    }

    fn family(&self) -> &'static str {
        "dotnet"
    }

    fn package_before_imports(&self) -> bool {
        true
    }

    fn hidden_receivers(&self) -> bool {
        false
    }

    fn summarize(&self, root: Node, src: &str) -> Summary {
        summarize(root, src)
    }

    fn repair(&self, src: &str) -> Option<String> {
        keep_first_branches(src)
    }

    fn place(&self, summary: &mut Summary, path: &Path, _roots: &[PathBuf]) {
        if let Some((dir, csproj)) = find_project(path) {
            summary.project = Some(dir);
            summary.imports.extend(std::fs::read_to_string(csproj).map(|t| project_usings(&t)).unwrap_or_default());
        }
    }

    fn name_node<'t>(&self, root: Node<'t>, _src: &str, start: usize, end: usize) -> Option<Node<'t>> {
        let mut node = root.descendant_for_byte_range(start, end)?;
        // Contextual keywords used as names (`by`, `on`) wrap a token.
        if !self.is_identifier(node) {
            node = node.parent().filter(|p| self.is_identifier(*p))?;
        }
        (node.start_byte() == start && node.end_byte() == end).then_some(node)
    }

    fn is_identifier(&self, n: Node) -> bool {
        matches!(n.kind(), "identifier" | "implicit_parameter")
    }

    fn is_declaration_name(&self, ident: Node) -> bool {
        is_declaration_name(ident)
    }

    fn named_argument_call<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        let arg = ident.parent().filter(|a| a.kind() == "argument" && is_field(*a, "name", ident))?;
        let call = arg.parent()?.parent()?;
        matches!(call.kind(), "invocation_expression" | "object_creation_expression").then_some(call)
    }

    fn member_receiver<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        member_receiver(ident)
    }

    fn type_qualifier<'t>(&self, ident: Node<'t>) -> Option<Node<'t>> {
        let n = if ident.parent()?.kind() == "generic_name" { ident.parent()? } else { ident };
        let p = n.parent()?;
        if p.kind() != "qualified_name" || !is_field(p, "name", n) {
            return None;
        }
        let q = field(p, "qualifier")?;
        match q.kind() {
            "identifier" => Some(q),
            "generic_name" => child_of_kind(q, "identifier"),
            "qualified_name" => field(q, "name").and_then(type_head),
            _ => None,
        }
    }

    fn in_type_position(&self, n: Node) -> bool {
        in_type_position(n)
    }

    fn qualified_path(&self, ident: Node, src: &str) -> Option<String> {
        let parent = ident.parent()?;
        // `using X = A.B.C;`: the alias stands for the whole path.
        if parent.kind() == "using_directive" && is_field(parent, "name", ident) {
            let target = named_children(parent).into_iter().find(|c| c.id() != ident.id())?;
            return Some(dotted(target, src));
        }
        let node = if parent.kind() == "qualified_name" && is_field(parent, "name", ident) { parent } else { ident };
        let mut top = node;
        while top.parent()?.kind() == "qualified_name" {
            top = top.parent()?;
        }
        matches!(top.parent()?.kind(), "using_directive" | "namespace_declaration" | "file_scoped_namespace_declaration")
            .then(|| dotted(node, src))
    }

    fn dotted_path(&self, ident: Node, src: &str) -> Option<String> {
        // `A.B.C` as an expression or a type, made only of names.
        let p = ident.parent()?;
        let whole = match p.kind() {
            "member_access_expression" | "qualified_name" if is_field(p, "name", ident) => p,
            _ => return None,
        };
        let mut cur = whole;
        loop {
            let q = field(cur, "expression").or_else(|| field(cur, "qualifier"))?;
            match q.kind() {
                "identifier" => break,
                "member_access_expression" | "qualified_name" => cur = q,
                _ => return None,
            }
        }
        Some(dotted(whole, src))
    }

    fn find_in_scopes<'t>(&self, from: Node<'t>, name: &str, src: &str) -> Option<Scoped<'t>> {
        find_in_scopes(from, name, src)
    }

    fn decl_type_name(&self, name: Node, src: &str) -> Option<String> {
        decl_type_name(name, src)
    }

    fn decl_initializer<'t>(&self, name: Node<'t>) -> Option<Node<'t>> {
        decl_initializer(name)
    }

    fn range_element<'t>(&self, name: Node<'t>, src: &str) -> Option<(Node<'t>, bool)> {
        let p = name.parent()?;
        if p.kind() == "foreach_statement" && is_field(p, "left", name) {
            return Some((field(p, "right")?, false));
        }
        lambda_source(name, src).map(|recv| (recv, false))
    }

    fn element_type(&self, container: &str, first: bool) -> Option<String> {
        element_type(container, first)
    }

    fn usage(&self, ident: Node) -> Usage {
        if ident.kind() != "identifier" {
            return Usage::Unknown;
        }
        if in_type_position(ident) {
            let args = ident
                .parent()
                .filter(|g| g.kind() == "generic_name")
                .and_then(|g| child_of_kind(g, "type_argument_list"))
                .map_or(0, |l| named_children(l).len());
            return Usage::TypeArgs(args);
        }
        let mut n = ident;
        if let Some(g) = n.parent().filter(|g| g.kind() == "generic_name") {
            n = g;
        }
        if let Some(p) = n.parent().filter(|p| matches!(p.kind(), "member_access_expression" | "member_binding_expression") && is_field(*p, "name", n)) {
            n = p;
        }
        if let Some(p) = n.parent().filter(|p| p.kind() == "conditional_access_expression" && named_children(*p).last().is_some_and(|l| l.id() == n.id())) {
            n = p;
        }
        match n.parent() {
            Some(call) if call.kind() == "invocation_expression" && is_field(call, "function", n) => {
                let args = field(call, "arguments").map_or(0, |a| named_children(a).into_iter().filter(|c| c.kind() == "argument").count());
                Usage::Called(args)
            }
            Some(p) if p.kind() == "argument" && is_field(p, "name", ident) => Usage::Unknown,
            _ => Usage::NotCalled,
        }
    }

    fn arity(&self, decl_name: Node, _src: &str) -> Option<(usize, Option<usize>)> {
        let decl = decl_name.parent()?;
        match decl.kind() {
            "method_declaration" | "local_function_statement" | "constructor_declaration" => {
                field(decl, "parameters").map(list_arity)
            }
            k if is_type_decl(k) && is_field(decl, "name", decl_name) => {
                let n = type_param_names(decl).len();
                Some((n, Some(n)))
            }
            _ => None,
        }
    }

    fn classify_expr<'t>(&self, expr: Node<'t>, src: &str) -> Expr<'t> {
        if is_literal(expr.kind()) {
            return Expr::Literal;
        }
        let type_of = |t: Option<Node>| t.and_then(|t| type_name_of(t, src)).map_or(Expr::Unknown, Expr::TypeName);
        match expr.kind() {
            "this" => Expr::This,
            // A class body stands for `this` (see `lambda_source`).
            "declaration_list" if expr.parent().is_some_and(|c| is_class_like(c.kind())) => Expr::This,
            "base" => Expr::Super,
            "identifier" => Expr::Name(expr),
            "generic_name" => child_of_kind(expr, "identifier").map_or(Expr::Unknown, Expr::Name),
            "invocation_expression" => {
                if let Some((recv, name)) = method_call(expr, src) {
                    let m = linq_name(name);
                    if SAME_SEQUENCE.contains(&m) {
                        return Expr::Wrapper(recv);
                    }
                    if ELEMENT.contains(&m) {
                        return Expr::Index(recv);
                    }
                }
                if let Some(t) = generic_call_type(expr, src) {
                    return Expr::TypeName(t);
                }
                self.callee_name(expr).map_or(Expr::Unknown, Expr::Call)
            }
            "member_access_expression" | "member_binding_expression" => {
                field(expr, "name").and_then(type_head).map_or(Expr::Unknown, Expr::Member)
            }
            // `a?.B.C`: the type of what comes after `?.`.
            "conditional_access_expression" => named_children(expr).into_iter().last().map_or(Expr::Unknown, Expr::Wrapper),
            "object_creation_expression" | "array_creation_expression" | "default_expression" | "cast_expression" => {
                type_of(field(expr, "type"))
            }
            "implicit_object_creation_expression" => target_type(expr, src).map_or(Expr::Unknown, Expr::TypeName),
            "as_expression" => type_of(field(expr, "right")),
            "parenthesized_expression" | "postfix_unary_expression" | "checked_expression" | "with_expression"
            | "ref_expression" => expr.named_child(0).map_or(Expr::Unknown, Expr::Wrapper),
            // `await LoadAsync()` is what the Task holds; `await
            // q.FirstOrDefaultAsync()` an element of q.
            "await_expression" => {
                let Some(inner) = expr.named_child(0) else { return Expr::Unknown };
                if let Some((recv, name)) = method_call(inner, src)
                    && name.ends_with("Async")
                {
                    let m = linq_name(name);
                    if SAME_SEQUENCE.contains(&m) {
                        return Expr::Wrapper(recv);
                    }
                    if ELEMENT.contains(&m) {
                        return Expr::Index(recv);
                    }
                }
                Expr::Index(inner)
            }
            "element_access_expression" => field(expr, "expression").map_or(Expr::Unknown, Expr::Index),
            "assignment_expression" => field(expr, "left").map_or(Expr::Unknown, Expr::Wrapper),
            "conditional_expression" => field(expr, "consequence").map_or(Expr::Unknown, Expr::Wrapper),
            // `a ?? b`
            "binary_expression" if field(expr, "operator").is_some_and(|o| text(o, src) == "??") => {
                field(expr, "left").map_or(Expr::Unknown, Expr::Wrapper)
            }
            _ => Expr::Unknown,
        }
    }

    fn callee_name<'t>(&self, call: Node<'t>) -> Option<Node<'t>> {
        match call.kind() {
            "invocation_expression" => {
                let f = unwrap_conditional(field(call, "function")?);
                match f.kind() {
                    "identifier" => Some(f),
                    "generic_name" => child_of_kind(f, "identifier"),
                    "member_access_expression" | "member_binding_expression" => field(f, "name").and_then(type_head),
                    _ => None,
                }
            }
            "object_creation_expression" => field(call, "type").and_then(type_head),
            _ => None,
        }
    }

    fn params_of<'t>(&self, decl_name: Node<'t>, _src: &str) -> Vec<Node<'t>> {
        let Some(decl) = decl_name.parent() else { return Vec::new() };
        match decl.kind() {
            "method_declaration" | "local_function_statement" | "delegate_declaration" => param_names(decl),
            // `new Circle(radius: 1)`: constructors and the primary
            // constructor.
            k if is_class_like(k) => {
                let mut out = param_names(decl);
                for m in body_members(decl).into_iter().filter(|m| m.kind() == "constructor_declaration") {
                    out.extend(param_names(m));
                }
                out
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_types() {
        assert_eq!(element_type("Circle[]", false).as_deref(), Some("Circle"));
        assert_eq!(element_type("List<Circle>", false).as_deref(), Some("Circle"));
        assert_eq!(element_type("Task<List<Circle>>", false).as_deref(), Some("List<Circle>"));
        assert_eq!(element_type("Dictionary<string, List<Circle>>", false).as_deref(), Some("List<Circle>"));
        assert_eq!(element_type("Box<Circle>", false).as_deref(), Some("Circle"));
        assert_eq!(element_type("Pair<Circle, Point>", false), None);
        assert_eq!(element_type("Circle", false), None);
    }

    #[test]
    fn preprocessor_branches() {
        let src = "class A : B\n#if X\n  , C\n#elif Y\n  , D\n#else\n  , é\n#endif\n{ }\n";
        let fixed = keep_first_branches(src).unwrap();
        assert_eq!(fixed.len(), src.len());
        assert_eq!(fixed.lines().count(), src.lines().count());
        assert_eq!(fixed.split_whitespace().collect::<Vec<_>>(), ["class", "A", ":", "B", ",", "C", "{", "}"]);
        assert_eq!(keep_first_branches("class A { }\n"), None);
    }

    #[test]
    fn csproj_usings() {
        let xml = r#"<ItemGroup><Using Include="Shop.Models" /><Using Include="Shop.Util.Mathx" Static="true"/>
            <Using Include="Shop.Other.Circle" Alias="Disk" /></ItemGroup>"#;
        let got: Vec<_> = project_usings(xml).into_iter().map(|i| (i.path, i.star, i.alias)).collect();
        assert_eq!(
            got,
            vec![
                ("Shop.Models".into(), true, None),
                ("Shop.Util.Mathx".into(), true, None),
                ("Shop.Other.Circle".into(), false, Some("Disk".into())),
            ]
        );
    }

    #[test]
    fn linq_names() {
        assert_eq!(linq_name("FirstOrDefaultAsync"), "FirstOrDefault");
        assert_eq!(linq_name("ToListAsync"), "ToList");
        assert_eq!(linq_name("LoadAsync"), "LoadAsync");
    }
}
