//! Workspace index: every source file parsed once, plus a name -> declarations
//! map. Definition = scope walk in the current file, then the index ranked by
//! imports/package. References = whole-word hits that resolve to the same place.
//!
//! Nothing here looks at node kinds; syntax questions go to the file's
//! [`Language`]. How files see each other is the language's [`Visibility`]:
//! JVM packages in `lookup_global`, or one module per file in
//! `lookup_module_global` and `resolve_import`. Only languages of the same
//! [`Language::family`] are candidates for each other.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use tree_sitter::{Node, Parser, Tree};

use crate::files::{Disk, Files};
use crate::lang::{self, Def, DefKind, Expr, Import, Language, Scoped, Summary, Usage, Visibility};
use crate::text::{self, LineIndex};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Loc {
    pub path: PathBuf,
    pub start: usize,
    pub end: usize,
}

pub struct FileData {
    pub lang: &'static dyn Language,
    pub text: String,
    pub tree: Tree,
    pub summary: Summary,
    pub lines: LineIndex,
    mtime: Option<SystemTime>,
    open: bool,
}

/// What we know about the type of an expression.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ty {
    /// A class name (may or may not be declared in the project), and for
    /// [`Visibility::Modules`] languages the file the name is written in, so
    /// that `models.Circle` and `other.Circle` stay apart.
    Class(String, Option<PathBuf>),
    /// A project module, by name (`m` after `import a.b as m`).
    Module(String),
    /// Whatever the initializer of the declaration at this location is: an
    /// untyped top-level `x = f()` in a [`Visibility::Modules`] language,
    /// worked out only when `x` is used as a receiver.
    Infer(Loc),
    /// Comes from a library: no project member can match.
    External,
    Unknown,
}


/// A resolved declaration plus what we know about its type.
#[derive(Clone, Debug)]
struct Resolved {
    loc: Loc,
    ty: Ty,
}

/// The name under the cursor (an identifier, or `$name` inside a string).
struct Target<'t> {
    node: Node<'t>,
    start: usize,
    end: usize,
    name: &'t str,
}

const RESCAN_EVERY: Duration = Duration::from_secs(3);
const MAX_DEPTH: u32 = 16;

pub struct Index {
    roots: Vec<PathBuf>,
    /// Where files are read from: the working tree, or a git revision.
    source: Box<dyn Files>,
    pub files: HashMap<PathBuf, FileData>,
    names: HashMap<String, Vec<(PathBuf, usize)>>,
    /// Module name -> file, for [`Visibility::Modules`] languages.
    modules: HashMap<String, PathBuf>,
    /// [`Summary::global_imports`] of every file, by language family and
    /// project.
    global_imports: HashMap<(&'static str, Option<PathBuf>), Vec<Import>>,
    names_dirty: bool,
    parser: Parser,
    last_scan: Option<Instant>,
}

pub fn is_source(path: &Path) -> bool {
    lang::for_path(path).is_some()
}

impl Index {
    /// An index of the working tree.
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self::with_files(roots, Box::new(Disk))
    }

    /// An index of the files `source` gives (e.g. a [`crate::files::Revision`]).
    pub fn with_files(roots: Vec<PathBuf>, source: Box<dyn Files>) -> Self {
        Self {
            roots,
            source,
            files: HashMap::new(),
            names: HashMap::new(),
            modules: HashMap::new(),
            global_imports: HashMap::new(),
            names_dirty: true,
            parser: Parser::new(),
            last_scan: None,
        }
    }

    fn build(&mut self, path: &Path, text: String, mtime: Option<SystemTime>, open: bool) -> Option<FileData> {
        let lang = lang::for_path(path)?;
        self.parser.set_language(&lang.grammar()).ok()?;
        let mut tree = self.parser.parse(&text, None)?;
        // A grammar that trips over valid code may parse a repaired copy
        // better; offsets are the same, so the tree goes with `text`.
        if tree.root_node().has_error()
            && let Some(fixed) = lang.repair(&text)
            && let Some(better) = self.parser.parse(&fixed, None)
            && error_count(&better) < error_count(&tree)
        {
            tree = better;
        }
        let mut summary = lang.summarize(tree.root_node(), &text);
        lang.place(&mut summary, path, &self.roots, self.source.as_ref());
        let lines = LineIndex::new(&text);
        Some(FileData { lang, text, tree, summary, lines, mtime, open })
    }

    /// didOpen / didChange: the editor's copy wins over the disk.
    pub fn set_open(&mut self, path: PathBuf, text: String) {
        if let Some(fd) = self.build(&path, text, None, true) {
            self.files.insert(path, fd);
            self.names_dirty = true;
        }
    }

    /// didClose: go back to the disk version on the next scan.
    pub fn close(&mut self, path: &Path) {
        if let Some(fd) = self.files.get_mut(path) {
            fd.open = false;
            fd.mtime = None;
        }
        self.last_scan = None;
    }

    /// Picks up files created, changed or deleted outside the editor.
    pub fn refresh(&mut self) {
        if self.last_scan.is_some_and(|t| t.elapsed() < RESCAN_EVERY) {
            return;
        }
        self.last_scan = Some(Instant::now());
        let mut seen = HashSet::new();
        for (path, mtime) in self.source.sources(&self.roots) {
            seen.insert(path.clone());
            match self.files.get(&path) {
                Some(fd) if fd.open || (fd.mtime.is_some() && fd.mtime == mtime) => continue,
                _ => {}
            }
            let Some(text) = self.source.read(&path) else { continue };
            if let Some(fd) = self.build(&path, text, mtime, false) {
                self.files.insert(path, fd);
                self.names_dirty = true;
            }
        }
        let before = self.files.len();
        self.files.retain(|p, fd| fd.open || seen.contains(p));
        if self.files.len() != before {
            self.names_dirty = true;
        }
    }

