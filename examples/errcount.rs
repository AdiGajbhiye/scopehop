fn main() {
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_kotlin_ng::LANGUAGE.into()).unwrap();
    let (mut files, mut bad) = (0, 0);
    for path in std::env::args().skip(1) {
        let Ok(src) = std::fs::read_to_string(&path) else { continue };
        files += 1;
        let tree = p.parse(&src, None).unwrap();
        if tree.root_node().has_error() {
            bad += 1;
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
    println!("files {files}, with errors {bad}");
}
