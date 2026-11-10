//! A plain-text dump of what the index resolves, for snapshot tests and for
//! diffing two builds on a real project (`examples/snapshot.rs`).
//!
//! ```text
//! app/Main.kt:12:9 radius -> model/Shapes.kt:5:22
//! model/Shapes.kt:5:22 radius <- app/Main.kt:12:9 model/Extensions.kt:3:30
//! ```
//!
//! `->` lines give the definitions of every identifier. `<-` lines give the
//! references of every identifier that resolves to itself (a declaration).
//! Positions are 1-based lines and byte columns, relative to `root`.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::index::{Index, Loc};
use crate::text::Encoding;

pub fn snapshot(ix: &mut Index, root: &Path, references: bool) -> String {
    ix.refresh();
    let mut paths: Vec<PathBuf> = ix.files.keys().cloned().collect();
    paths.sort();
    let mut out = String::new();
    for path in &paths {
        for off in identifiers(ix, path) {
            let here = Loc { path: path.clone(), start: off, end: off };
            let name = word(ix, &here);
            let defs = ix.definition_at(path, off);
            let _ = writeln!(out, "{} {name} -> {}", show(ix, root, &here), show_all(ix, root, &defs));
            let is_decl = defs.len() == 1 && defs[0].path == *path && defs[0].start == off;
            if references && is_decl {
                let refs = ix.references_at(path, off, false, false);
                let _ = writeln!(out, "{} {name} <- {}", show(ix, root, &here), show_all(ix, root, &refs));
            }
        }
    }
    out
}

/// Start offsets of every identifier in the file.
fn identifiers(ix: &Index, path: &Path) -> Vec<usize> {
    let fd = &ix.files[path];
    let mut out = Vec::new();
    let mut c = fd.tree.walk();
    'walk: loop {
        let n = c.node();
        if fd.lang.is_identifier(n) {
            out.push(n.start_byte());
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
    out
}

fn word(ix: &Index, loc: &Loc) -> String {
    let text = &ix.files[&loc.path].text;
    crate::text::word_at(text, loc.start).map_or_else(String::new, |(s, e)| text[s..e].to_string())
}

fn show(ix: &Index, root: &Path, loc: &Loc) -> String {
    let rel = loc.path.strip_prefix(root).unwrap_or(&loc.path);
    match ix.files.get(&loc.path) {
        Some(fd) => {
            let p = fd.lines.position(&fd.text, loc.start, Encoding::Utf8);
            format!("{}:{}:{}", rel.display(), p.line + 1, p.character + 1)
        }
        None => format!("{}@{}", rel.display(), loc.start),
    }
}

fn show_all(ix: &Index, root: &Path, locs: &[Loc]) -> String {
    let all: Vec<String> = locs.iter().map(|l| show(ix, root, l)).collect();
    all.join(" ")
}
