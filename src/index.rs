//! Workspace index: every Kotlin file parsed once, plus a name -> declarations
//! map. Definition = scope walk in the current file, then the index ranked by
//! imports/package. References = whole-word hits that resolve to the same place.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use tree_sitter::{Node, Parser, Tree};

use crate::kotlin::{self, Def, DefKind, Summary};
use crate::text::{self, LineIndex};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Loc {
    pub path: PathBuf,
    pub start: usize,
    pub end: usize,
}

pub struct FileData {
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
    matches!(path.extension().and_then(|e| e.to_str()), Some("kt" | "kts"))
}

impl Index {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&kotlin::language())
            .expect("kotlin grammar");
        Self {
            roots,
            files: HashMap::new(),
            names: HashMap::new(),
            names_dirty: true,
            parser,
            last_scan: None,
        }
    }

    fn build(&mut self, text: String, mtime: Option<SystemTime>, open: bool) -> Option<FileData> {
        let tree = self.parser.parse(&text, None)?;
        let summary = kotlin::summarize(tree.root_node(), &text);
        let lines = LineIndex::new(&text);
        Some(FileData { text, tree, summary, lines, mtime, open })
    }

    /// didOpen / didChange: the editor's copy wins over the disk.
    pub fn set_open(&mut self, path: PathBuf, text: String) {
        if let Some(fd) = self.build(text, None, true) {
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
                if let Some(fd) = self.build(text, mtime, false) {
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
        for (path, fd) in &self.files {
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
        let mut locs: Vec<Loc> = self.resolve(path, fd, &t, 0).into_iter().map(|r| r.loc).collect();
        locs.dedup();
        locs
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
        out.sort_by_key(|(_, d)| (d.name.len(), d.name.clone()));
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
        let here = |s, e| Loc { path: path.to_path_buf(), start: s, end: e };
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        if t.node.kind() == "identifier" {
            if kotlin::is_declaration_name(t.node) {
                let ty = self.local_type(path, fd, t.node, depth);
                return vec![Resolved { loc: here(t.start, t.end), ty }];
            }
            if let Some(call) = kotlin::named_argument_call(t.node) {
                return self.named_argument(path, fd, call, t.name, depth);
            }
            if let Some(recv) = kotlin::member_receiver(t.node) {
                let ty = self.type_of(path, fd, recv, depth + 1);
                return self.lookup_members(t.name, &ty, path);
            }
            // Outer.Inner in a type: Inner is a member of Outer.
            if t.node.parent().is_some_and(|p| p.kind() == "user_type") {
                if let Some(prev) = t.node.prev_named_sibling().filter(|n| n.kind() == "identifier") {
                    let outer = Ty::Class(kotlin::text(prev, src).to_string());
                    return self.lookup_members(t.name, &outer, path);
                }
            }
        }
        if let Some(d) = kotlin::find_in_scopes(t.node, t.name, src) {
            if d.kind() == "lambda_literal" {
                let loc = here(d.start_byte(), d.start_byte() + 1);
                return vec![Resolved { loc, ty: Ty::Unknown }];
            }
            let ty = self.local_type(path, fd, d, depth);
            return vec![Resolved { loc: here(d.start_byte(), d.end_byte()), ty }];
        }
        // Inside `fun Foo.ext()`, bare names may be members of Foo.
        let mut allow_members = true;
        if let Some(recv) = kotlin::extension_receiver_type(t.node, src) {
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
        let types_only = t.node.parent().is_some_and(|p| p.kind() == "user_type");
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
        if let Some(t) = kotlin::decl_type_name(name, &fd.text) {
            return Ty::Class(t);
        }
        match kotlin::decl_initializer(name) {
            Some(init) => self.type_of(path, fd, init, depth + 1),
            None => Ty::Unknown,
        }
    }

    /// `label` in `callee(label = x)`: the callee's parameter.
    fn named_argument(&self, path: &Path, fd: &FileData, call: Node, label: &str, depth: u32) -> Vec<Resolved> {
        let Some(callee) = call.named_child(0) else { return Vec::new() };
        let callee = match callee.kind() {
            "navigation_expression" => {
                match callee.named_child((callee.named_child_count().saturating_sub(1)) as _) {
                    Some(n) => n,
                    None => return Vec::new(),
                }
            }
            _ => callee,
        };
        if callee.kind() != "identifier" {
            return Vec::new();
        }
        let t = self.target_for(fd, callee);
        let mut out = Vec::new();
        for r in self.resolve(path, fd, &t, depth + 1) {
            let Some(f) = self.files.get(&r.loc.path) else { continue };
            let Some(name) = f.tree.root_node().descendant_for_byte_range(r.loc.start, r.loc.end) else { continue };
            for p in kotlin::params_of(name) {
                if kotlin::text(p, &f.text) == label {
                    out.push(Resolved {
                        loc: Loc { path: r.loc.path.clone(), start: p.start_byte(), end: p.end_byte() },
                        ty: Ty::from(kotlin::decl_type_name(p, &f.text)),
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
        if kotlin::is_literal(expr.kind()) {
            return Ty::External;
        }
        match expr.kind() {
            "this_expression" => Ty::from(
                kotlin::extension_receiver_type(expr, src)
                    .or_else(|| kotlin::enclosing_class(expr).and_then(|c| kotlin::class_name(c, src))),
            ),
            "super_expression" => Ty::from(
                kotlin::enclosing_class(expr)
                    .and_then(|c| kotlin::class_supertypes(c, src).into_iter().next()),
            ),
            "identifier" => {
                let t = self.target_for(fd, expr);
                let hits = self.resolve(path, fd, &t, depth + 1);
                if let Some(r) = hits.first() {
                    return r.ty.clone();
                }
                // Not in the project: a library class (Modifier) or value.
                if t.name.starts_with(|c: char| c.is_uppercase()) {
                    Ty::Class(t.name.to_string())
                } else {
                    Ty::External
                }
            }
            // Foo(...) -> Foo, makeFoo() -> declared return type
            "call_expression" => match expr.named_child(0) {
                Some(callee) => self.type_of(path, fd, callee, depth + 1),
                None => Ty::Unknown,
            },
            "navigation_expression" => {
                let Some(last) = expr.named_child((expr.named_child_count().saturating_sub(1)) as _) else {
                    return Ty::Unknown;
                };
                if last.kind() != "identifier" {
                    return Ty::Unknown;
                }
                let t = self.target_for(fd, last);
                match self.resolve(path, fd, &t, depth + 1).first() {
                    Some(r) => r.ty.clone(),
                    None => Ty::External,
                }
            }
            // (x), x!!, and similar wrappers
            _ if expr.named_child_count() == 1 => match expr.named_child(0) {
                Some(inner) => self.type_of(path, fd, inner, depth + 1),
                None => Ty::Unknown,
            },
            _ => Ty::Unknown,
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
                    if matches!(d.kind, DefKind::Class | DefKind::Object) {
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
            defs.retain(|(_, _, d)| matches!(d.kind, DefKind::Class | DefKind::Object | DefKind::TypeAlias));
        }
        if defs.is_empty() {
            return Vec::new();
        }
        let pkg = &fd.summary.package;
        let enclosing_supers: HashSet<String> = kotlin::enclosing_class(at)
            .map(|c| self.supertype_closure(&c, &fd.text))
            .unwrap_or_default();

        // Lower is better; None means Kotlin would not see it from here.
        let tier = |p: &PathBuf, f: &FileData, d: &Def| -> Option<u8> {
            let top = d.container.is_none();
            let fq = fq_name(f, d);
            let imported = fd.summary.imports.iter().any(|i| {
                (!i.star && i.alias.is_none() && i.path == fq)
                    || (i.star && fq.rsplit_once('.').map(|(q, _)| q) == Some(i.path.as_str()))
            });
            let is_type = matches!(d.kind, DefKind::Class | DefKind::Object | DefKind::TypeAlias);
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

    fn supertype_closure(&self, class: &Node, src: &str) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut queue = kotlin::class_supertypes(*class, src);
        while let Some(c) = queue.pop() {
            if out.len() > 32 || !out.insert(c.clone()) {
                continue;
            }
            for (_, _, d) in self.defs_named(&c) {
                if matches!(d.kind, DefKind::Class | DefKind::Object) {
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

fn fuzzy_match(name: &str, query: &str) -> bool {
    let mut it = name.chars();
    query.chars().all(|q| it.any(|c| c == q))
}

fn target_at(fd: &FileData, offset: usize) -> Option<Target<'_>> {
    let (s, e) = text::word_at(&fd.text, offset)?;
    let node = fd.tree.root_node().descendant_for_byte_range(s, e)?;
    let name = &fd.text[s..e];
    if node.kind() == "identifier" && node.start_byte() == s && node.end_byte() == e {
        return Some(Target { node, start: s, end: e, name });
    }
    // "$name" string templates are plain string content in this grammar.
    if node.kind() == "string_content" && s > 0 && fd.text.as_bytes()[s - 1] == b'$' {
        return Some(Target { node, start: s, end: e, name });
    }
    None
}
