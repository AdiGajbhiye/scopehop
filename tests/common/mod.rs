//! Shared helpers for the resolution tests. Each test file indexes one
//! project under `tests/fixtures/<name>`.
//!
//! A cursor is a snippet with a `|` in it, e.g. `"c.ra|dius"`. The snippet
//! (minus the `|`) must occur exactly once in the file. Results are
//! `file:line:col` strings so failures read like source locations.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use scopehop::index::{Index, Loc};
use scopehop::text::{self, Encoding};

pub fn fixture_root(name: &str) -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

pub struct Fx {
    pub ix: Index,
    pub root: PathBuf,
}

impl Fx {
    pub fn new(fixture: &str) -> Self {
        let root = fixture_root(fixture);
        let mut ix = Index::new(vec![root.clone()]);
        ix.refresh();
        Fx { ix, root }
    }

    pub fn cursor(&self, file: &str, snippet: &str) -> (PathBuf, usize) {
        let path = self.root.join(file);
        let text = &self.ix.files.get(&path).unwrap_or_else(|| panic!("{file} not indexed")).text;
        let caret = snippet.find('|').unwrap_or_else(|| panic!("{snippet:?} has no |"));
        let needle = snippet.replacen('|', "", 1);
        let hits: Vec<usize> = text.match_indices(&needle).map(|(i, _)| i).collect();
        assert_eq!(hits.len(), 1, "{needle:?} occurs {} times in {file}", hits.len());
        (path, hits[0] + caret)
    }

    pub fn show(&self, loc: &Loc) -> String {
        let fd = &self.ix.files[&loc.path];
        let p = fd.lines.position(&fd.text, loc.start, Encoding::Utf8);
        let rel = loc.path.strip_prefix(&self.root).unwrap();
        format!("{}:{}:{}", rel.display(), p.line + 1, p.character + 1)
    }

    /// Location of the word under the cursor (or the cursor itself when it
    /// is not on a word, e.g. the `{` that declares an implicit `it`).
    pub fn at(&self, file: &str, snippet: &str) -> String {
        let (path, off) = self.cursor(file, snippet);
        let start = text::word_at(&self.ix.files[&path].text, off).map_or(off, |(s, _)| s);
        self.show(&Loc { path, start, end: start })
    }

    pub fn def(&mut self, file: &str, snippet: &str) -> Vec<String> {
        let (path, off) = self.cursor(file, snippet);
        let locs = self.ix.definition_at(&path, off);
        locs.iter().map(|l| self.show(l)).collect()
    }

    /// References, without the declaration itself, sorted.
    pub fn refs(&mut self, file: &str, snippet: &str) -> Vec<String> {
        let (path, off) = self.cursor(file, snippet);
        let locs = self.ix.references_at(&path, off, false, false);
        let mut out: Vec<String> = locs.iter().map(|l| self.show(l)).collect();
        out.sort();
        out
    }
}

/// `def(file, cursor) == [at(file, cursor)]`, with readable failures.
#[macro_export]
macro_rules! assert_def {
    ($fx:expr, $file:expr, $use:expr => $dfile:expr, $decl:expr) => {{
        let want = vec![$fx.at($dfile, $decl)];
        assert_eq!($fx.def($file, $use), want, "definition of {:?} in {}", $use, $file);
    }};
    ($fx:expr, $file:expr, $use:expr => none) => {{
        assert_eq!($fx.def($file, $use), Vec::<String>::new(), "definition of {:?} in {}", $use, $file);
    }};
}

pub fn snapshot(fixture: &str) -> String {
    let root = fixture_root(fixture);
    let mut ix = Index::new(vec![root.clone()]);
    scopehop::snapshot::snapshot(&mut ix, &root, true)
}

/// Each Index gets differently seeded HashMaps, so ties between candidates in
/// different files would come out in a different order under some of these.
pub fn assert_seed_independent(fixture: &str) {
    let first = snapshot(fixture);
    for _ in 0..7 {
        assert!(snapshot(fixture) == first, "snapshot changed between two indexes of the same project");
    }
}

/// Every identifier in the fixture, with its definitions and (for
/// declarations) references, against `tests/snapshots/<fixture>.txt`. Run
/// with `UPDATE_SNAPSHOTS=1` to accept changes, then review the diff.
pub fn assert_snapshot(fixture: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/snapshots/{fixture}.txt"));
    let got = snapshot(fixture);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    if got != want {
        let diff: Vec<String> = want
            .lines()
            .filter(|l| !got.lines().any(|g| g == *l))
            .map(|l| format!("- {l}"))
            .chain(got.lines().filter(|l| !want.lines().any(|w| w == *l)).map(|l| format!("+ {l}")))
            .take(40)
            .collect();
        panic!("{fixture} snapshot changed (UPDATE_SNAPSHOTS=1 cargo test to accept):\n{}", diff.join("\n"));
    }
}
