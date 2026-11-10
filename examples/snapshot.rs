//! snapshot <root> [--no-refs]
//!
//! Prints what scopehop resolves for every identifier under <root> (see
//! `scopehop::snapshot`). Run it before and after a change and diff the two.
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::fs::canonicalize(PathBuf::from(args.next().expect("usage: snapshot <root> [--no-refs]"))).unwrap();
    let refs = !args.any(|a| a == "--no-refs");
    let mut ix = scopehop::index::Index::new(vec![root.clone()]);
    print!("{}", scopehop::snapshot::snapshot(&mut ix, &root, refs));
}
