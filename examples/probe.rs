//! probe <root> <file> <line> <col> [def|refs]   (1-based line, 1-based byte col)
use std::path::PathBuf;
use scopehop::{index::Index, text::Encoding};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(&a[1]);
    let file = std::fs::canonicalize(&a[2]).unwrap();
    let (line, col): (u32, u32) = (a[3].parse().unwrap(), a[4].parse().unwrap());
    let mode = a.get(5).map(String::as_str).unwrap_or("def");
    let mut ix = Index::new(vec![root]);
    let t = std::time::Instant::now();
    ix.refresh();
    let fd = &ix.files[&file];
    let off = fd.lines.offset(&fd.text, lsp_types::Position::new(line - 1, col - 1), Encoding::Utf8);
    println!("[{} files, {:?}] {} at {:?}", ix.files.len(), t.elapsed(), mode, &fd.text[off..(off + 20).min(fd.text.len())].lines().next().unwrap_or(""));
    let t = std::time::Instant::now();
    let locs = if mode == "def" { ix.definition_at(&file, off) } else { ix.references_at(&file, off, true, false) };
    for l in &locs {
        let fd = &ix.files[&l.path];
        let p = fd.lines.position(&fd.text, l.start, Encoding::Utf8);
        let line = fd.text.lines().nth(p.line as usize).unwrap_or("").trim();
        println!("  {}:{}:{}  {}", l.path.display(), p.line + 1, p.character + 1, line);
    }
    println!("  ({} results in {:?})", locs.len(), t.elapsed());
}
