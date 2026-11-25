//! Blocks and the context of a region, for tools that read code by block
//! rather than by file (a coding agent's `path#symbol` reads, or a review
//! UI that shows a definition in place).
//!
//! A file's **outline** is the declarations other files can see (the
//! index's [`Def`]s), each with the extent of its whole declaration, named
//! by its nesting: `Circle`, `Circle.area`, `Outer.Inner.f`. Declarations
//! inside a function body (Python's `self.x = ...`, JavaScript's
//! `this.count = 0`, Go's anonymous struct fields) are members of the index
//! but not blocks.
//!
//! The **context** of a region is what it uses that is declared outside it,
//! and, for the declarations in it, which blocks use them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::index::{FileData, Index, Loc};
use crate::lang::{Def, DefKind, ExtraKind};
use crate::text;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Class,
    Object,
    /// A function declared outside any class.
    Function,
    /// A function declared in a class, object or interface.
    Method,
    Property,
    EnumEntry,
    TypeAlias,
    /// A test case written as a call (`it("adds", ...)`), named with the
    /// groups around it: `math > adds`.
    Test,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Class => "class",
            BlockKind::Object => "object",
            BlockKind::Function => "function",
            BlockKind::Method => "method",
            BlockKind::Property => "property",
            BlockKind::EnumEntry => "enumEntry",
            BlockKind::TypeAlias => "typeAlias",
            BlockKind::Test => "test",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Qualified by the blocks it is nested in: `Circle.area`.
    pub name: String,
    pub kind: BlockKind,
    /// Byte range of the whole declaration, with its annotations,
    /// decorators and modifiers, and an `export` in front of it.
    pub start: usize,
    pub end: usize,
    /// Byte range of the declared name.
    pub name_start: usize,
    pub name_end: usize,
}

/// Nodes that wrap one declaration without being a scope of their own:
/// a declaration that is the only one in such a node extends to it
/// (`export function f`, `const x = 1;`, a decorated Python function).
const WRAPPERS: &[&str] = &[
    "ambient_declaration",
    "assignment",
    "const_declaration",
    "constant_declaration",
    "decorated_definition",
    "event_field_declaration",
    "export_statement",
    "expression_statement",
    "field_declaration",
    "lexical_declaration",
    "local_declaration_statement",
    "property_declaration",
    "type_declaration",
    "var_declaration",
    "variable_declaration",
    "variable_declarator",
];

