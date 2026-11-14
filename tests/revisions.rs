//! Resolving code as it was in a git revision: the library API, the
//! `def`/`refs` command line and the `scopehop/definition` LSP request.
//!
//! Each test builds a small repository with two commits and an uncommitted
//! change:
//!
//! - commit 1: Go module `example.com/old` with `models.Gone()`, called
//!   from `app/main.go` and `app/old.go`; a TypeScript app importing
//!   `@m/shape` through a `tsconfig.json` path alias.
//! - commit 2: the module is `example.com/new`, `Gone` and `app/old.go`
//!   are gone, and the alias is replaced by a relative import.
//! - working tree: `Area` renamed to `Size`, not committed.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use scopehop::files::Revision;
use scopehop::index::Index;
use scopehop::text::Encoding;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const MAIN_V1: &str = "package main\n\nimport \"example.com/old/models\"\n\nfunc main() {\n\tprintln(models.Area() + models.Gone())\n}\n";
const OLD_GO: &str = "package main\n\nimport \"example.com/old/models\"\n\nfunc legacy() int { return models.Gone() }\n";
const MODELS_V1: &str = "package models\n\nfunc Area() int { return 1 }\n\nfunc Gone() int { return 2 }\n";
const APP_TS_V1: &str = "import { area } from \"@m/shape\";\n\nexport const total = area() + 1;\n";

/// A repository in the temp directory, deleted when dropped.
struct Repo(PathBuf);

impl std::ops::Deref for Repo {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Repo {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh repository (see the module docs), canonicalized as git reports it.
fn repo() -> Repo {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("scopehop-rev-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);

    write(&dir, "go.mod", "module example.com/old\n\ngo 1.22\n");
    write(&dir, "models/models.go", MODELS_V1);
    write(&dir, "app/main.go", MAIN_V1);
    write(&dir, "app/old.go", OLD_GO);
    write(&dir, "web/tsconfig.json", "{ \"compilerOptions\": { \"paths\": { \"@m/*\": [\"src/models/*\"] } } }\n");
    write(&dir, "web/src/models/shape.ts", "export function area(): number {\n  return 1;\n}\n");
    write(&dir, "web/src/app.ts", APP_TS_V1);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "one"]);