    fn ensure_names(&mut self) {
        if !self.names_dirty {
            return;
        }
        self.names.clear();
        self.modules.clear();
        self.global_imports.clear();
        // Sorted so candidate lists, and every choice made from them (ranking
        // ties, `type_of` taking the first hit, supertype walk order), do not
        // depend on the map's random seed.
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by(|a, b| a.0.cmp(b.0));
        for (path, fd) in files {
            if fd.lang.visibility() == Visibility::Modules && !fd.summary.package.is_empty() {
                // `pkg.py` and `pkg/__init__.py`, or a Go package's files: the
                // first one (sorted) wins, preferring files in the build.
                let excluded = |p: &PathBuf| self.files.get(p).is_some_and(|f| f.summary.excluded);
                match self.modules.get(&fd.summary.package) {
                    Some(have) if !excluded(have) || fd.summary.excluded => {}
                    _ => {
                        self.modules.insert(fd.summary.package.clone(), path.clone());
                    }
                }
            }
            if !fd.summary.global_imports.is_empty() {
                let all = self.global_imports.entry((fd.lang.family(), fd.summary.project.clone())).or_default();
                all.extend(fd.summary.global_imports.iter().cloned());
            }
            for (i, d) in fd.summary.defs.iter().enumerate() {
                self.names
                    .entry(d.name.clone())
                    .or_default()
                    .push((path.clone(), i));
            }
        }
        self.names_dirty = false;
    }

    fn prepare(&mut self) {
        self.refresh();
        self.ensure_names();
    }

    // -----------------------------------------------------------------------
    // Requests
    // -----------------------------------------------------------------------

    pub fn definition_at(&mut self, path: &Path, offset: usize) -> Vec<Loc> {
        self.prepare();
        let Some(fd) = self.files.get(path) else { return Vec::new() };
        let Some(t) = target_at(fd, offset) else { return Vec::new() };
        let mut seen = HashSet::new();
        let locs: Vec<Loc> =
            self.resolve(path, fd, &t, 0).into_iter().map(|r| r.loc).filter(|l| seen.insert(l.clone())).collect();
        self.main_part(locs)
    }

    /// The parts of one partial class (C#: one name, one package, several
    /// declarations) are one definition: the part in the file named after
    /// the class, else the one that names the supertypes, else the first.
    /// References still count every part.
    fn main_part(&self, locs: Vec<Loc>) -> Vec<Loc> {
        if locs.len() < 2 {
            return locs;
        }
        let parts: Option<Vec<(&Loc, &FileData, &Def)>> = locs
            .iter()
            .map(|l| {
                let f = self.files.get(&l.path)?;
                let d = f.summary.defs.iter().find(|d| d.start == l.start)?;
                (f.lang.visibility() == Visibility::Packages && d.kind.is_class_like()).then_some((l, f, d))
            })
            .collect();
        let Some(parts) = parts else { return locs };
        let (_, f0, d0) = parts[0];
        let one_class = parts.iter().all(|(_, f, d)| {
            d.name == d0.name && d.container == d0.container && f.summary.package == f0.summary.package
        });
        if !one_class {
            return locs;
        }
        let named = |l: &Loc| l.path.file_stem().is_some_and(|s| s.to_str() == Some(d0.name.as_str()));
        let main = parts
            .iter()
            .find(|(l, _, _)| named(l))
            .or_else(|| parts.iter().find(|(_, _, d)| !d.supertypes.is_empty()))
            .unwrap_or(&parts[0]);
        vec![main.0.clone()]
    }

    pub fn references_at(
        &mut self,
        path: &Path,
        offset: usize,
        include_decl: bool,
        this_file_only: bool,
    ) -> Vec<Loc> {
        self.prepare();
        let Some(fd) = self.files.get(path) else { return Vec::new() };
        let Some(t) = target_at(fd, offset) else { return Vec::new() };
        let targets: HashSet<Loc> = self.resolve(path, fd, &t, 0).into_iter().map(|r| r.loc).collect();
        if targets.is_empty() {
            return Vec::new();
        }
        // Locals can only be used in their own file.
        let local = targets.iter().all(|l| l.path == path && !self.is_indexed_def(l));
        let paths: Vec<&PathBuf> = if local || this_file_only {
            self.files.get_key_value(path).map(|(p, _)| p).into_iter().collect()
        } else {
            self.files.keys().collect()
        };
        let name = t.name;
        let mut out = Vec::new();
        for p in paths {
            let fd = &self.files[p];
            for off in text::word_occurrences(&fd.text, name) {
                let Some(hit) = target_at(fd, off) else { continue };
                if hit.start != off {
                    continue;
                }
                let loc = Loc { path: p.clone(), start: hit.start, end: hit.end };
                if !include_decl && targets.contains(&loc) {
                    continue;
                }
                if self.resolve(p, fd, &hit, 0).iter().any(|r| targets.contains(&r.loc)) {
                    out.push(loc);
                }
            }
        }
        out.sort();
        out
    }

    pub fn workspace_symbols(&mut self, query: &str) -> Vec<(PathBuf, Def)> {
        self.prepare();
        let q = query.to_lowercase();
        let mut out = Vec::new();
        for (path, fd) in &self.files {
            // Synthetic names (Go's anonymous structs) are not symbols.
            for d in fd.summary.defs.iter().filter(|d| !d.name.contains('@')) {
                if fuzzy_match(&d.name.to_lowercase(), &q) {
                    out.push((path.clone(), d.clone()));
                }
            }
        }
        out.sort_by(|(pa, a), (pb, b)| {
            (a.name.len(), &a.name, pa, a.start).cmp(&(b.name.len(), &b.name, pb, b.start))
        });
        out.truncate(256);
        out
    }

    // -----------------------------------------------------------------------
    // Resolution
    // -----------------------------------------------------------------------

    fn is_indexed_def(&self, loc: &Loc) -> bool {
        self.files
            .get(&loc.path)
            .is_some_and(|fd| fd.summary.defs.iter().any(|d| d.start == loc.start))
    }

    /// [`Index::resolve_any`], preferring declarations from files that are
    /// in the build when the asking file is, and the other way round, so a
    /// name declared in both `binding.go` and its `//go:build nomsgpack`
    /// twin has one answer.
    fn resolve(&self, path: &Path, fd: &FileData, t: &Target, depth: u32) -> Vec<Resolved> {
        let all = self.resolve_any(path, fd, t, depth);
        let excluded = |r: &Resolved| self.files.get(&r.loc.path).is_some_and(|f| f.summary.excluded);
        let (same, other): (Vec<_>, Vec<_>) = all.into_iter().partition(|r| excluded(r) == fd.summary.excluded);
        let hits = if same.is_empty() { other } else { same };
        self.by_usage(fd, t.node, hits)
    }

