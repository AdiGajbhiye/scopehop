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
use scopehop::index::{self, FileData, Index, Loc};
use scopehop::lang::DefKind;
use scopehop::outline::{self, Block, Region};
use scopehop::text::{self, Encoding};

const USAGE: &str = "\
usage:
  scopehop                      language server over stdio
  scopehop def  [options] FILE LINE COL
  scopehop refs [options] FILE LINE COL
  scopehop outline [options] FILE
  scopehop context [options] FILE#BLOCK | FILE:LINE[-LINE] | FILE

LINE and COL are 1-based; COL counts bytes. Options:
  --rev REV    resolve in a git revision (branch, tag, HEAD~1, hash): FILE
               is the path it had there, and may not exist any more
  --root DIR   project root (default: the git repository, or the current
               directory)
  --json       print a JSON array of {path, line, column, endLine,
               endColumn} instead of path:line:col lines
  --no-decl    refs: leave out the declaration itself
  --no-callers context: leave out what uses the region's declarations

outline lists the file's blocks (classes, functions, methods, properties)
with their lines, named by nesting (Circle.area). context lists what a
block or range of lines uses that is declared outside it, with the block
each declaration is in, and the blocks elsewhere that use it.";

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
        Some(cmd @ ("def" | "refs" | "outline" | "context")) => return cli(cmd, &args[1..]),
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
                    "scopehop/definition" | "scopehop/references" | "scopehop/outline" | "scopehop/context" => {
                        revisions.handle(&mut index, enc, req)
                    }
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
    /// definition, references
    position: Option<lsp_types::Position>,
    /// references
    #[serde(default)]
    context: Option<lsp_types::ReferenceContext>,
    /// context: the region is this block (`Circle.area`), else `range`,
    /// else the whole file.
    block: Option<String>,
    range: Option<Range>,
    /// context: also list what uses the region's declarations (default true).
    callers: Option<bool>,
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
        let method = req.method.as_str();
        let run = |ix: &mut Index| -> Result<serde_json::Value, String> {
            ix.refresh();
            let Some(fd) = ix.files.get(&path) else { return Ok(serde_json::Value::Null) };
            let json = |v: serde_json::Result<serde_json::Value>| v.map_err(|e| e.to_string());
            match method {
                "scopehop/outline" => {
                    let blocks: Vec<serde_json::Value> =
                        outline::outline(fd).iter().map(|b| lsp_block(fd, enc, b)).collect();
                    Ok(serde_json::Value::Array(blocks))
                }
                "scopehop/context" => {
                    let region = match (&p.block, &p.range) {
                        (Some(b), _) => Region::Block(b.clone()),
                        (None, Some(r)) => {
                            let (s, e) = (fd.lines.offset(&fd.text, r.start, enc), fd.lines.offset(&fd.text, r.end, enc));
                            return Ok(lsp_context(ix, enc, &path, s, e.max(s), None, p.callers.unwrap_or(true)));
                        }
                        (None, None) => Region::Whole,
                    };
                    let (s, e, block) = region.range(fd)?;
                    Ok(lsp_context(ix, enc, &path, s, e, block, p.callers.unwrap_or(true)))
                }
                _ => {
                    let pos = p.position.ok_or("missing position")?;
                    let off = fd.lines.offset(&fd.text, pos, enc);
                    let locs = if method == "scopehop/references" {
                        ix.references_at(&path, off, p.context.is_none_or(|c| c.include_declaration), false)
                    } else {
                        ix.definition_at(&path, off)
                    };
                    json(serde_json::to_value(to_locations(ix, enc, &locs)))
                }
            }
        };
        let result = match &p.revision {
            None => run(workspace),
            Some(rev) => {
                let dir = path.parent().map(existing_ancestor).unwrap_or_default();
                self.index(&dir, rev).map_err(|e| e.to_string()).and_then(run)
            }
        };
        match result {
            Ok(v) => Response::new_ok(id, v),
            Err(e) => Response::new_err(id, ErrorCode::RequestFailed as i32, e),
        }
    }
}

fn lsp_range(fd: &FileData, enc: Encoding, start: usize, end: usize) -> Range {
    Range::new(fd.lines.position(&fd.text, start, enc), fd.lines.position(&fd.text, end, enc))
}

fn lsp_block(fd: &FileData, enc: Encoding, b: &Block) -> serde_json::Value {
    serde_json::json!({
        "name": b.name, "kind": b.kind.as_str(),
        "range": lsp_range(fd, enc, b.start, b.end),
        "selectionRange": lsp_range(fd, enc, b.name_start, b.name_end),
    })
}

