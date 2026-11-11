//! The per-language seam. `index.rs` does the resolution (scope walk, type
//! guesses, ranking by imports and package) and asks a [`Language`] only
//! syntax questions: which nodes declare names, which open scopes, where type
//! hints live, and what shape an expression has.
//!
//! The shared data model ([`Summary`], [`Def`], [`Import`]) is a package plus
//! imports model. [`Visibility`] says how a language reads it: JVM packages
//! (many files share one, members are visible bare) or modules (Python's
//! files, Go's package directories: only the module's own names and what the
//! file imports).

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::go::Go;
use crate::java::Java;
use crate::kotlin::Kotlin;
use crate::python::Python;

/// Every language scopehop understands, picked by file extension.
pub static LANGUAGES: &[&dyn Language] = &[&Kotlin, &Java, &Python, &Go];

pub fn for_path(path: &Path) -> Option<&'static dyn Language> {
    let ext = path.extension()?.to_str()?;
    LANGUAGES.iter().copied().find(|l| l.extensions().contains(&ext))
}

pub fn text<'a>(n: Node, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

pub(crate) fn named_children<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

pub(crate) fn child_of_kind<'t>(n: Node<'t>, kind: &str) -> Option<Node<'t>> {
    named_children(n).into_iter().find(|c| c.kind() == kind)
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

/// How a file sees declarations in other files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// Kotlin and Java: files in the same package see each other's top-level
    /// declarations, inherited members are visible as bare names, and
    /// top-level functions can be called as members (extension functions).
    Packages,
    /// Python and Go: every file belongs to the module named by
    /// [`Summary::package`] (one file in Python, a directory in Go). A bare
    /// name is the file's own, its module's, or imported (`from m import x`,
    /// `import m`, `from m import *`, Go's dot imports), and imports follow
    /// re-exports. A module can be a receiver (`m.x`). Type names may be
    /// qualified as written (`models.Circle`), read from the file that
    /// wrote them.
    Modules,
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub package: String,
    pub imports: Vec<Import>,
    pub defs: Vec<Def>,
    /// Left out of a plain build (Go build constraints such as
    /// `//go:build nomsgpack` or a `_windows.go` name). Such files are still
    /// indexed, but lookups from other files prefer their twins that are in
    /// the build.
    pub excluded: bool,
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
    /// An element of a container: `xs[i]`, Go's `<-ch`. Holds the container.
    Index(Node<'t>),
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
    /// Languages in the same family resolve each other's declarations (Kotlin
    /// and Java are both "jvm"); other families are never candidates.
    fn family(&self) -> &'static str;

    fn visibility(&self) -> Visibility {
        Visibility::Packages
    }

    // --- Indexing ----------------------------------------------------------

    /// Package, imports and the declarations other files can see.
    fn summarize(&self, root: Node, src: &str) -> Summary;

    /// Fills in what depends on where the file lives, after [`summarize`]:
    /// Python's module name comes from its path, and relative imports become
    /// absolute. `roots` are the workspace folders.
    ///
    /// [`summarize`]: Language::summarize
    fn place(&self, _summary: &mut Summary, _path: &Path, _roots: &[PathBuf]) {}

    /// An import path as written in `file` (whose summary is placed), made
    /// absolute. Only Python has relative imports (`.models`).
    fn absolute_import(&self, path: &str, _file: &Path, _summary: &Summary) -> String {
        path.to_string()
    }

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

    /// For a loop variable, the expression it ranges over, and whether it
    /// is the first of the loop's variables (Go's `k` in `for k, v := range
    /// xs`).
    fn range_element<'t>(&self, _name: Node<'t>) -> Option<(Node<'t>, bool)> {
        None
    }

    /// What ranging over or indexing a value of the type `container` (as
    /// the language writes type names) gives: the key when `first` is set,
    /// the value otherwise. `None` when it is not a container type.
    fn element_type(&self, _container: &str, _first: bool) -> Option<String> {
        None
    }

    /// What kind of expression `expr` is, for type inference.
    fn classify_expr<'t>(&self, expr: Node<'t>, src: &str) -> Expr<'t>;

    /// Name identifier of the callee in a call (`f` in `a.b.f(x)`).
    fn callee_name<'t>(&self, call: Node<'t>) -> Option<Node<'t>>;

    /// Parameter name nodes of the function or class whose name is given.
    fn params_of<'t>(&self, decl_name: Node<'t>, src: &str) -> Vec<Node<'t>>;

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