    /// Overloads that take as many arguments as the call passes (when any
    /// do); for a name that is not called, no extension functions.
    fn by_usage(&self, fd: &FileData, at: Node, hits: Vec<Resolved>) -> Vec<Resolved> {
        if hits.is_empty() || !fd.lang.is_identifier(at) || fd.lang.is_declaration_name(at) {
            return hits;
        }
        match fd.lang.usage(at) {
            Usage::Unknown => hits,
            Usage::NotCalled => hits.into_iter().filter(|r| !self.def_at(&r.loc).is_some_and(is_extension)).collect(),
            Usage::Called(n) if hits.len() > 1 => {
                let fits: Vec<(Resolved, bool)> =
                    hits.iter().filter_map(|r| self.accepts(r, n).map(|exact| (r.clone(), exact))).collect();
                // A fit without a variable argument list wins.
                let exact: Vec<Resolved> = fits.iter().filter(|f| f.1).map(|f| f.0.clone()).collect();
                match (exact.is_empty(), fits.is_empty()) {
                    (false, _) => exact,
                    (true, false) => fits.into_iter().map(|f| f.0).collect(),
                    (true, true) => hits,
                }
            }
            Usage::TypeArgs(n) if hits.len() > 1 => {
                let fits: Vec<Resolved> = hits.iter().filter(|r| self.accepts(r, n).is_some()).cloned().collect();
                if fits.is_empty() { hits } else { fits }
            }
            Usage::Called(_) | Usage::TypeArgs(_) => hits,
        }
    }

    fn def_at(&self, loc: &Loc) -> Option<&Def> {
        self.files.get(&loc.path)?.summary.defs.iter().find(|d| d.start == loc.start)
    }

    /// Whether a function at `r` can take `n` arguments (or `n + 1`, the
    /// receiver of an extension function being one): `Some(true)` without
    /// a variable argument list, `Some(false)` with one. Anything that is
    /// not a function with a known arity may be callable.
    fn accepts(&self, r: &Resolved, n: usize) -> Option<bool> {
        let Some(f) = self.files.get(&r.loc.path) else { return Some(true) };
        let Some(name) = f.tree.root_node().descendant_for_byte_range(r.loc.start, r.loc.end) else { return Some(true) };
        let Some((min, max)) = f.lang.arity(name, &f.text) else { return Some(true) };
        let fits = |n: usize| n >= min && max.is_none_or(|m| n <= m);
        (fits(n) || (self.def_at(&r.loc).is_some_and(is_extension) && fits(n + 1))).then_some(max.is_some())
    }

    fn resolve_any(&self, path: &Path, fd: &FileData, t: &Target, depth: u32) -> Vec<Resolved> {
        let src = fd.text.as_str();
        let lang = fd.lang;
        let here = |s, e| Loc { path: path.to_path_buf(), start: s, end: e };
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        // Import paths can be strings (Go), so this comes before the
        // identifier-only cases.
        if let Some(fq) = lang.qualified_path(t.node, src) {
            let fq = lang.absolute_import(&fq, path, &fd.summary, self.source.as_ref());
            return self.resolve_import(lang, &fq, 0);
        }
        if lang.is_identifier(t.node) {
            if lang.is_declaration_name(t.node) {
                let ty = self.local_type(path, fd, t.node, depth);
                return vec![Resolved { loc: here(t.start, t.end), ty }];
            }
            if let Some(call) = lang.named_argument_call(t.node) {
                return self.named_argument(path, fd, call, t.name, depth);
            }
            if let Some(recv) = lang.member_receiver(t.node) {
                let ty = self.type_of(path, fd, recv, depth + 1);
                let hits = self.lookup_members(lang, t.name, &ty, path);
                return if hits.is_empty() { self.dotted(fd, t.node) } else { hits };
            }
            // Outer.Inner in a type: Inner is a member of Outer.
            if let Some(outer) = lang.type_qualifier(t.node) {
                let outer = self.class_ty(Some(lang::text(outer, src).to_string()), path);
                let hits = self.lookup_members(lang, t.name, &outer, path);
                return if hits.is_empty() { self.dotted(fd, t.node) } else { hits };
            }
        }
        match lang.find_in_scopes(t.node, t.name, src) {
            Some(Scoped::Implicit(n)) => {
                let loc = here(n.start_byte(), n.start_byte() + 1);
                return vec![Resolved { loc, ty: Ty::Unknown }];
            }
            Some(Scoped::Decl(d)) => {
                let ty = self.local_type(path, fd, d, depth);
                return vec![Resolved { loc: here(d.start_byte(), d.end_byte()), ty }];
            }
            None => {}
        }
        if lang.visibility() == Visibility::Modules {
            return self.lookup_module_global(t.name, path, fd);
        }
        let types_only = lang.in_type_position(t.node);
        // Inside `fun Foo.ext()`, bare names may be members of Foo. Types are
        // not: the receiver brings members into scope, not nested classes, and
        // this would otherwise look `Foo` itself up as a member of `Foo`.
        let mut allow_members = true;
        if let Some(recv) = lang.extension_receiver_type(t.node, src).filter(|_| !types_only) {
            let hits: Vec<_> = self
                .lookup_members(lang, t.name, &self.class_ty(Some(recv), path), path)
                .into_iter()
                .filter(|r| !self.is_top_level(&r.loc))
                .collect();
            if !hits.is_empty() {
                return hits;
            }
            allow_members = false;
        }
        self.lookup_global(t.name, path, fd, t.node, allow_members, types_only)
    }

    /// `A.B.C` read as a namespace path (see [`Language::dotted_path`]):
    /// fully qualified, under the file's package or one of its parents, or
    /// under a star import. The first part may be an import alias.
    fn dotted(&self, fd: &FileData, ident: Node) -> Vec<Resolved> {
        let Some(dotted) = fd.lang.dotted_path(ident, &fd.text) else { return Vec::new() };
        self.qualified_candidates(fd, &dotted)
            .iter()
            .map(|c| self.resolve_import(fd.lang, c, 0))
            .find(|hits| !hits.is_empty())
            .unwrap_or_default()
    }

