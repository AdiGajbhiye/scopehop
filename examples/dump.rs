fn main() {
    let path = std::env::args().nth(1).unwrap();
    let src = std::fs::read_to_string(&path).unwrap();
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_kotlin_ng::LANGUAGE.into()).unwrap();
    let tree = p.parse(&src, None).unwrap();
    let mut c = tree.walk();
    let mut depth = 0;
    loop {
        let n = c.node();
        if n.is_named() {
            let text = if n.child_count() == 0 { format!(" {:?}", &src[n.byte_range()]) } else { String::new() };
            let field = c.field_name().map(|f| format!("{f}: ")).unwrap_or_default();
            println!("{}{}{}{} [{}]", "  ".repeat(depth), field, n.kind(), text, n.start_position().row + 1);
        }
        if c.goto_first_child() { depth += 1; continue; }
        while !c.goto_next_sibling() {
            if !c.goto_parent() { return; }
            depth -= 1;
        }
    }
}
