use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentHighlight, DocumentHighlightParams, GotoDefinitionParams, GotoDefinitionResponse,
    InitializeParams, Location, OneOf, PositionEncodingKind, Range, ReferenceParams,
    ServerCapabilities, SymbolInformation, SymbolKind, TextDocumentPositionParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri, WorkspaceSymbolParams,
};
use treenav::index::{self, Index, Loc};
use treenav::kotlin::DefKind;
use treenav::text::{self, Encoding};

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version") {
        println!("treenav {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
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
            "serverInfo": { "name": "treenav", "version": env!("CARGO_PKG_VERSION") },
        }),
    )?;

    let mut index = Index::new(roots.clone());
    let t = Instant::now();
    index.refresh();
    eprintln!("treenav: indexed {} files under {:?} in {:?}", index.files.len(), roots, t.elapsed());

    for msg in &conn.receiver {
        match msg {
            Message::Request(req) => {
                if conn.handle_shutdown(&req)? {
                    break;
                }
                let resp = handle_request(&mut index, enc, req);
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