/// The blocks of a file, in source order (a block before the blocks nested
/// in it).
pub fn outline(fd: &FileData) -> Vec<Block> {
    let root = fd.tree.root_node();
    let defs: Vec<&Def> = fd.summary.defs.iter().filter(|d| !d.name.contains('@')).collect();
    let names: Vec<(&Def, Node)> =
        defs.iter().filter_map(|d| Some((*d, root.descendant_for_byte_range(d.start, d.end)?))).collect();
    let mut raw: Vec<Item> = Vec::new();
    for &(d, name) in &names {
        let ext = extent(name, d, &names);
        raw.push(Item {
            name: d.name.clone(),
            kind: Some(d.kind),
            container: d.container.as_deref(),
            start: ext.start_byte(),
            end: ext.end_byte(),
            name_start: d.start,
            name_end: d.end,
        });
    }
    for x in fd.lang.extra_blocks(root, &fd.text) {
        raw.push(Item {
            kind: (x.kind == ExtraKind::Function).then_some(DefKind::Function),
            name: x.name,
            container: None,
            start: x.start,
            end: x.end,
            name_start: x.name_start,
            name_end: x.name_end,
        });
    }
    // Outer blocks first; for equal starts, the longer one.
    raw.sort_by_key(|i| (i.start, std::cmp::Reverse(i.end), i.name_start));
    raw.dedup_by(|a, b| a.name_start == b.name_start);

    let mut out: Vec<Block> = Vec::new();
    // Indexes into `out` of the blocks enclosing the current one.
    let mut stack: Vec<usize> = Vec::new();
    for i in raw {
        while let Some(&top) = stack.last() {
            if out[top].start <= i.start && i.end <= out[top].end {
                break;
            }
            stack.pop();
        }
        let parent = stack.last().map(|&i| &out[i]);
        if parent.is_some_and(|p| matches!(p.kind, BlockKind::Function | BlockKind::Method | BlockKind::Test)) {
            continue;
        }
        let kind = match i.kind {
            None => BlockKind::Test,
            Some(DefKind::Class) => BlockKind::Class,
            Some(DefKind::Object) => BlockKind::Object,
            // Go declares methods outside their type.
            Some(DefKind::Function)
                if parent.map_or(i.container.is_some(), |p| matches!(p.kind, BlockKind::Class | BlockKind::Object)) =>
            {
                BlockKind::Method
            }
            Some(DefKind::Function) => BlockKind::Function,
            Some(DefKind::Property) => BlockKind::Property,
            Some(DefKind::EnumEntry) => BlockKind::EnumEntry,
            Some(DefKind::TypeAlias) => BlockKind::TypeAlias,
        };
        // A member of an anonymous object type or literal that is not nested
        // in a block (`module.exports = { run }`) has no name to go by.
        if parent.is_none() && i.container.is_some_and(|c| c.contains('@')) {
            continue;
        }
        let name = match (parent, i.container) {
            (Some(p), _) => format!("{}.{}", p.name, i.name),
            (None, Some(c)) => format!("{c}.{}", i.name),
            (None, None) => i.name,
        };
        out.push(Block { name, kind, start: i.start, end: i.end, name_start: i.name_start, name_end: i.name_end });
        stack.push(out.len() - 1);
    }
    out
}

/// A block before nesting names it.
struct Item<'a> {
    name: String,
    /// `None` for a test.
    kind: Option<DefKind>,
    container: Option<&'a str>,
    start: usize,
    end: usize,
    name_start: usize,
    name_end: usize,
}

