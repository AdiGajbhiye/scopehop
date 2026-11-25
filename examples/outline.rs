//! Print the outline (blocks with 1-based line ranges) of every file in a
//! project: `cargo run --example outline -- <root>`.
fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: outline <root>"));
    let root = std::fs::canonicalize(root).unwrap();
    let mut ix = scopehop::index::Index::new(vec![root.clone()]);
    ix.refresh();
    let mut paths: Vec<_> = ix.files.keys().cloned().collect();
    paths.sort();
    for p in paths {
        let fd = &ix.files[&p];
        println!("{}", p.strip_prefix(&root).unwrap().display());
        for b in scopehop::outline::outline(fd) {
            let line = |o| fd.lines.position(&fd.text, o, scopehop::text::Encoding::Utf8).line + 1;
            println!("  {:<9} {} {}-{}", b.kind.as_str(), b.name, line(b.start), line(b.end.saturating_sub(1).max(b.start)));
        }
    }
}
