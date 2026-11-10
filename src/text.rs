//! Byte offsets <-> LSP positions (UTF-8 or UTF-16 columns).

use lsp_types::Position;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

#[derive(Clone, Debug, Default)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(memchr::memchr_iter(b'\n', text.as_bytes()).map(|i| i + 1));
        Self { starts }
    }

    pub fn position(&self, text: &str, offset: usize, enc: Encoding) -> Position {
        let offset = offset.min(text.len());
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let start = self.starts[line];
        let col = match enc {
            Encoding::Utf8 => offset - start,
            Encoding::Utf16 => text[start..offset].encode_utf16().count(),
        };
        Position::new(line as u32, col as u32)
    }

    pub fn offset(&self, text: &str, pos: Position, enc: Encoding) -> usize {
        let Some(&start) = self.starts.get(pos.line as usize) else {
            return text.len();
        };
        let end = self
            .starts
            .get(pos.line as usize + 1)
            .copied()
            .unwrap_or(text.len());
        let line = &text[start..end];
        let want = pos.character as usize;
        let mut units = 0;
        for (i, ch) in line.char_indices() {
            if units >= want || ch == '\n' {
                return start + i;
            }
            units += match enc {
                Encoding::Utf8 => ch.len_utf8(),
                Encoding::Utf16 => ch.len_utf16(),
            };
        }
        end
    }
}

pub fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Whole-word occurrences of `word` in `text`.
pub fn word_occurrences<'a>(text: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    let bytes = text.as_bytes();
    memchr::memmem::find_iter(bytes, word.as_bytes()).filter(move |&i| {
        let before = i == 0 || !is_ident_byte(bytes[i - 1]);
        let j = i + word.len();
        let after = j >= bytes.len() || !is_ident_byte(bytes[j]);
        before && after
    })
}

/// The identifier-like word covering `offset`, as a byte range.
pub fn word_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let mut s = offset.min(b.len());
    while s > 0 && is_ident_byte(b[s - 1]) {
        s -= 1;
    }
    let mut e = offset.min(b.len());
    while e < b.len() && is_ident_byte(b[e]) {
        e += 1;
    }
    (s < e && !b[s].is_ascii_digit()).then_some((s, e))
}

pub fn path_to_uri(path: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned().into())
}