    /// The fully qualified names a qualified name written in `fd` may stand
    /// for, best first: as written, under the file's package and its
    /// parents, then under each star import. A leading import alias is
    /// expanded first.
    fn qualified_candidates(&self, fd: &FileData, dotted: &str) -> Vec<String> {
        let imports: Vec<&Import> = self.imports_of(fd).collect();
        let (first, rest) = dotted.split_once('.').unwrap_or((dotted, ""));
        let expanded = match imports.iter().find(|i| !i.star && i.alias.as_deref() == Some(first)) {
            Some(i) if rest.is_empty() => i.path.clone(),
            Some(i) => format!("{}.{rest}", i.path),
            None => dotted.to_string(),
        };
        let mut candidates = vec![expanded.clone()];
        let mut pkg = fd.summary.package.as_str();
        while !pkg.is_empty() {
            candidates.push(format!("{pkg}.{expanded}"));
            pkg = pkg.rsplit_once('.').map_or("", |(p, _)| p);
        }
        candidates.extend(imports.iter().filter(|i| i.star).map(|i| format!("{}.{expanded}", i.path)));
        candidates
    }

    /// The file's imports, then the global ones of its project.
    fn imports_of<'a>(&'a self, fd: &'a FileData) -> impl Iterator<Item = &'a Import> {
        let key = (fd.lang.family(), fd.summary.project.clone());
        let globals = self.global_imports.get(&key).map(Vec::as_slice).unwrap_or_default();
        fd.summary.imports.iter().chain(globals)
    }

    fn is_top_level(&self, loc: &Loc) -> bool {
        self.files.get(&loc.path).is_some_and(|fd| {
            fd.summary
                .defs
                .iter()
                .any(|d| d.start == loc.start && d.container.is_none())
        })
    }

    /// Type of a declared name: annotation, `= Foo()`, or the initializer's type.
    fn local_type(&self, path: &Path, fd: &FileData, name: Node, depth: u32) -> Ty {
        if let Some(t) = fd.lang.decl_type_name(name, &fd.text) {
            return self.class_ty(Some(t), path);
        }
        // A member found by the scope walk (a method called without a
        // receiver): the index knows its return type.
        let indexed = fd.summary.defs.iter().find(|d| d.start == name.start_byte());
        if let Some(t) = indexed.and_then(|d| d.type_name.clone()) {
            return self.class_ty(Some(t), path);
        }
        if let Some((over, first)) = fd.lang.range_element(name, &fd.text) {
            let ty = self.type_of(path, fd, over, depth + 1);
            return self.element_of(fd.lang, ty, first, depth);
        }
        if let Some((from, keys)) = fd.lang.destructured(name, &fd.text) {
            let mut ty = self.type_of(path, fd, from, depth + 1);
            for key in keys {
                let hits = self.lookup_members(fd.lang, &key, &self.settle(ty, depth), path);
                ty = likely_type(&hits, path).unwrap_or(Ty::Unknown);
            }
            return self.settle(ty, depth);
        }
        match fd.lang.decl_initializer(name) {
            Some(init) => self.type_of(path, fd, init, depth + 1),
            None => Ty::Unknown,
        }
    }

    /// The element type of a container type (see [`Language::element_type`]),
    /// following named types to what they are declared as (`type Routes
    /// []Route`).
    fn element_of(&self, lang: &dyn Language, ty: Ty, first: bool, depth: u32) -> Ty {
        let ty = self.settle(ty, depth);
        let Ty::Class(name, ctx) = ty else {
            return if ty == Ty::External { Ty::External } else { Ty::Unknown };
        };
        let mut queue = vec![(name, ctx)];
        let mut seen = HashSet::new();
        while let Some((name, ctx)) = queue.pop() {
            if !seen.insert((name.clone(), ctx.clone())) || seen.len() > 16 {
                continue;
            }
            if let Some(e) = lang.element_type(&name, first) {
                // Packages: the element's own class, seen from where the
                // container type was written.
                let ctx = match lang.visibility() {
                    Visibility::Modules => ctx,
                    Visibility::Packages => ctx.and_then(|f| self.class_context(&e, &f)),
                };
                return Ty::Class(e, ctx);
            }
            let Some(classes) = ctx.as_deref().and_then(|c| self.classes_seen_from(&name, c)) else { continue };
            for (p, d) in classes {
                queue.extend(d.supertypes.iter().map(|s| (s.clone(), self.class_context(s, p))));
            }
        }
        Ty::Unknown
    }

    /// `label` in `callee(label = x)`: the callee's parameter.
    fn named_argument(&self, path: &Path, fd: &FileData, call: Node, label: &str, depth: u32) -> Vec<Resolved> {
        let Some(callee) = fd.lang.callee_name(call) else { return Vec::new() };
        let mut out = Vec::new();
        let mut params = |f: &FileData, at: &Path, decl: Node| {
            for p in f.lang.params_of(decl, &f.text) {
                if lang::text(p, &f.text) == label {
                    out.push(Resolved {
                        loc: Loc { path: at.to_path_buf(), start: p.start_byte(), end: p.end_byte() },
                        ty: self.class_ty(f.lang.decl_type_name(p, &f.text), at),
                    });
                }
            }
        };
        // Not a name: the callee is its own declaration (Go's `struct{...}`).
        if !fd.lang.is_identifier(callee) {
            params(fd, path, callee);
            return out;
        }
        let t = self.target_for(fd, callee);
        for r in self.resolve(path, fd, &t, depth + 1) {
            let Some(f) = self.files.get(&r.loc.path) else { continue };
            let Some(name) = f.tree.root_node().descendant_for_byte_range(r.loc.start, r.loc.end) else { continue };
            params(f, &r.loc.path, name);
        }
        out
    }

    fn target_for<'t>(&self, fd: &'t FileData, node: Node<'t>) -> Target<'t> {
        Target {
            node,
            start: node.start_byte(),
            end: node.end_byte(),
            name: &fd.text[node.byte_range()],
        }
    }

