//! Workspace index: every source file parsed once, plus a name -> declarations
//! map. Definition = scope walk in the current file, then the index ranked by
//! imports/package. References = whole-word hits that resolve to the same place.
//!
//! Nothing here looks at node kinds; syntax questions go to the file's
//! [`Language`]. What is still language-shaped is the visibility model in
//! `lookup_global` (package + imports) and treating top-level functions as
//! possible members in `lookup_members` (extension functions).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use tree_sitter::{Node, Parser, Tree};

use crate::lang::{self, Def, DefKind, Expr, Language, Scoped, Summary};
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
    /// A class name (may or may not be declared in the project).
    Class(String),
    /// Comes from a library: no project member can match.
    External,
    Unknown,
}

impl Ty {
    fn from(name: Option<String>) -> Self {
        name.map_or(Ty::Unknown, Ty::Class)
    }
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
const MAX_DEPTH: u32 = 6;

pub struct Index {
    roots: Vec<PathBuf>,
    pub files: HashMap<PathBuf, FileData>,
    names: HashMap<String, Vec<(PathBuf, usize)>>,
    names_dirty: bool,
    parser: Parser,
    last_scan: Option<Instant>,
}

pub fn is_source(path: &Path) -> bool {
    lang::for_path(path).is_some()
}

impl Index {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            files: HashMap::new(),
            names: HashMap::new(),
            names_dirty: true,
            parser: Parser::new(),
            last_scan: None,
        }
    }

    fn build(&mut self, path: &Path, text: String, mtime: Option<SystemTime>, open: bool) -> Option<FileData> {
        let lang = lang::for_path(path)?;
        self.parser.set_language(&lang.grammar()).ok()?;
        let tree = self.parser.parse(&text, None)?;
        let summary = lang.summarize(tree.root_node(), &text);
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
        for root in self.roots.clone() {
            for entry in ignore::WalkBuilder::new(&root).build().flatten() {
                let path = entry.path();
                if !entry.file_type().is_some_and(|t| t.is_file()) || !is_source(path) {
                    continue;
                }
                let path = path.to_path_buf();
                seen.insert(path.clone());
                let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
                match self.files.get(&path) {
                    Some(fd) if fd.open || (fd.mtime.is_some() && fd.mtime == mtime) => continue,
                    _ => {}
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                if let Some(fd) = self.build(&path, text, mtime, false) {
                    self.files.insert(path, fd);
                    self.names_dirty = true;
                }
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
        // Sorted so candidate lists, and every choice made from them (ranking
        // ties, `type_of` taking the first hit, supertype walk order), do not
        // depend on the map's random seed.
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by(|a, b| a.0.cmp(b.0));
        for (path, fd) in files {
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
        self.resolve(path, fd, &t, 0)
            .into_iter()
            .map(|r| r.loc)
            .filter(|l| seen.insert(l.clone()))
            .collect()
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
            for d in &fd.summary.defs {
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

    fn resolve(&self, path: &Path, fd: &FileData, t: &Target, depth: u32) -> Vec<Resolved> {
        let src = fd.text.as_str();
        let lang = fd.lang;
        let here = |s, e| Loc { path: path.to_path_buf(), start: s, end: e };
        if depth > MAX_DEPTH {
            return Vec::new();
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
                return self.lookup_members(t.name, &ty, path);
            }
            // Outer.Inner in a type: Inner is a member of Outer.
            if let Some(outer) = lang.type_qualifier(t.node) {
                let outer = Ty::Class(lang::text(outer, src).to_string());
                return self.lookup_members(t.name, &outer, path);
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
        let types_only = lang.in_type_position(t.node);
        // Inside `fun Foo.ext()`, bare names may be members of Foo. Types are
        // not: the receiver brings members into scope, not nested classes, and
        // this would otherwise look `Foo` itself up as a member of `Foo`.
        let mut allow_members = true;
        if let Some(recv) = lang.extension_receiver_type(t.node, src).filter(|_| !types_only) {
            let hits: Vec<_> = self
                .lookup_members(t.name, &Ty::Class(recv), path)
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
            return Ty::Class(t);
        }
        match fd.lang.decl_initializer(name) {
            Some(init) => self.type_of(path, fd, init, depth + 1),
            None => Ty::Unknown,
        }
    }

    /// `label` in `callee(label = x)`: the callee's parameter.
    fn named_argument(&self, path: &Path, fd: &FileData, call: Node, label: &str, depth: u32) -> Vec<Resolved> {
        let Some(callee) = fd.lang.callee_name(call) else { return Vec::new() };
        let t = self.target_for(fd, callee);
        let mut out = Vec::new();
        for r in self.resolve(path, fd, &t, depth + 1) {
            let Some(f) = self.files.get(&r.loc.path) else { continue };
            let Some(name) = f.tree.root_node().descendant_for_byte_range(r.loc.start, r.loc.end) else { continue };
            for p in f.lang.params_of(name) {
                if lang::text(p, &f.text) == label {
                    out.push(Resolved {
                        loc: Loc { path: r.loc.path.clone(), start: p.start_byte(), end: p.end_byte() },
                        ty: Ty::from(f.lang.decl_type_name(p, &f.text)),
                    });
                }
            }
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
        match lang.classify_expr(expr) {
            Expr::Literal => Ty::External,
            Expr::This => Ty::from(
                lang.extension_receiver_type(expr, src)
                    .or_else(|| lang.enclosing_class(expr).and_then(|c| lang.class_name(c, src))),
            ),
            Expr::Super => Ty::from(
                lang.enclosing_class(expr)
                    .and_then(|c| lang.class_supertypes(c, src).into_iter().next()),
            ),
            Expr::Name(n) => {
                let t = self.target_for(fd, n);
                let hits = self.resolve(path, fd, &t, depth + 1);
                if let Some(ty) = likely_type(&hits, path) {
                    return ty;
                }
                // Not in the project: a library class (Modifier) or value.
                if lang.looks_like_type_name(t.name) {
                    Ty::Class(t.name.to_string())
                } else {
                    Ty::External
                }
            }
            // Foo(...) -> Foo, makeFoo() -> declared return type
            Expr::Call(callee) => self.type_of(path, fd, callee, depth + 1),
            Expr::Member(name) => {
                let t = self.target_for(fd, name);
                likely_type(&self.resolve(path, fd, &t, depth + 1), path).unwrap_or(Ty::External)
            }
            Expr::Wrapper(inner) => self.type_of(path, fd, inner, depth + 1),
            Expr::Unknown => Ty::Unknown,
        }
    }

    fn defs_named<'a>(&'a self, name: &str) -> Vec<(&'a PathBuf, &'a FileData, &'a Def)> {
        let Some(list) = self.names.get(name) else { return Vec::new() };
        list.iter()
            .filter_map(|(p, i)| {
                let fd = self.files.get(p)?;
                Some((p, fd, fd.summary.defs.get(*i)?))
            })
            .collect()
    }

    fn resolved(path: &Path, d: &Def) -> Resolved {
        Resolved {
            loc: Loc { path: path.to_path_buf(), start: d.start, end: d.end },
            ty: Ty::from(d.type_name.clone()),
        }
    }

    /// `name` as a member of the receiver's class (walking supertypes), or
    /// any member when the receiver's type is unknown.
    fn lookup_members(&self, name: &str, recv: &Ty, from: &Path) -> Vec<Resolved> {
        let defs = self.defs_named(name);
        let rank = |mut v: Vec<(&PathBuf, &Def)>| {
            v.sort_by_key(|(p, _)| p.as_path() != from);
            v.into_iter().map(|(p, d)| Self::resolved(p, d)).collect::<Vec<_>>()
        };
        let extensions_only = || {
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
        if let Ty::Class(class) = recv {
            let mut queue = vec![class.to_string()];
            let mut seen = HashSet::new();
            while let Some(cls) = queue.pop() {
                if !seen.insert(cls.clone()) || seen.len() > 32 {
                    continue;
                }
                let hits: Vec<_> = defs
                    .iter()
                    .filter(|(_, _, d)| d.container.as_deref() == Some(cls.as_str()))
                    .map(|(p, _, d)| (*p, *d))
                    .collect();
                if !hits.is_empty() {
                    return rank(hits);
                }
                for (_, _, d) in self.defs_named(&cls) {
                    if d.kind.is_class_like() {
                        queue.extend(d.supertypes.iter().cloned());
                    }
                }
            }
            // Library class or inherited from outside the project: only
            // extension functions can match.
            return extensions_only();
        }
        rank(
            defs.iter()
                .filter(|(_, _, d)| d.container.is_some() || d.kind == DefKind::Function)
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
        let mut defs = self.defs_named(name);
        // import a.b.Foo as Bar
        if let Some(imp) = fd.summary.imports.iter().find(|i| i.alias.as_deref() == Some(name)) {
            if let Some(last) = imp.path.rsplit('.').next() {
                defs = self.defs_named(last);
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
        let enclosing_supers: HashSet<String> = fd
            .lang
            .enclosing_class(at)
            .map(|c| self.supertype_closure(fd, c))
            .unwrap_or_default();

        // Lower is better; None means the language would not see it from here.
        let tier = |p: &PathBuf, f: &FileData, d: &Def| -> Option<u8> {
            let top = d.container.is_none();
            let fq = fq_name(f, d);
            let imported = fd.summary.imports.iter().any(|i| {
                (!i.star && i.alias.is_none() && i.path == fq)
                    || (i.star && fq.rsplit_once('.').map(|(q, _)| q) == Some(i.path.as_str()))
            });
            let is_type = d.kind.is_type();
            if top && p.as_path() == from {
                Some(0)
            } else if imported {
                Some(1)
            } else if top && &f.summary.package == pkg {
                Some(2)
            } else if allow_members && d.container.as_ref().is_some_and(|c| enclosing_supers.contains(c)) {
                Some(3)
            } else if allow_members && !top && !is_type && !types_only {
                // Receiver of a scope function (apply/with/run) we can't see.
                Some(4)
            } else {
                None
            }
        };
        let Some(best) = defs.iter().filter_map(|(p, f, d)| tier(p, f, d)).min() else {
            return Vec::new();
        };
        defs.iter()
            .filter(|(p, f, d)| tier(p, f, d) == Some(best))
            .map(|(p, _, d)| Self::resolved(p, d))
            .collect()
    }

    fn supertype_closure(&self, fd: &FileData, class: Node) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut queue = fd.lang.class_supertypes(class, &fd.text);
        while let Some(c) = queue.pop() {
            if out.len() > 32 || !out.insert(c.clone()) {
                continue;
            }
            for (_, _, d) in self.defs_named(&c) {
                if d.kind.is_class_like() {
                    queue.extend(d.supertypes.iter().cloned());
                }
            }
        }
        out
    }
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