/// The answer to `scopehop/context`, with LSP ranges and file URIs.
fn lsp_context(
    ix: &mut Index,
    enc: Encoding,
    path: &Path,
    start: usize,
    end: usize,
    block: Option<Block>,
    callers: bool,
) -> serde_json::Value {
    let ctx = outline::context(ix, path, start, end, callers);
    let uri = |p: &Path| text::path_to_uri(p);
    let sites = |locs: &[Loc]| -> Vec<Range> {
        locs.iter().map(|l| lsp_range(&ix.files[&l.path], enc, l.start, l.end)).collect()
    };
    let fd = &ix.files[path];
    let uses: Vec<serde_json::Value> = ctx
        .uses
        .iter()
        .map(|u| {
            let f = &ix.files[&u.target.path];
            serde_json::json!({
                "name": u.name,
                "location": { "uri": uri(&u.target.path), "range": lsp_range(f, enc, u.target.start, u.target.end) },
                "block": u.block.as_ref().map(|b| lsp_block(f, enc, b)),
                "ambiguous": u.ambiguous,
                "sites": sites(&u.sites),
            })
        })
        .collect();
    let used_by: Vec<serde_json::Value> = ctx
        .used_by
        .iter()
        .map(|c| {
            let f = &ix.files[&c.path];
            serde_json::json!({
                "uri": uri(&c.path), "block": c.block.as_ref().map(|b| lsp_block(f, enc, b)),
                "names": c.names, "sites": sites(&c.sites),
            })
        })
        .collect();
    serde_json::json!({
        "region": { "range": lsp_range(fd, enc, start, end), "block": block.as_ref().map(|b| lsp_block(fd, enc, b)) },
        "uses": uses, "usedBy": used_by, "unresolved": ctx.unresolved,
    })
}

/// The nearest directory at or above `dir` that exists (a file deleted
/// since the revision may sit in a deleted directory).
fn existing_ancestor(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| d.is_dir()).unwrap_or(dir).to_path_buf()
}

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

struct Opts {
    rev: Option<String>,
    root: Option<PathBuf>,
    json: bool,
    decl: bool,
    callers: bool,
    pos: Vec<String>,
}

fn options(args: &[String]) -> Result<Opts> {
    let mut o = Opts { rev: None, root: None, json: false, decl: true, callers: true, pos: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rev" => o.rev = Some(it.next().context("--rev needs a revision")?.clone()),
            "--root" => o.root = Some(absolute(Path::new(it.next().context("--root needs a directory")?))),
            "--json" => o.json = true,
            "--no-decl" => o.decl = false,
            "--no-callers" => o.callers = false,
            _ if a.starts_with("--") => anyhow::bail!("unknown option {a}\n{USAGE}"),
            _ => o.pos.push(a.clone()),
        }
    }
    Ok(o)
}

/// The index that `file` (absolute) is in: the working tree, or `--rev`.
fn open_index(o: &Opts, file: &Path) -> Result<Index> {
    let dir = existing_ancestor(file.parent().unwrap_or(Path::new(".")));
    let mut ix = match &o.rev {
        Some(rev) => {
            let (top, commit) = files::resolve_commit(&dir, rev)?;
            let roots = vec![o.root.clone().unwrap_or_else(|| top.clone())];
            Index::with_files(roots, Box::new(Revision::load(top, commit)?))
        }
        None => {
            let root = o.root.clone().or_else(|| files::resolve_commit(&dir, "HEAD").ok().map(|(top, _)| top));
            Index::new(vec![root.unwrap_or_else(|| absolute(Path::new(".")))])
        }
    };
    ix.refresh();
    if !ix.files.contains_key(file) {
        let at = o.rev.as_ref().map(|r| format!(" in {r}")).unwrap_or_default();
        anyhow::bail!("{} is not a source file{at}", file.display());
    }
    Ok(ix)
}