    /// Best guess at the type of an expression (usually the part before `.`).
    fn type_of(&self, path: &Path, fd: &FileData, expr: Node, depth: u32) -> Ty {
        if depth > MAX_DEPTH {
            return Ty::Unknown;
        }
        let src = fd.text.as_str();
        let lang = fd.lang;
        match lang.classify_expr(expr, src) {
            Expr::Literal => Ty::External,
            Expr::This => self.class_ty(
                lang.extension_receiver_type(expr, src)
                    .or_else(|| lang.enclosing_class(expr).and_then(|c| lang.class_name(c, src))),
                path,
            ),
            Expr::Super => self.class_ty(
                lang.enclosing_class(expr)
                    .and_then(|c| lang.class_supertypes(c, src).into_iter().next()),
                path,
            ),
            Expr::Name(n) => {
                let t = self.target_for(fd, n);
                let hits = self.resolve(path, fd, &t, depth + 1);
                if let Some(ty) = likely_type(&hits, path) {
                    return self.settle(ty, depth);
                }
                // Not in the project: a library class (Modifier) or value. A
                // member (`x.Foo`) is never a class.
                if lang.looks_like_type_name(t.name) && lang.member_receiver(n).is_none() {
                    self.class_ty(Some(t.name.to_string()), path)
                } else {
                    Ty::External
                }
            }
            // Foo(...) -> Foo, makeFoo() -> declared return type
            Expr::Call(callee) => self.type_of(path, fd, callee, depth + 1),
            Expr::Member(name) => {
                let t = self.target_for(fd, name);
                let ty = likely_type(&self.resolve(path, fd, &t, depth + 1), path).unwrap_or(Ty::External);
                self.settle(ty, depth)
            }
            Expr::Wrapper(inner) => self.type_of(path, fd, inner, depth + 1),
            Expr::TypeName(t) => self.class_ty(Some(t), path),
            Expr::Index(container) => {
                let ty = self.type_of(path, fd, container, depth + 1);
                self.element_of(lang, ty, false, depth)
            }
            Expr::Unknown => Ty::Unknown,
        }
    }

    /// Works out a [`Ty::Infer`].
    fn settle(&self, ty: Ty, depth: u32) -> Ty {
        let Ty::Infer(loc) = ty else { return ty };
        let Some(fd) = self.files.get(&loc.path) else { return Ty::Unknown };
        let name = fd.tree.root_node().descendant_for_byte_range(loc.start, loc.end);
        match name.and_then(|n| fd.lang.decl_initializer(n)) {
            Some(init) => self.type_of(&loc.path, fd, init, depth + 1),
            None => Ty::Unknown,
        }
    }

