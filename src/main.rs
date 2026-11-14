use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentHighlight, DocumentHighlightParams, GotoDefinitionParams, GotoDefinitionResponse,
    InitializeParams, Location, OneOf, PositionEncodingKind, Range, ReferenceParams,
    ServerCapabilities, SymbolInformation, SymbolKind, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri, WorkspaceSymbolParams,
};
use scopehop::files::{self, Revision};
use scopehop::index::{self, Index, Loc};
use scopehop::lang::DefKind;
use scopehop::text::{self, Encoding};

const USAGE: &str = "\
usage:
  scopehop                      language server over stdio
  scopehop def  [options] FILE LINE COL
  scopehop refs [options] FILE LINE COL

LINE and COL are 1-based; COL counts bytes. Options:
  --rev REV    resolve in a git revision (branch, tag, HEAD~1, hash): FILE
               is the path it had there, and may not exist any more
  --root DIR   project root (default: the git repository, or the current
               directory)
  --json       print a JSON array of {path, line, column, endLine,
               endColumn} instead of path:line:col lines
  --no-decl    refs: leave out the declaration itself";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("scopehop {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(cmd @ ("def" | "refs")) => return cli(cmd, &args[1..]),
        Some(other) => anyhow::bail!("unknown command {other:?}\n{USAGE}"),
        None => {}
    }
    let (conn, io) = Connection::stdio();
    let (id, params) = conn.initialize_start()?;
    let init: InitializeParams = serde_json::from_value(params)?;

    let utf8 = init
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_ref())
        .is_some_and(|v| v.contains(&PositionEncodingKind::UTF8));
    let enc = if utf8 { Encoding::Utf8 } else { Encoding::Utf16 };

    let mut roots: Vec<PathBuf> = init
        .workspace_folders
        .iter()
        .flatten()
        .filter_map(|f| text::uri_to_path(f.uri.as_str()))
        .collect();
    #[allow(deprecated)]
    if roots.is_empty() {
        roots.extend(init.root_uri.as_ref().and_then(|u| text::uri_to_path(u.as_str())));
    }

    let caps = ServerCapabilities {
        position_encoding: Some(if utf8 { PositionEncodingKind::UTF8 } else { PositionEncodingKind::UTF16 }),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        ..Default::default()
    };
    conn.initialize_finish(
        id,
        serde_json::json!({
            "capabilities": caps,
            "serverInfo": { "name": "scopehop", "version": env!("CARGO_PKG_VERSION") },
        }),
    )?;

    let mut index = Index::new(roots.clone());
    let t = Instant::now();
    index.refresh();
    eprintln!("scopehop: indexed {} files under {:?} in {:?}", index.files.len(), roots, t.elapsed());
    let mut revisions = Revisions { roots, cache: Vec::new() };

    for msg in &conn.receiver {
        match msg {
            Message::Request(req) => {
                if conn.handle_shutdown(&req)? {
                    break;
                }
                let resp = match req.method.as_str() {
                    "scopehop/definition" | "scopehop/references" => revisions.handle(&mut index, enc, req),
                    _ => handle_request(&mut index, enc, req),
                };
                conn.sender.send(Message::Response(resp))?;
            }
            Message::Notification(n) => handle_notification(&mut index, n),
            Message::Response(_) => {}
        }
    }
    // The writer thread only stops once every sender is gone.
    drop(conn);
    io.join()?;
    Ok(())
}

fn handle_request(index: &mut Index, enc: Encoding, req: Request) -> Response {
    let id = req.id.clone();
    let result = match req.method.as_str() {
        "textDocument/definition" => serde_json::from_value::<GotoDefinitionParams>(req.params)
            .map(|p| {
                let locs = at_position(index, enc, &p.text_document_position_params, |ix, path, off| {
                    ix.definition_at(path, off)
                });
                serde_json::to_value(GotoDefinitionResponse::Array(to_locations(index, enc, &locs)))
            }),
        "textDocument/references" => serde_json::from_value::<ReferenceParams>(req.params).map(|p| {
            let include = p.context.include_declaration;
            let locs = at_position(index, enc, &p.text_document_position, |ix, path, off| {
                ix.references_at(path, off, include, false)
            });
            serde_json::to_value(to_locations(index, enc, &locs))
        }),
        "textDocument/documentHighlight" => {
            serde_json::from_value::<DocumentHighlightParams>(req.params).map(|p| {
                let locs = at_position(index, enc, &p.text_document_position_params, |ix, path, off| {
                    ix.references_at(path, off, true, true)
                });
                let hl: Vec<DocumentHighlight> = to_locations(index, enc, &locs)
                    .into_iter()
                    .map(|l| DocumentHighlight { range: l.range, kind: None })
                    .collect();
                serde_json::to_value(hl)
            })
        }
        "workspace/symbol" => serde_json::from_value::<WorkspaceSymbolParams>(req.params).map(|p| {
            let syms: Vec<SymbolInformation> = index
                .workspace_symbols(&p.query)
                .into_iter()
                .filter_map(|(path, d)| {
                    let loc = Loc { path, start: d.start, end: d.end };
                    let location = to_location(index, enc, &loc)?;
                    #[allow(deprecated)]
                    Some(SymbolInformation {
                        name: d.name,
                        kind: match d.kind {
                            DefKind::Class => SymbolKind::CLASS,
                            DefKind::Object => SymbolKind::OBJECT,
                            DefKind::Function => SymbolKind::FUNCTION,
                            DefKind::Property => SymbolKind::PROPERTY,
                            DefKind::EnumEntry => SymbolKind::ENUM_MEMBER,
                            DefKind::TypeAlias => SymbolKind::TYPE_PARAMETER,
                        },
                        tags: None,
                        deprecated: None,
                        location,
                        container_name: d.container,
                    })
                })
                .collect();
            serde_json::to_value(syms)
        }),
        _ => {
            return Response::new_err(id, ErrorCode::MethodNotFound as i32, format!("unsupported: {}", req.method));
        }
    };
    match result {
        Ok(Ok(v)) => Response::new_ok(id, v),
        Ok(Err(e)) => Response::new_err(id, ErrorCode::InternalError as i32, e.to_string()),
        Err(e) => Response::new_err(id, ErrorCode::InvalidParams as i32, e.to_string()),
    }
}