    write(&dir, "go.mod", "module example.com/new\n\ngo 1.22\n");
    write(&dir, "models/models.go", "package models\n\nfunc Area() int { return 1 }\n");
    write(&dir, "app/main.go", "package main\n\nimport \"example.com/new/models\"\n\nfunc main() {\n\tprintln(models.Area())\n}\n");
    std::fs::remove_file(dir.join("app/old.go")).unwrap();
    write(&dir, "web/tsconfig.json", "{}\n");
    write(&dir, "web/src/app.ts", "import { area } from \"./models/shape\";\n\nexport const total = area() + 1;\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "two"]);

    // Not committed.
    write(&dir, "models/models.go", "package models\n\nfunc Size() int { return 1 }\n");
    write(&dir, "app/main.go", "package main\n\nimport \"example.com/new/models\"\n\nfunc main() {\n\tprintln(models.Size())\n}\n");
    Repo(dir)
}

/// 1-based line and byte column of the `|` in `snippet`, which occurs once
/// in `text`.
fn pos(text: &str, snippet: &str) -> (u32, u32) {
    let caret = snippet.find('|').expect("no |");
    let needle = snippet.replacen('|', "", 1);
    let hits: Vec<_> = text.match_indices(&needle).collect();
    assert_eq!(hits.len(), 1, "{needle:?} occurs {} times", hits.len());
    let off = hits[0].0 + caret;
    let line = text[..off].matches('\n').count() as u32 + 1;
    let col = (off - text[..off].rfind('\n').map_or(0, |i| i + 1)) as u32 + 1;
    (line, col)
}

/// `file:line:col` (relative to `root`) of each definition of the `|` in
/// `snippet`, in `file` as it is in `rev`.
fn def_at(root: &Path, rev: &str, file: &str, text: &str, snippet: &str) -> Vec<String> {
    let mut ix = Index::with_files(vec![root.to_path_buf()], Box::new(Revision::open(root, rev).unwrap()));
    ix.refresh();
    let path = root.join(file);
    let (line, col) = pos(text, snippet);
    let fd = ix.files.get(&path).unwrap_or_else(|| panic!("{file} not in {rev}"));
    let off = fd.lines.offset(&fd.text, lsp_types::Position::new(line - 1, col - 1), Encoding::Utf8);
    ix.definition_at(&path, off)
        .iter()
        .map(|l| {
            let fd = &ix.files[&l.path];
            let p = fd.lines.position(&fd.text, l.start, Encoding::Utf8);
            format!("{}:{}:{}", l.path.strip_prefix(root).unwrap().display(), p.line + 1, p.character + 1)
        })
        .collect()
}

#[test]
fn removed_code_resolves_in_the_old_revision() {
    let root = repo();
    // `example.com/old/models` is only a package under commit 1's go.mod.
    assert_eq!(def_at(&root, "HEAD~1", "app/main.go", MAIN_V1, "models.|Gone()"), vec!["models/models.go:5:6"]);
    assert_eq!(def_at(&root, "HEAD~1", "app/main.go", MAIN_V1, "import \"example.com/old/|models\""), vec!["models/models.go:1:1"]);
}

#[test]
fn deleted_files_resolve_in_the_old_revision() {
    let root = repo();
    assert!(!root.join("app/old.go").exists());
    assert_eq!(def_at(&root, "HEAD~1", "app/old.go", OLD_GO, "models.|Gone()"), vec!["models/models.go:5:6"]);
}

#[test]
fn old_config_files_are_read_from_the_revision() {
    let root = repo();
    // The `@m/*` alias exists only in commit 1's tsconfig.json.
    assert_eq!(def_at(&root, "HEAD~1", "web/src/app.ts", APP_TS_V1, "|area() + 1"), vec!["web/src/models/shape.ts:1:17"]);
}

#[test]
fn a_revision_ignores_uncommitted_changes() {
    let root = repo();
    let committed = "package main\n\nimport \"example.com/new/models\"\n\nfunc main() {\n\tprintln(models.Area())\n}\n";
    assert_eq!(def_at(&root, "HEAD", "app/main.go", committed, "models.|Area()"), vec!["models/models.go:3:6"]);
}

fn cli(root: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_scopehop")).current_dir(root).args(args).output().unwrap();
    assert!(out.status.success(), "scopehop {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn command_line() {
    let root = repo();
    let (line, col) = pos(MAIN_V1, "models.|Gone()");
    let (line, col) = (line.to_string(), col.to_string());
    let out = cli(&root, &["def", "--rev", "HEAD~1", "app/main.go", &line, &col]);
    assert_eq!(out, "models/models.go:5:6: func Gone() int { return 2 }\n");

    let json: serde_json::Value = serde_json::from_str(&cli(&root, &["def", "--json", "--rev", "HEAD~1", "app/main.go", &line, &col])).unwrap();
    assert_eq!(
        json,
        serde_json::json!([{ "path": root.join("models/models.go"), "line": 5, "column": 6, "endLine": 5, "endColumn": 10 }])
    );

    // References across both files that call it in commit 1.
    let out = cli(&root, &["refs", "--no-decl", "--rev", "HEAD~1", "app/main.go", &line, &col]);
    let mut got: Vec<&str> = out.lines().map(|l| l.split(": ").next().unwrap()).collect();
    got.sort();
    assert_eq!(got, ["app/main.go:6:33", "app/old.go:5:35"]);

    // The working tree: `Size`, not committed anywhere.
    let (line, col) = pos("package main\n\nimport \"example.com/new/models\"\n\nfunc main() {\n\tprintln(models.Size())\n}\n", "models.|Size()");
    let out = cli(&root, &["def", "app/main.go", &line.to_string(), &col.to_string()]);
    assert_eq!(out, "models/models.go:3:6: func Size() int { return 1 }\n");
}

/// One LSP message in, with `Content-Length` framing.
fn send(stdin: &mut impl Write, msg: serde_json::Value) {
    let body = msg.to_string();
    write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    stdin.flush().unwrap();
}

/// The next response with `id`.
fn receive(out: &mut impl BufRead, id: u64) -> serde_json::Value {
    loop {
        let mut len = 0;
        loop {
            let mut header = String::new();
            out.read_line(&mut header).unwrap();
            let header = header.trim();
            if header.is_empty() {
                break;
            }
            if let Some(n) = header.strip_prefix("Content-Length: ") {
                len = n.parse().unwrap();
            }
        }
        let mut body = vec![0; len];
        out.read_exact(&mut body).unwrap();
        let msg: serde_json::Value = serde_json::from_slice(&body).unwrap();
        if msg["id"] == id {
            return msg;
        }
    }
}

#[test]
fn lsp_request() {
    let root = repo();
    let mut child = Command::new(env!("CARGO_BIN_EXE_scopehop"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let uri = |rel: &str| format!("file://{}", root.join(rel).display());

    send(&mut stdin, serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "capabilities": { "general": { "positionEncodings": ["utf-8"] } },
            "workspaceFolders": [{ "uri": uri(""), "name": "repo" }],
        },
    }));
    receive(&mut out, 1);
    send(&mut stdin, serde_json::json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));

    let (line, col) = pos(OLD_GO, "models.|Gone()");
    send(&mut stdin, serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "scopehop/definition",
        "params": {
            "revision": "HEAD~1",
            "textDocument": { "uri": uri("app/old.go") },
            "position": { "line": line - 1, "character": col - 1 },
        },
    }));
    let resp = receive(&mut out, 2);
    assert_eq!(
        resp["result"],
        serde_json::json!([{
            "uri": uri("models/models.go"),
            "range": { "start": { "line": 4, "character": 5 }, "end": { "line": 4, "character": 9 } },
        }])
    );

    send(&mut stdin, serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "scopehop/definition",
        "params": { "revision": "no-such-rev", "textDocument": { "uri": uri("app/old.go") }, "position": { "line": 0, "character": 0 } },
    }));
    assert!(receive(&mut out, 3)["error"]["message"].as_str().unwrap().contains("no-such-rev"));

    send(&mut stdin, serde_json::json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown" }));
    receive(&mut out, 4);
    send(&mut stdin, serde_json::json!({ "jsonrpc": "2.0", "method": "exit" }));
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