    /// Declarations named `name` in files of `lang`'s family.
    fn defs_named<'a>(&'a self, lang: &dyn Language, name: &str) -> Vec<(&'a PathBuf, &'a FileData, &'a Def)> {
        let Some(list) = self.names.get(name) else { return Vec::new() };
        let family = lang.family();
        list.iter()
            .filter_map(|(p, i)| {
                let fd = self.files.get(p)?;
                (fd.lang.family() == family).then_some(())?;
                Some((p, fd, fd.summary.defs.get(*i)?))
            })
            .collect()
    }

    /// A module, as a location (the start of its file).
    fn module(&self, name: &str) -> Option<Resolved> {
        let path = self.modules.get(name)?;
        Some(Resolved {
            loc: Loc { path: path.clone(), start: 0, end: 0 },
            ty: Ty::Module(name.to_string()),
        })
    }

    /// What an absolute import path names: a declaration with that fully
    /// qualified name, a module, or (for modules) a name the module itself
    /// imports, so `from pkg import Foo` reaches `Foo` when `pkg/__init__.py`
    /// does `from .models import Foo`.
    fn resolve_import(&self, lang: &dyn Language, fq: &str, depth: u32) -> Vec<Resolved> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let last = fq.rsplit('.').next().unwrap_or(fq);
        let defs: Vec<Resolved> = self
            .defs_named(lang, last)
            .into_iter()
            .filter(|(_, f, d)| fq_name(f, d) == fq)
            .map(|(p, _, d)| self.resolved(p, d))
            .collect();
        if !defs.is_empty() {
            return defs;
        }
        if let Some(m) = self.module(fq) {
            return vec![m];
        }
        let Some((parent, last)) = fq.rsplit_once('.') else { return Vec::new() };
        let Some(mfd) = self.modules.get(parent).and_then(|p| self.files.get(p)) else { return Vec::new() };
        for imp in mfd.summary.imports.iter().filter(|i| !i.star && binding(i) == last) {
            let hits = self.resolve_import(lang, &imp.path, depth + 1);
            if !hits.is_empty() {
                return hits;
            }
        }
        for imp in mfd.summary.imports.iter().filter(|i| i.star) {
            let hits = self.resolve_import(lang, &format!("{}.{last}", imp.path), depth + 1);
            if !hits.is_empty() {
                return hits;
            }
        }
        Vec::new()
    }

    /// A bare name in a [`Visibility::Modules`] file that no enclosing scope
    /// declares: the file's own top level, the rest of its module's (a Go
    /// package's other files), then its imports.
    fn lookup_module_global(&self, name: &str, from: &Path, fd: &FileData) -> Vec<Resolved> {
        let top: Vec<_> = self
            .defs_named(fd.lang, name)
            .into_iter()
            .filter(|(_, _, d)| d.container.is_none())
            .collect();
        let pick = |keep: &dyn Fn(&Path) -> bool| -> Vec<Resolved> {
            top.iter()
                .filter(|(p, _, _)| keep(p))
                .map(|(p, _, d)| self.resolved(p, d))
                .collect()
        };
        let own = pick(&|p| p == from);
        if !own.is_empty() {
            return own;
        }
        // A name this file imports is that import, even from outside the
        // project. Go forbids a package-level name that clashes with it, so
        // a same-named one in another file of the module is from another
        // build (`var json` in a `//go:build jsoniter` file).
        let bound: Vec<_> = fd.summary.imports.iter().filter(|i| !i.star && binding(i) == name).collect();
        if !bound.is_empty() {
            return bound
                .iter()
                .map(|imp| self.resolve_import(fd.lang, &imp.path, 0))
                .find(|hits| !hits.is_empty())
                .unwrap_or_default();
        }
        let module = pick(&|p| self.same_module(p, from));
        if !module.is_empty() {
            return module;
        }
        for imp in fd.summary.imports.iter().filter(|i| i.star) {
            let hits = self.resolve_import(fd.lang, &format!("{}.{name}", imp.path), 0);
            if !hits.is_empty() {
                return hits;
            }
        }
        Vec::new()
    }

    fn resolved(&self, path: &Path, d: &Def) -> Resolved {
        let loc = Loc { path: path.to_path_buf(), start: d.start, end: d.end };
        let ty = match &d.type_name {
            None if d.kind == DefKind::Property && self.type_context(path).is_some() => Ty::Infer(loc.clone()),
            t => self.class_ty(t.clone(), path),
        };
        Resolved { loc, ty }
    }

    /// Two files whose declarations belong to one class: one module, or
    /// for [`Visibility::Packages`] one package.
    fn same_scope(&self, a: &Path, b: &Path) -> bool {
        let (Some(fa), Some(fb)) = (self.files.get(a), self.files.get(b)) else { return false };
        match fa.lang.visibility() {
            Visibility::Modules => self.same_module(a, b),
            Visibility::Packages => fa.lang.family() == fb.lang.family() && fa.summary.package == fb.summary.package,
        }
    }

    /// Two files of one [`Visibility::Modules`] module.
    fn same_module(&self, a: &Path, b: &Path) -> bool {
        if a == b {
            return true;
        }
        let (Some(fa), Some(fb)) = (self.files.get(a), self.files.get(b)) else { return false };
        fa.lang.family() == fb.lang.family()
            && fa.lang.visibility() == Visibility::Modules
            && !fa.summary.package.is_empty()
            && fa.summary.package == fb.summary.package
    }

    /// The type named `name` in `file`.
    fn class_ty(&self, name: Option<String>, file: &Path) -> Ty {
        let Some(mut name) = name else { return Ty::Unknown };
        // `import a.b.Foo as Bar` / `using Bar = A.B.Foo;`: a Bar is a Foo.
        if let Some(fd) = self.files.get(file).filter(|f| f.lang.visibility() == Visibility::Packages)
            && let Some(imp) = self.imports_of(fd).find(|i| !i.star && i.alias.as_deref() == Some(name.as_str()))
        {
            name = imp.path.rsplit('.').next().unwrap_or(&imp.path).to_string();
        }
        let ctx = self.class_context(&name, file);
        Ty::Class(name, ctx)
    }

    /// `file`, when its language tells same-named classes apart by where the
    /// name is written.
    fn type_context(&self, file: &Path) -> Option<PathBuf> {
        let fd = self.files.get(file)?;
        (fd.lang.visibility() == Visibility::Modules).then(|| file.to_path_buf())
    }

    /// What tells the class `name`, written in `file`, apart from others
    /// with the same name. For [`Visibility::Modules`], `file` itself. For
    /// [`Visibility::Packages`], the first file declaring the one class
    /// `file` sees by that name (its own, its package's or an imported one),
    /// so that every mention of a class gets the same context; `None` when
    /// it sees none or several (from a library, or ambiguous).
    fn class_context(&self, name: &str, file: &Path) -> Option<PathBuf> {
        let fd = self.files.get(file)?;
        if fd.lang.visibility() == Visibility::Modules {
            return Some(file.to_path_buf());
        }
        let base = name.split('<').next().unwrap_or(name);
        // `Models.Circle`: the class with that qualified name.
        if let Some((_, last)) = base.rsplit_once('.') {
            let classes: Vec<_> =
                self.defs_named(fd.lang, last).into_iter().filter(|(_, _, d)| d.kind.is_class_like()).collect();
            return self
                .qualified_candidates(fd, base)
                .iter()
                .find_map(|c| classes.iter().find(|(_, f, d)| &fq_name(f, d) == c))
                .map(|(p, _, _)| (*p).clone());
        }
        let classes: Vec<_> =
            self.defs_named(fd.lang, base).into_iter().filter(|(_, _, d)| d.kind.is_class_like()).collect();
        if classes.is_empty() {
            // A library container (`List<Circle>`): its element is read
            // from where it was written.
            return fd.lang.element_type(name, false).map(|_| file.to_path_buf());
        }
        let imports: Vec<&Import> = self.imports_of(fd).collect();
        let package_first = fd.lang.package_before_imports();
        let tier = |p: &PathBuf, f: &FileData, d: &Def| -> Option<u8> {
            let top = d.container.is_none();
            let same_package = top && f.summary.package == fd.summary.package;
            let fq = fq_name(f, d);
            let imported = imports.iter().any(|i| {
                (!i.star && i.path == fq) || (i.star && fq.rsplit_once('.').map(|(q, _)| q) == Some(i.path.as_str()))
            });
            if p.as_path() == file {
                Some(0)
            } else if same_package && (package_first || !imported) {
                Some(if package_first { 1 } else { 2 })
            } else if imported {
                Some(if package_first { 2 } else { 1 })
            } else {
                None
            }
        };
        let best = classes.iter().filter_map(|(p, f, d)| tier(p, f, d)).min()?;
        let seen: Vec<_> = classes.iter().filter(|(p, f, d)| tier(p, f, d) == Some(best)).collect();
        let package = &seen[0].1.summary.package;
        seen.iter().all(|(_, f, _)| &f.summary.package == package).then(|| seen[0].0.clone())
    }

    /// The classes `name` refers to in `file` (imported, or declared there).
    /// `None` when that is unknown, so every class with that name counts. A
    /// qualified name (`models.Circle`) is looked up in the module its
    /// qualifier names; a qualifier from outside the project names no class.
    /// For [`Visibility::Packages`], `file` is a [`Index::class_context`],
    /// and the classes are the ones named `name` in its package (all parts
    /// of a partial class).
    fn classes_seen_from(&self, name: &str, file: &Path) -> Option<Vec<(&PathBuf, &Def)>> {
        let fd = self.files.get(file)?;
        if fd.lang.visibility() == Visibility::Packages {
            let base = name.split('<').next().unwrap_or(name);
            let base = base.rsplit('.').next().unwrap_or(base);
            let pkg = &fd.summary.package;
            return Some(
                self.defs_named(fd.lang, base)
                    .into_iter()
                    .filter(|(_, f, d)| d.kind.is_class_like() && &f.summary.package == pkg)
                    .map(|(p, _, d)| (p, d))
                    .collect(),
            );
        }
        let (base, found) = match name.rsplit_once('.') {
            Some((qual, base)) => {
                let module = self.lookup_module_global(qual, file, fd).into_iter().find_map(|r| match r.ty {
                    Ty::Module(m) => Some(m),
                    _ => None,
                });
                let Some(m) = module else { return Some(Vec::new()) };
                (base, self.resolve_import(fd.lang, &format!("{m}.{base}"), 0))
            }
            None => (name, self.lookup_module_global(name, file, fd)),
        };
        let all: Vec<(&PathBuf, &Def)> = self
            .defs_named(fd.lang, base)
            .into_iter()
            .filter(|(_, _, d)| d.kind.is_class_like())
            .map(|(p, _, d)| (p, d))
            .collect();
        let locs: Vec<Loc> = found.into_iter().map(|r| r.loc).collect();
        let seen: Vec<_> = all
            .iter()
            .copied()
            .filter(|(p, d)| locs.iter().any(|l| &l.path == *p && l.start == d.start))
            .collect();
        if !seen.is_empty() {
            return Some(seen);
        }
        // A nested class of this file.
        let here: Vec<_> = all.into_iter().filter(|(p, _)| p.as_path() == file).collect();
        (!here.is_empty()).then_some(here)
    }

    /// `name` as a member of the receiver's class (walking supertypes), or
    /// any member when the receiver's type is unknown. `lang` is the language
    /// of the file asking.
    fn lookup_members(&self, lang: &dyn Language, name: &str, recv: &Ty, from: &Path) -> Vec<Resolved> {
        if let Ty::Module(m) = recv {
            return self.resolve_import(lang, &format!("{m}.{name}"), 0);
        }
        // Only JVM-style languages call top-level functions as members.
        let extensions = lang.visibility() == Visibility::Packages;
        let defs = self.defs_named(lang, name);
        let rank = |mut v: Vec<(&PathBuf, &Def)>| {
            v.sort_by_key(|(p, _)| p.as_path() != from);
            v.into_iter().map(|(p, d)| self.resolved(p, d)).collect::<Vec<_>>()
        };
        let extensions_only = || {
            if !extensions {
                return Vec::new();
            }
            rank(
                defs.iter()
                    .filter(|(_, _, d)| d.container.is_none() && d.kind == DefKind::Function)
                    .map(|(p, _, d)| (*p, *d))
                    .collect(),
            )
        };
        if *recv == Ty::External {
            return extensions_only();
        }
        if let Ty::Class(class, ctx) = recv {
            let mut queue = vec![(class.to_string(), ctx.clone())];
            let mut seen = HashSet::new();
            while let Some((cls, ctx)) = queue.pop() {
                if !seen.insert((cls.clone(), ctx.clone())) || seen.len() > 32 {
                    continue;
                }
                // Which declarations of `cls` this is, when the language can
                // tell. Their members may be in other files of their module
                // (Go methods).
                let classes = ctx.as_deref().and_then(|c| self.classes_seen_from(&cls, c));
                // `Box<int>` has the members of `Box`, `models.Circle` and
                // `Models.Circle` those of `Circle`.
                let base = cls.split('<').next().unwrap_or(&cls);
                let base = base.rsplit('.').next().unwrap_or(base);
                let in_class =
                    |p: &PathBuf| classes.as_ref().is_none_or(|cs| cs.iter().any(|(cp, _)| self.same_scope(cp, p)));
                let hits: Vec<_> = defs
                    .iter()
                    .filter(|(p, _, d)| d.container.as_deref() == Some(base) && in_class(p))
                    .map(|(p, _, d)| (*p, *d))
                    .collect();
                if !hits.is_empty() {
                    return rank(hits);
                }
                let decls: Vec<(&PathBuf, &Def)> = match &classes {
                    Some(cs) => cs.clone(),
                    None => self.defs_named(lang, base).into_iter().map(|(p, _, d)| (p, d)).collect(),
                };
                for (p, d) in decls {
                    if d.kind.is_class_like() {
                        queue.extend(d.supertypes.iter().map(|s| (s.clone(), self.class_context(s, p))));
                    }
                }
            }
            // Library class or inherited from outside the project: only
            // extension functions can match.
            return extensions_only();
        }
        // An unknown receiver can be any class, but not an object literal or
        // anonymous struct (synthetic `@` names): those are only reached
        // through a value known to be them.
        rank(
            defs.iter()
                .filter(|(_, _, d)| match &d.container {
                    Some(c) => !c.contains('@'),
                    None => extensions && d.kind == DefKind::Function,
                })
                .map(|(p, _, d)| (*p, *d))
                .collect(),
        )
    }

    /// A bare name not declared in any enclosing scope of this file.
    fn lookup_global(
        &self,
        name: &str,
        from: &Path,
        fd: &FileData,
        at: Node,
        allow_members: bool,
        types_only: bool,
    ) -> Vec<Resolved> {
        let mut defs = self.defs_named(fd.lang, name);
        let imports: Vec<&Import> = self.imports_of(fd).collect();
        // import a.b.Foo as Bar
        if let Some(imp) = imports.iter().find(|i| i.alias.as_deref() == Some(name)) {
            if let Some(last) = imp.path.rsplit('.').next() {
                defs = self.defs_named(fd.lang, last);
                defs.retain(|(_, f, d)| fq_name(f, d) == imp.path);
            }
        }
        if types_only {
            defs.retain(|(_, _, d)| d.kind.is_type());
        }
        if defs.is_empty() {
            return Vec::new();
        }
        let pkg = &fd.summary.package;
        let enclosing = fd.lang.enclosing_class(at);
        let enclosing_supers: HashMap<String, u8> =
            enclosing.map(|c| self.supertype_closure(fd, c)).unwrap_or_default();
        // Members of the class's other parts (C#'s partial classes).
        let enclosing_name = enclosing.and_then(|c| fd.lang.class_name(c, &fd.text));

        // Lower is better; None means the language would not see it from here.
        // Inherited members rank by how far up they are declared, so an
        // override beats the interface it implements.
        let tier = |p: &PathBuf, f: &FileData, d: &Def| -> Option<u8> {
            let top = d.container.is_none();
            let fq = fq_name(f, d);
            // `import a.b.Foo as Bar` makes `Bar` visible, and not `Foo`.
            let imported = imports.iter().any(|i| {
                (!i.star && i.path == fq && i.alias.as_deref().is_none_or(|a| a == name))
                    || (i.star && fq.rsplit_once('.').map(|(q, _)| q) == Some(i.path.as_str()))
            });
            let is_type = d.kind.is_type();
            let same_package = top && &f.summary.package == pkg;
            let package_first = fd.lang.package_before_imports();
            let own_part = d.container.is_some() && d.container == enclosing_name && &f.summary.package == pkg;
            let inherited = d.container.as_ref().and_then(|c| enclosing_supers.get(c));
            if top && p.as_path() == from {
                Some(0)
            } else if imported && !(package_first && same_package) {
                Some(if package_first { 20 } else { 10 })
            } else if same_package {
                Some(if package_first { 10 } else { 20 })
            } else if allow_members && own_part {
                Some(30)
            } else if allow_members && let Some(depth) = inherited {
                Some(30 + (*depth).min(9))
            } else if allow_members && !top && !is_type && !types_only && fd.lang.hidden_receivers() {
                // Receiver of a scope function (apply/with/run) we can't see.
                Some(40)
            } else {
                None
            }
        };
        let Some(best) = defs.iter().filter_map(|(p, f, d)| tier(p, f, d)).min() else {
            return Vec::new();
        };
        defs.iter()
            .filter(|(p, f, d)| tier(p, f, d) == Some(best))
            .map(|(p, _, d)| self.resolved(p, d))
            .collect()
    }

    /// Every supertype of `class`, with how many steps up it is (1 for a
    /// direct supertype).
    fn supertype_closure(&self, fd: &FileData, class: Node) -> HashMap<String, u8> {
        let mut out = HashMap::new();
        let mut queue: std::collections::VecDeque<(String, u8)> =
            fd.lang.class_supertypes(class, &fd.text).into_iter().map(|s| (s, 1)).collect();
        // Other parts of a partial class (C#) may name the supertypes.
        if let Some(name) = fd.lang.class_name(class, &fd.text) {
            for (_, f, d) in self.defs_named(fd.lang, &name) {
                if d.kind.is_class_like() && f.summary.package == fd.summary.package {
                    queue.extend(d.supertypes.iter().map(|s| (s.clone(), 1)));
                }
            }
        }
        while let Some((c, depth)) = queue.pop_front() {
            // `Base<T>` is `Base`.
            let c = c.split('<').next().unwrap_or(&c).to_string();
            if out.len() > 32 || out.contains_key(&c) {
                continue;
            }
            out.insert(c.clone(), depth);
            for (_, _, d) in self.defs_named(fd.lang, &c) {
                if d.kind.is_class_like() {
                    queue.extend(d.supertypes.iter().map(|s| (s.clone(), depth.saturating_add(1))));
                }
            }
        }
        out
    }
}