fn at_position(
    index: &mut Index,
    enc: Encoding,
    p: &TextDocumentPositionParams,
    f: impl FnOnce(&mut Index, &std::path::Path, usize) -> Vec<Loc>,
) -> Vec<Loc> {
    let Some(path) = text::uri_to_path(p.text_document.uri.as_str()) else { return Vec::new() };
    index.refresh();
    let Some(fd) = index.files.get(&path) else { return Vec::new() };
    let offset = fd.lines.offset(&fd.text, p.position, enc);
    f(index, &path, offset)
}

fn to_location(index: &Index, enc: Encoding, loc: &Loc) -> Option<Location> {
    let fd = index.files.get(&loc.path)?;
    let uri: Uri = text::path_to_uri(&loc.path).parse().ok()?;
    Some(Location {
        uri,
        range: Range::new(
            fd.lines.position(&fd.text, loc.start, enc),
            fd.lines.position(&fd.text, loc.end, enc),
        ),
    })
}

fn to_locations(index: &Index, enc: Encoding, locs: &[Loc]) -> Vec<Location> {
    locs.iter().filter_map(|l| to_location(index, enc, l)).collect()
}

// ---------------------------------------------------------------------------
// Revisions
// ---------------------------------------------------------------------------

/// Parameters of `scopehop/definition` and `scopehop/references`: a
/// position in a file as it is in a git revision (the removed side of a
/// diff, say). Without `revision`, the workspace as the editor has it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevisionParams {
    revision: Option<String>,
    text_document: lsp_types::TextDocumentIdentifier,
    position: lsp_types::Position,
    #[serde(default)]
    context: Option<lsp_types::ReferenceContext>,
}

/// Indexes of the last few revisions asked about, newest last.
struct Revisions {
    roots: Vec<PathBuf>,
    cache: Vec<(String, Index)>,
}

const REVISIONS_KEPT: usize = 4;

impl Revisions {
    /// The index of `rev` in the repository containing `dir`.
    fn index(&mut self, dir: &Path, rev: &str) -> Result<&mut Index> {
        let (top, commit) = files::resolve_commit(dir, rev)?;
        let key = format!("{}@{commit}", top.display());
        if let Some(i) = self.cache.iter().position(|(k, _)| *k == key) {
            let hit = self.cache.remove(i);
            self.cache.push(hit);
        } else {
            let t = Instant::now();
            let roots: Vec<PathBuf> = match self.roots.iter().filter(|r| r.starts_with(&top)).cloned().collect::<Vec<_>>() {
                v if v.is_empty() => vec![top.clone()],
                v => v,
            };
            let mut ix = Index::with_files(roots, Box::new(Revision::load(top, commit)?));
            ix.refresh();
            eprintln!("scopehop: indexed {} files of {key} in {:?}", ix.files.len(), t.elapsed());
            if self.cache.len() >= REVISIONS_KEPT {
                self.cache.remove(0);
            }
            self.cache.push((key, ix));
        }
        Ok(&mut self.cache.last_mut().expect("just pushed").1)
    }

    fn handle(&mut self, workspace: &mut Index, enc: Encoding, req: Request) -> Response {
        let id = req.id.clone();
        let p: RevisionParams = match serde_json::from_value(req.params) {
            Ok(p) => p,
            Err(e) => return Response::new_err(id, ErrorCode::InvalidParams as i32, e.to_string()),
        };
        let Some(path) = text::uri_to_path(p.text_document.uri.as_str()) else {
            return Response::new_err(id, ErrorCode::InvalidParams as i32, "not a file URI".into());
        };
        let refs = req.method == "scopehop/references";
        let include = p.context.is_none_or(|c| c.include_declaration);
        let run = |ix: &mut Index| -> Vec<Location> {
            ix.refresh();
            let Some(fd) = ix.files.get(&path) else { return Vec::new() };
            let off = fd.lines.offset(&fd.text, p.position, enc);
            let locs = if refs { ix.references_at(&path, off, include, false) } else { ix.definition_at(&path, off) };
            to_locations(ix, enc, &locs)
        };
        let result = match &p.revision {
            None => Ok(run(workspace)),
            Some(rev) => {
                let dir = path.parent().map(existing_ancestor).unwrap_or_default();
                self.index(&dir, rev).map(run)
            }
        };
        match result {
            Ok(locs) => Response::new_ok(id, locs),
            Err(e) => Response::new_err(id, ErrorCode::RequestFailed as i32, e.to_string()),
        }
    }
}