fn cli(cmd: &str, args: &[String]) -> Result<()> {
    let o = options(args)?;
    match cmd {
        "outline" => return cli_outline(&o),
        "context" => return cli_context(&o),
        _ => {}
    }
    let [file, line, col] = o.pos.as_slice() else { anyhow::bail!("expected FILE LINE COL\n{USAGE}") };
    let file = absolute(Path::new(file));
    let (line, col): (u32, u32) = (line.parse().context("LINE")?, col.parse().context("COL")?);
    anyhow::ensure!(line > 0 && col > 0, "LINE and COL are 1-based");
    let mut ix = open_index(&o, &file)?;
    let fd = &ix.files[&file];
    let off = fd.lines.offset(&fd.text, lsp_types::Position::new(line - 1, col - 1), Encoding::Utf8);
    let locs = if cmd == "def" { ix.definition_at(&file, off) } else { ix.references_at(&file, off, o.decl, false) };

    let cwd = std::env::current_dir().unwrap_or_default();
    let mut out = Vec::new();
    for l in &locs {
        let fd = &ix.files[&l.path];
        let (s, e) = (fd.lines.position(&fd.text, l.start, Encoding::Utf8), fd.lines.position(&fd.text, l.end, Encoding::Utf8));
        if o.json {
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
    if o.json {
        println!("{}", serde_json::Value::Array(out));
    }
    Ok(())
}

/// 1-based line and byte column of `off` in `fd`.
fn line_col(fd: &FileData, off: usize) -> (u32, u32) {
    let p = fd.lines.position(&fd.text, off, Encoding::Utf8);
    (p.line + 1, p.character + 1)
}

/// The last line of `start..end` (an end at the start of a line belongs to
/// the line before).
fn end_line(fd: &FileData, start: usize, end: usize) -> u32 {
    line_col(fd, end.saturating_sub(1).max(start)).0
}

fn block_json(fd: &FileData, b: &Block) -> serde_json::Value {
    let (nl, nc) = line_col(fd, b.name_start);
    serde_json::json!({
        "name": b.name, "kind": b.kind.as_str(),
        "line": line_col(fd, b.start).0, "endLine": end_line(fd, b.start, b.end),
        "nameLine": nl, "nameColumn": nc,
    })
}

fn cli_outline(o: &Opts) -> Result<()> {
    let [file] = o.pos.as_slice() else { anyhow::bail!("expected FILE\n{USAGE}") };
    let file = absolute(Path::new(file));
    let ix = open_index(o, &file)?;
    let fd = &ix.files[&file];
    let blocks = outline::outline(fd);
    if o.json {
        println!("{}", serde_json::Value::Array(blocks.iter().map(|b| block_json(fd, b)).collect()));
        return Ok(());
    }
    for b in &blocks {
        println!("{:>5}-{:<5} {:<9} {}", line_col(fd, b.start).0, end_line(fd, b.start, b.end), b.kind.as_str(), b.name);
    }
    Ok(())
}

fn cli_context(o: &Opts) -> Result<()> {
    let [spec] = o.pos.as_slice() else { anyhow::bail!("expected FILE#BLOCK, FILE:LINE[-LINE] or FILE\n{USAGE}") };
    let (file, region) = Region::parse(spec);
    let file = absolute(Path::new(file));
    let mut ix = open_index(o, &file)?;
    let (start, end, block) = region.range(&ix.files[&file]).map_err(anyhow::Error::msg)?;
    let ctx = outline::context(&mut ix, &file, start, end, o.callers);

    let cwd = std::env::current_dir().unwrap_or_default();
    let shown = |p: &Path| p.strip_prefix(&cwd).unwrap_or(p).display().to_string();
    let site = |l: &Loc| {
        let (line, column) = line_col(&ix.files[&l.path], l.start);
        serde_json::json!({ "line": line, "column": column })
    };
    let fd = &ix.files[&file];
    if o.json {
        let uses: Vec<serde_json::Value> = ctx
            .uses
            .iter()
            .map(|u| {
                let f = &ix.files[&u.target.path];
                let (line, column) = line_col(f, u.target.start);
                serde_json::json!({
                    "name": u.name, "path": u.target.path, "line": line, "column": column,
                    "block": u.block.as_ref().map(|b| block_json(f, b)),
                    "ambiguous": u.ambiguous,
                    "sites": u.sites.iter().map(site).collect::<Vec<_>>(),
                })
            })
            .collect();
        let used_by: Vec<serde_json::Value> = ctx
            .used_by
            .iter()
            .map(|c| {
                let f = &ix.files[&c.path];
                serde_json::json!({
                    "path": c.path, "block": c.block.as_ref().map(|b| block_json(f, b)),
                    "names": c.names, "sites": c.sites.iter().map(site).collect::<Vec<_>>(),
                })
            })
            .collect();
        let out = serde_json::json!({
            "region": {
                "path": file, "block": block.as_ref().map(|b| block_json(fd, b)),
                "line": line_col(fd, start).0, "endLine": end_line(fd, start, end),
            },
            "uses": uses, "usedBy": used_by, "unresolved": ctx.unresolved,
        });
        println!("{out}");
        return Ok(());
    }
    let place = |p: &Path, b: &Option<Block>, off: usize| {
        let f = &ix.files[p];
        match b {
            Some(b) => format!("{}#{} ({}-{})", shown(p), b.name, line_col(f, b.start).0, end_line(f, b.start, b.end)),
            None => format!("{}:{}", shown(p), line_col(f, off).0),
        }
    };
    println!("uses:");
    for u in &ctx.uses {
        let more = if u.ambiguous { "  (one of several)" } else { "" };
        println!("  {:<48} {} x{}{more}", place(&u.target.path, &u.block, u.target.start), u.name, u.sites.len());
    }
    if o.callers {
        println!("used by:");
        for c in &ctx.used_by {
            let first = c.sites.first().map_or(0, |s| s.start);
            println!("  {:<48} {} x{}", place(&c.path, &c.block, first), c.names.join(", "), c.sites.len());
        }
    }
    if !ctx.unresolved.is_empty() {
        println!("unresolved: {}", ctx.unresolved.join(", "));
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