/// The name an import binds: `Bar` for `import a.Foo as Bar`, else the last
/// part of the path.
fn error_count(tree: &Tree) -> usize {
    let mut c = tree.walk();
    let mut n = 0;
    'walk: loop {
        let node = c.node();
        if node.is_error() || node.is_missing() {
            n += 1;
        } else if node.has_error() && c.goto_first_child() {
            continue;
        }
        while !c.goto_next_sibling() {
            if !c.goto_parent() {
                break 'walk;
            }
        }
    }
    n
}

/// A function without a container in a [`Visibility::Packages`] language:
/// called as a member of its first parameter.
fn is_extension(d: &Def) -> bool {
    d.container.is_none() && d.kind == DefKind::Function
}

fn binding(i: &Import) -> &str {
    i.alias.as_deref().unwrap_or_else(|| i.path.rsplit('.').next().unwrap_or(&i.path))
}

fn fq_name(f: &FileData, d: &Def) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !f.summary.package.is_empty() {
        parts.push(&f.summary.package);
    }
    if let Some(c) = &d.container {
        parts.push(c);
    }
    parts.push(&d.name);
    parts.join(".")
}

/// The type most of `hits` agree on, for using a lookup's result as a
/// receiver. An untyped receiver (`item.manga` with `item` an untyped lambda
/// param) matches every `manga` member in the project, and voting picks the
/// common case instead of whichever candidate happens to come first. Hits in
/// `from` win outright, as they do in `lookup_members`; ties go to the
/// earlier hit.
fn likely_type(hits: &[Resolved], from: &Path) -> Option<Ty> {
    let local: Vec<&Resolved> = hits.iter().filter(|r| r.loc.path == from).collect();
    let pool = if local.is_empty() { hits.iter().collect() } else { local };
    let mut votes: Vec<(&Ty, usize)> = Vec::new();
    for r in pool {
        match votes.iter_mut().find(|(t, _)| **t == r.ty) {
            Some(v) => v.1 += 1,
            None => votes.push((&r.ty, 1)),
        }
    }
    let best = votes.iter().map(|v| v.1).max()?;
    votes.into_iter().find(|v| v.1 == best).map(|v| v.0.clone())
}

fn fuzzy_match(name: &str, query: &str) -> bool {
    let mut it = name.chars();
    query.chars().all(|q| it.any(|c| c == q))
}

fn target_at(fd: &FileData, offset: usize) -> Option<Target<'_>> {
    let (s, e) = text::word_at(&fd.text, offset)?;
    let node = fd.lang.name_node(fd.tree.root_node(), &fd.text, s, e)?;
    Some(Target { node, start: s, end: e, name: &fd.text[s..e] })
}