/// The node spanning the declaration whose name is `name`: its parent
/// (unless other declarations are named right in it, as in a TypeScript
/// enum's body), and then any [`WRAPPERS`] around that, as long as they hold
/// no other declaration of the same container (a Go `const (...)` group or
/// Java's `int a, b;` stay apart).
fn extent<'t>(name: Node<'t>, d: &Def, names: &[(&Def, Node<'t>)]) -> Node<'t> {
    let Some(p) = name.parent() else { return name };
    let declared_in_p = |n: &Node| n.parent() == Some(p) || n.parent().and_then(|q| q.parent()) == Some(p);
    if p.parent().is_none() || names.iter().any(|(o, n)| {
            o.start != d.start && o.container.as_deref() != Some(d.name.as_str()) && declared_in_p(n)
        }) {
        return name;
    }
    let alone = |n: Node, inner: Node| {
        names.iter().all(|(o, _)| {
            o.start == d.start
                || o.start < n.start_byte()
                || o.start >= n.end_byte()
                || (inner.start_byte() <= o.start && o.start < inner.end_byte())
                || o.container.as_deref() == Some(d.name.as_str())
                || o.container != d.container
        })
    };
    let mut cur = p;
    while let Some(p) = cur.parent() {
        if p.parent().is_none() || !WRAPPERS.contains(&p.kind()) || !alone(p, cur) {
            break;
        }
        cur = p;
    }
    cur
}

/// The block `name` names in `blocks`: the one with that qualified name,
/// else the only one whose name ends with `.name`.
pub fn find<'b>(blocks: &'b [Block], name: &str) -> Result<&'b Block, String> {
    if let Some(b) = blocks.iter().find(|b| b.name == name) {
        return Ok(b);
    }
    let suffix = format!(".{name}");
    let hits: Vec<&Block> = blocks.iter().filter(|b| b.name.ends_with(&suffix)).collect();
    match hits.as_slice() {
        [b] => Ok(b),
        [] => Err(format!("no block named {name:?}")),
        _ => Err(format!(
            "{name:?} is ambiguous: {}",
            hits.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The innermost block of `blocks` containing `offset`.
pub fn enclosing(blocks: &[Block], offset: usize) -> Option<&Block> {
    blocks.iter().rev().find(|b| b.start <= offset && offset < b.end)
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// A declaration outside a region that the region uses.
#[derive(Clone, Debug)]
pub struct Use {
    pub name: String,
    /// The declaration's name.
    pub target: Loc,
    /// The block it declares, or else the block it is in (a local declared
    /// before the region, in the function around it). `None` at a file's top
    /// level outside any block.
    pub block: Option<Block>,
    /// Where the region uses it.
    pub sites: Vec<Loc>,
    /// The name resolved to more than one declaration (an untyped receiver,
    /// say), and this is one of them.
    pub ambiguous: bool,
}

/// A block outside a region that uses declarations in it.
#[derive(Clone, Debug)]
pub struct Caller {
    pub path: PathBuf,
    /// `None` at a file's top level outside any block.
    pub block: Option<Block>,
    /// The region's declarations it uses.
    pub names: Vec<String>,
    pub sites: Vec<Loc>,
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    /// Declarations outside the region that it uses, in order of first use.
    pub uses: Vec<Use>,
    /// Blocks (or file top levels) outside the region that use the blocks
    /// declared in it, sorted by path and offset.
    pub used_by: Vec<Caller>,
    /// Names used in the region that resolve to nothing in the project
    /// (library, builtin or misspelled), in order of first use.
    pub unresolved: Vec<String>,
}

/// What the region `start..end` of `path` uses and, when `callers` is set,
/// what uses the blocks declared in it.
pub fn context(ix: &mut Index, path: &Path, start: usize, end: usize, callers: bool) -> Context {
    ix.refresh();
    let Some(fd) = ix.files.get(path) else { return Context::default() };
    let idents = identifiers_in(fd, start, end);
    let here = outline(fd);
    let mut outlines: HashMap<PathBuf, Vec<Block>> = HashMap::new();
    outlines.insert(path.to_path_buf(), here.clone());

    let inside = |l: &Loc| l.path == path && start <= l.start && l.start < end;
    let mut ctx = Context::default();
    let mut by_target: HashMap<Loc, usize> = HashMap::new();
    for (off, name) in idents {
        let defs = ix.definition_at(path, off);
        if defs.is_empty() {
            if !ctx.unresolved.contains(&name) {
                ctx.unresolved.push(name);
            }
            continue;
        }
        let ambiguous = defs.len() > 1;
        let site = Loc { path: path.to_path_buf(), start: off, end: off + name.len() };
        for target in defs.into_iter().filter(|l| !inside(l)) {
            if let Some(&i) = by_target.get(&target) {
                ctx.uses[i].sites.push(site.clone());
                continue;
            }
            let blocks = blocks_of(ix, &mut outlines, &target.path);
            let block = blocks
                .iter()
                .find(|b| b.name_start == target.start)
                .or_else(|| enclosing(blocks, target.start))
                .cloned();
            let name = ix.files.get(&target.path).map_or(String::new(), |f| f.text[target.start..target.end].to_string());
            by_target.insert(target.clone(), ctx.uses.len());
            ctx.uses.push(Use { name, target, block, sites: vec![site.clone()], ambiguous });
        }
    }

    if callers {
        let declared: Vec<Block> = here.iter().filter(|b| start <= b.name_start && b.name_start < end).cloned().collect();
        // Keyed by file and block extent (0..0 for a file's top level).
        // The names used, the block, the uses.
        type Group = (Vec<String>, Option<Block>, Vec<Loc>);
        let mut groups: BTreeMap<(PathBuf, usize, usize), Group> = BTreeMap::new();
        for d in &declared {
            for r in ix.references_at(path, d.name_start, false, false) {
                if inside(&r) {
                    continue;
                }
                let block = enclosing(blocks_of(ix, &mut outlines, &r.path), r.start).cloned();
                let key = (r.path.clone(), block.as_ref().map_or(0, |b| b.start), block.as_ref().map_or(0, |b| b.end));
                let g = groups.entry(key).or_insert_with(|| (Vec::new(), block, Vec::new()));
                let used = &ix.files[path].text[d.name_start..d.name_end];
                if !g.0.iter().any(|n| n == used) {
                    g.0.push(used.to_string());
                }
                g.2.push(r);
            }
        }
        ctx.used_by = groups
            .into_iter()
            .map(|((path, _, _), (names, block, mut sites))| {
                sites.sort();
                sites.dedup();
                Caller { path, block, names, sites }
            })
            .collect();
    }
    ctx
}

fn blocks_of<'o>(ix: &Index, outlines: &'o mut HashMap<PathBuf, Vec<Block>>, path: &Path) -> &'o [Block] {
    outlines
        .entry(path.to_path_buf())
        .or_insert_with(|| ix.files.get(path).map(outline).unwrap_or_default())
}

/// Identifiers starting in `start..end` that are uses, not declarations,
/// with their text, in source order.
fn identifiers_in(fd: &FileData, start: usize, end: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut c = fd.tree.walk();
    'walk: loop {
        let n = c.node();
        let overlaps = n.start_byte() < end && start < n.end_byte();
        if overlaps
            && n.start_byte() >= start
            && fd.lang.is_identifier(n)
            && !fd.lang.is_declaration_name(n)
            && let Some((s, e)) = text::word_at(&fd.text, n.start_byte()).filter(|&(s, _)| s == n.start_byte())
        {
            out.push((s, fd.text[s..e].to_string()));
        }
        if overlaps && c.goto_first_child() {
            continue;
        }
        while !c.goto_next_sibling() {
            if !c.goto_parent() {
                break 'walk;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Regions
// ---------------------------------------------------------------------------

/// Part of a file, as a reader names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Region {
    /// A block by its qualified name (or a unique last part of it).
    Block(String),
    /// 1-based lines, inclusive.
    Lines(usize, usize),
    Whole,
}

impl Region {
    /// Splits `path#Block`, `path:12` or `path:12-30` (anything else is the
    /// whole file) into the path and the region.
    pub fn parse(spec: &str) -> (&str, Region) {
        if let Some((path, block)) = spec.split_once('#').filter(|(p, b)| !p.is_empty() && !b.is_empty()) {
            return (path, Region::Block(block.to_string()));
        }
        if let Some((path, lines)) = spec.rsplit_once(':') {
            let (a, b) = lines.split_once('-').unwrap_or((lines, lines));
            if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>())
                && a > 0
                && a <= b
            {
                return (path, Region::Lines(a, b));
            }
        }
        (spec, Region::Whole)
    }

    /// The byte range of the region in `fd`, and the block it names.
    pub fn range(&self, fd: &FileData) -> Result<(usize, usize, Option<Block>), String> {
        match self {
            Region::Whole => Ok((0, fd.text.len(), None)),
            Region::Block(name) => {
                let blocks = outline(fd);
                let b = find(&blocks, name)?;
                Ok((b.start, b.end, Some(b.clone())))
            }
            Region::Lines(a, b) => {
                let line_start = |l: usize| {
                    fd.lines.offset(&fd.text, lsp_types::Position::new(l as u32 - 1, 0), text::Encoding::Utf8)
                };
                let start = line_start(*a);
                let end = line_start(*b + 1);
                if start >= fd.text.len() && !fd.text.is_empty() {
                    return Err(format!("line {a} is past the end of the file"));
                }
                Ok((start, end, None))
            }
        }
    }
}