/// The nearest directory at or above `dir` that exists (a file deleted
/// since the revision may sit in a deleted directory).
fn existing_ancestor(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| d.is_dir()).unwrap_or(dir).to_path_buf()
}

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

fn cli(cmd: &str, args: &[String]) -> Result<()> {
    let (mut rev, mut root, mut json, mut decl) = (None, None, false, true);
    let mut pos = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rev" => rev = Some(it.next().context("--rev needs a revision")?.clone()),
            "--root" => root = Some(absolute(Path::new(it.next().context("--root needs a directory")?))),
            "--json" => json = true,
            "--no-decl" => decl = false,
            _ if a.starts_with("--") => anyhow::bail!("unknown option {a}\n{USAGE}"),
            _ => pos.push(a.clone()),
        }
    }
    let [file, line, col] = pos.as_slice() else { anyhow::bail!("expected FILE LINE COL\n{USAGE}") };
    let file = absolute(Path::new(file));
    let (line, col): (u32, u32) = (line.parse().context("LINE")?, col.parse().context("COL")?);
    anyhow::ensure!(line > 0 && col > 0, "LINE and COL are 1-based");
    let dir = existing_ancestor(file.parent().unwrap_or(Path::new(".")));

    let mut ix = match &rev {
        Some(rev) => {
            let (top, commit) = files::resolve_commit(&dir, rev)?;
            let roots = vec![root.clone().unwrap_or_else(|| top.clone())];
            Index::with_files(roots, Box::new(Revision::load(top, commit)?))
        }
        None => {
            let root = root.clone().or_else(|| files::resolve_commit(&dir, "HEAD").ok().map(|(top, _)| top));
            Index::new(vec![root.unwrap_or_else(|| absolute(Path::new(".")))])
        }
    };
    ix.refresh();
    let Some(fd) = ix.files.get(&file) else {
        anyhow::bail!("{} is not a source file{}", file.display(), rev.map(|r| format!(" in {r}")).unwrap_or_default());
    };
    let off = fd.lines.offset(&fd.text, lsp_types::Position::new(line - 1, col - 1), Encoding::Utf8);
    let locs = if cmd == "def" { ix.definition_at(&file, off) } else { ix.references_at(&file, off, decl, false) };

    let cwd = std::env::current_dir().unwrap_or_default();
    let mut out = Vec::new();
    for l in &locs {
        let fd = &ix.files[&l.path];
        let (s, e) = (fd.lines.position(&fd.text, l.start, Encoding::Utf8), fd.lines.position(&fd.text, l.end, Encoding::Utf8));
        if json {
            out.push(serde_json::json!({
                "path": l.path, "line": s.line + 1, "column": s.character + 1,
                "endLine": e.line + 1, "endColumn": e.character + 1,
            }));
        } else {
            let shown = l.path.strip_prefix(&cwd).unwrap_or(&l.path);
            let text = fd.text.lines().nth(s.line as usize).unwrap_or("").trim();
            println!("{}:{}:{}: {text}", shown.display(), s.line + 1, s.character + 1);
        }
    }
    if json {
        println!("{}", serde_json::Value::Array(out));
    }
    Ok(())
}

/// `p` made absolute against the current directory, with `.` and `..`
/// removed and the part that exists canonicalized (so `/tmp` and
/// `/private/tmp` agree with what git reports), without requiring the
/// file itself to exist.
fn absolute(p: &Path) -> PathBuf {
    let joined = std::env::current_dir().unwrap_or_default().join(p);
    let mut clean = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            c => clean.push(c),
        }
    }
    let base = existing_ancestor(&clean);
    let rest = clean.strip_prefix(&base).map(Path::to_path_buf).unwrap_or_default();
    std::fs::canonicalize(&base).unwrap_or(base).join(rest)
}

fn handle_notification(index: &mut Index, n: Notification) {
    match n.method.as_str() {
        "textDocument/didOpen" => {
            if let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(n.params) {
                if let Some(path) = text::uri_to_path(p.text_document.uri.as_str()) {
                    if index::is_source(&path) {
                        index.set_open(path, p.text_document.text);
                    }
                }
            }
        }
        "textDocument/didChange" => {
            if let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) {
                if let (Some(path), Some(change)) =
                    (text::uri_to_path(p.text_document.uri.as_str()), p.content_changes.into_iter().last())
                {
                    if index::is_source(&path) {
                        index.set_open(path, change.text);
                    }
                }
            }
        }
        "textDocument/didClose" => {
            if let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) {
                if let Some(path) = text::uri_to_path(p.text_document.uri.as_str()) {
                    index.close(&path);
                }
            }
        }
        _ => {}
    }
}
