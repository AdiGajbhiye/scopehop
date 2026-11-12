fn main() {
    let mut p = tree_sitter::Parser::new();
    let (mut files, mut bad, mut raw_bad) = (0, 0, 0);
    for path in std::env::args().skip(1) {
        let Some(lang) = scopehop::lang::for_path(path.as_ref()) else { continue };
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        p.set_language(&lang.grammar()).unwrap();
        files += 1;
        let mut tree = p.parse(&src, None).unwrap();
        if tree.root_node().has_error() {
            raw_bad += 1;
            if let Some(fixed) = lang.repair(&src) {
                let better = p.parse(&fixed, None).unwrap();
                if !better.root_node().has_error() {
                    tree = better;
                }
            }
        }
        if tree.root_node().has_error() {
            bad += 1;
            println!("{path}: has errors");
            // find first error
            let mut c = tree.walk();
            'o: loop {
                let n = c.node();
                if n.is_error() || n.is_missing() { println!("{path}:{}: {}", n.start_position().row + 1, src.lines().nth(n.start_position().row).unwrap_or("").trim()); break 'o; }
                if n.has_error() && c.goto_first_child() { continue; }
                while !c.goto_next_sibling() { if !c.goto_parent() { break 'o; } }
            }
        }
    }
    println!("files {files}, with errors {raw_bad}, after repair {bad}");
}
