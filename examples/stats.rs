//! Resolve every identifier in a project; report counts, timing and samples.
use std::collections::HashMap;
use std::path::PathBuf;
use treenav::{index::Index, text::Encoding};

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap());
    let mut ix = Index::new(vec![root]);
    let t = std::time::Instant::now();
    ix.refresh();
    println!("indexed {} files in {:?}", ix.files.len(), t.elapsed());
    let mut paths: Vec<_> = ix.files.keys().cloned().collect();
    paths.sort();
    let mut buckets: HashMap<&str, usize> = HashMap::new();
    let mut zero: HashMap<String, usize> = HashMap::new();
    let mut multi: Vec<(String, usize, String)> = Vec::new();
    let t = std::time::Instant::now();
    let mut total = 0;
    for p in &paths {
        let (text, idents) = {
            let fd = &ix.files[p];
            let mut v = Vec::new();
            let mut c = fd.tree.walk();
            'w: loop {
                let n = c.node();
                if n.kind() == "identifier" { v.push(n.start_byte()); }
                if c.goto_first_child() { continue; }
                while !c.goto_next_sibling() { if !c.goto_parent() { break 'w; } }
            }
            (fd.text.clone(), v)
        };
        for off in idents {
            // skip package/import paths
            let line_start = text[..off].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let line = &text[line_start..text[off..].find('\n').map(|i| off + i).unwrap_or(text.len())];
            if line.starts_with("import ") || line.starts_with("package ") { continue; }
            total += 1;
            let locs = ix.definition_at(p, off);
            let name: String = text[off..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            match locs.len() {
                0 => { *buckets.entry("0").or_default() += 1; *zero.entry(name).or_default() += 1; }
                1 => *buckets.entry("1").or_default() += 1,
                n => { *buckets.entry(">1").or_default() += 1; if multi.len() < 4000 { multi.push((name, n, format!("{}:{}", p.file_name().unwrap().to_string_lossy(), ix.files[p].lines.position(&text, off, Encoding::Utf8).line + 1))); } }
            }
        }
    }
    println!("{total} identifiers in {:?}: {:?}", t.elapsed(), buckets);
    let mut z: Vec<_> = zero.into_iter().collect();
    z.sort_by(|a, b| b.1.cmp(&a.1));
    println!("top unresolved (mostly library names): {:?}", &z[..z.len().min(40)]);
    let mut m: HashMap<String, (usize, usize, String)> = HashMap::new();
    for (name, n, at) in multi { let e = m.entry(name).or_insert((0, n, at)); e.0 += 1; }
    let mut m: Vec<_> = m.into_iter().collect();
    m.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    println!("top ambiguous (name, (uses, candidates, example)):");
    for x in m.iter().take(40) { println!("  {:?}", x); }
}
