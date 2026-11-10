//! The per-language seam. `index.rs` does the resolution (scope walk, type
//! guesses, ranking by imports and package) and asks a [`Language`] only
//! syntax questions: which nodes declare names, which open scopes, where type
//! hints live, and what shape an expression has.
//!
//! The shared data model ([`Summary`], [`Def`], [`Import`]) is a package plus
//! imports model. It fits Kotlin, Java, Scala and Go. Languages that import
//! modules by path (Python, JS) will need a visibility hook here as well.

use std::path::Path;

use tree_sitter::Node;

use crate::kotlin::Kotlin;

/// Every language scopehop understands, picked by file extension.
pub static LANGUAGES: &[&dyn Language] = &[&Kotlin];

pub fn for_path(path: &Path) -> Option<&'static dyn Language> {
    let ext = path.extension()?.to_str()?;
    LANGUAGES.iter().copied().find(|l| l.extensions().contains(&ext))
}

pub fn text<'a>(n: Node, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

// ---------------------------------------------------------------------------
// Shared data model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    Class,
    Object,
    Function,
    Property,
    EnumEntry,
    TypeAlias,
}

impl DefKind {
    /// Declares a type, so it can appear in a type position.
    pub fn is_type(self) -> bool {
        matches!(self, DefKind::Class | DefKind::Object | DefKind::TypeAlias)
    }

    /// Has members and supertypes of its own.
    pub fn is_class_like(self) -> bool {
        matches!(self, DefKind::Class | DefKind::Object)
    }
}

/// A declaration visible outside its file (top level or class member).
#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    pub kind: DefKind,
    pub start: usize,
    pub end: usize,
    /// Enclosing class/object name, `None` for top-level declarations.
    pub container: Option<String>,
    /// Declared or obvious type (`val x: Foo`, `val x = Foo()`).
    pub type_name: Option<String>,
    /// Supertypes, for classes and objects.
    pub supertypes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Import {
    /// Full path without the trailing wildcard, e.g. `com.example.Foo`.
    pub path: String,
    pub star: bool,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub package: String,
    pub imports: Vec<Import>,
    pub defs: Vec<Def>,
}

/// What a scope walk found for a name.
pub enum Scoped<'t> {
    /// The identifier that declares it.
    Decl(Node<'t>),
    /// A name declared without any identifier, e.g. Kotlin's implicit `it`.
    /// Holds the node that introduces it (the lambda).
    Implicit(Node<'t>),
}

/// The parts of an expression that type inference cares about.
pub enum Expr<'t> {
    /// A literal: always a library type.
    Literal,
    /// `this`
    This,
    /// `super`
    Super,
    /// A bare name.
    Name(Node<'t>),
    /// A call. Holds the callee (`Foo` in `Foo(...)`).
    Call(Node<'t>),
    /// `recv.member`. Holds the member's name identifier.
    Member(Node<'t>),
    /// A wrapper with the same type as its only child: `(x)`, `x!!`.
    Wrapper(Node<'t>),
    Unknown,
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Syntax knowledge for one tree-sitter grammar. Methods that take a "name"
/// node expect the identifier returned by [`Language::name_node`].
pub trait Language: Sync {
    fn name(&self) -> &'static str;
    fn grammar(&self) -> tree_sitter::Language;
    /// File extensions without the dot.
    fn extensions(&self) -> &'static [&'static str];

    // --- Indexing ----------------------------------------------------------

    /// Package, imports and the declarations other files can see.
    fn summarize(&self, root: Node, src: &str) -> Summary;

    // --- Classifying the name under the cursor -----------------------------

    /// The node for the word at `start..end`, if it can name something. This
    /// is usually an identifier, but it can also be a name inside a string
    /// template.
    fn name_node<'t>(&self, root: Node<'t>, src: &str, start: usize, end: usize) -> Option<Node<'t>>;

    /// A real identifier node. A string template name is not one, so it gets
    /// only scope and global lookups.
    fn is_identifier(&self, n: Node) -> bool;

    /// `ident` is the name being declared, not a use.
    fn is_declaration_name(&self, ident: Node) -> bool;

    /// For the label in `f(label = x)`, the call node.
    fn named_argument_call<'t>(&self, ident: Node<'t>) -> Option<Node<'t>>;

    /// For the member in `recv.member`, `recv::member` or similar, the receiver.
    fn member_receiver<'t>(&self, ident: Node<'t>) -> Option<Node<'t>>;

    /// For `Inner` in the type `Outer.Inner`, the `Outer` identifier.
    fn type_qualifier<'t>(&self, ident: Node<'t>) -> Option<Node<'t>>;

    /// `n` sits where only a type can go.
    fn in_type_position(&self, n: Node) -> bool;

    /// For an identifier in an import or package path, the dotted path up to
    /// and including it (`com.example.Foo` for `Foo` in
    /// `import com.example.Foo.bar`). For an import alias, the whole imported
    /// path. These resolve by fully qualified name, not by scope.
    fn qualified_path(&self, _ident: Node, _src: &str) -> Option<String> {
        None
    }

    // --- Scopes ------------------------------------------------------------

    /// Walks outwards from `from` to the local declaration of `name`. Stops
    /// before anything the index covers (top-level and member declarations of
    /// other files).
    fn find_in_scopes<'t>(&self, from: Node<'t>, name: &str, src: &str) -> Option<Scoped<'t>>;

    // --- Types -------------------------------------------------------------

    /// Type written on a declared name, or one that is obvious from its
    /// initializer (`val x = Foo()`).
    fn decl_type_name(&self, name: Node, src: &str) -> Option<String>;

    /// The initializer expression of a declared name (`foo` in `val x = foo`).
    fn decl_initializer<'t>(&self, name: Node<'t>) -> Option<Node<'t>>;

    /// What kind of expression `expr` is, for type inference.
    fn classify_expr<'t>(&self, expr: Node<'t>) -> Expr<'t>;

    /// Name identifier of the callee in a call (`f` in `a.b.f(x)`).
    fn callee_name<'t>(&self, call: Node<'t>) -> Option<Node<'t>>;

    /// Parameter name nodes of the function or class whose name is given.
    fn params_of<'t>(&self, decl_name: Node<'t>) -> Vec<Node<'t>>;

    /// Nearest enclosing class-like declaration.
    fn enclosing_class<'t>(&self, n: Node<'t>) -> Option<Node<'t>>;
    fn class_name(&self, class: Node, src: &str) -> Option<String>;
    fn class_supertypes(&self, class: Node, src: &str) -> Vec<String>;

    /// Receiver type of the nearest enclosing extension function, for
    /// languages that have extension functions.
    fn extension_receiver_type(&self, _n: Node, _src: &str) -> Option<String> {
        None
    }

    /// A name that resolves to nothing in the project but looks like a type
    /// (so `X.foo` is a member of library class `X`, not of a value).
    fn looks_like_type_name(&self, name: &str) -> bool {
        name.starts_with(|c: char| c.is_uppercase())
    }
}
