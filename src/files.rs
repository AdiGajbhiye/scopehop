//! Where the index reads files from: the working tree ([`Disk`]), or one
//! git commit ([`Revision`]), so code can be resolved as it was in an older
//! revision (the removed side of a diff).
//!
//! Everything that looks at files goes through [`Files`]: listing sources,
//! reading them, and the probes languages make in `Language::place` and
//! `Language::absolute_import` (`go.mod`, `tsconfig.json`, `.csproj`,
//! `__init__.py`). Paths are absolute; a revision's files are named by
//! where they would be in the working tree.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};

use crate::lang;

pub trait Files: Send + Sync {
    /// Source files under `roots`, with a stamp that changes when the file
    /// does (`None` when unknown: always read again).
    fn sources(&self, roots: &[PathBuf]) -> Vec<(PathBuf, Option<SystemTime>)>;
    fn read(&self, path: &Path) -> Option<String>;
    fn is_file(&self, path: &Path) -> bool;
    /// The files directly in `dir`.
    fn files_in(&self, dir: &Path) -> Vec<PathBuf>;
}

/// The working tree. Gitignored files and `node_modules` are skipped.
pub struct Disk;

impl Files for Disk {
    fn sources(&self, roots: &[PathBuf]) -> Vec<(PathBuf, Option<SystemTime>)> {
        let mut out = Vec::new();
        for root in roots {
            // Installed packages are libraries, even when not gitignored.
            let walk = ignore::WalkBuilder::new(root).filter_entry(|e| e.file_name() != "node_modules").build();
            for entry in walk.flatten() {
                let path = entry.path();
                if entry.file_type().is_some_and(|t| t.is_file()) && lang::for_path(path).is_some() {
                    let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
                    out.push((path.to_path_buf(), mtime));
                }
            }
        }
        out
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn files_in(&self, dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| e.path()).collect()
    }
}

/// The files of one commit, read once with `git ls-tree` and `git cat-file
/// --batch`. Sources and the config files languages read are kept in memory;
/// other files are only known to exist.
pub struct Revision {
    /// The full commit id.
    pub commit: String,
    /// The repository's top directory, which a revision's paths are under.
    pub top: PathBuf,
    texts: HashMap<PathBuf, String>,
    others: HashSet<PathBuf>,
    dirs: HashMap<PathBuf, Vec<PathBuf>>,
}

/// Files read along with the sources because a language looks into them.
fn is_config(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(name, "go.mod" | "tsconfig.json" | "jsconfig.json")
        || (name.starts_with("tsconfig") && name.ends_with(".json"))
        || path.extension().is_some_and(|e| e == "csproj")
}

fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// The top directory of the repository containing `dir`, and the full id
/// of the commit `rev` names there (a branch, tag, `HEAD~1`, a hash).
pub fn resolve_commit(dir: &Path, rev: &str) -> Result<(PathBuf, String)> {
    let top = String::from_utf8(git(dir, &["rev-parse", "--show-toplevel"])?)?;
    let top = PathBuf::from(top.trim());
    let commit = String::from_utf8(git(&top, &["rev-parse", "--verify", "--end-of-options", &format!("{rev}^{{commit}}")])?)?;
    Ok((top, commit.trim().to_string()))
}

impl Revision {
    /// The commit `rev` names in the repository containing `dir`.
    pub fn open(dir: &Path, rev: &str) -> Result<Revision> {
        let (top, commit) = resolve_commit(dir, rev)?;
        Self::load(top, commit)
    }

    /// `commit` (a full id) of the repository at `top`.
    pub fn load(top: PathBuf, commit: String) -> Result<Revision> {

        // `<mode> <type> <id>\t<path>`, NUL-terminated.
        let listing = git(&top, &["ls-tree", "-r", "-z", "--full-tree", &commit])?;
        let mut wanted: Vec<(PathBuf, String)> = Vec::new();
        let mut others = HashSet::new();
        let mut dirs: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let entry = String::from_utf8_lossy(entry);
            let Some((meta, rel)) = entry.split_once('\t') else { continue };
            let mut meta = meta.split(' ');
            let (Some(_mode), Some(kind), Some(id)) = (meta.next(), meta.next(), meta.next()) else { continue };
            if kind != "blob" {
                continue;
            }
            let path = top.join(rel);
            if let Some(parent) = path.parent() {
                dirs.entry(parent.to_path_buf()).or_default().push(path.clone());
            }
            let in_node_modules = Path::new(rel).components().any(|c| c.as_os_str() == "node_modules");
            if (lang::for_path(&path).is_some() && !in_node_modules) || is_config(&path) {
                wanted.push((path, id.to_string()));
            } else {
                others.insert(path);
            }
        }
        let texts = cat_files(&top, &wanted)?;
        Ok(Revision { commit, top, texts, others, dirs })
    }
}

/// Blob contents by path, from one `git cat-file --batch`.
fn cat_files(top: &Path, wanted: &[(PathBuf, String)]) -> Result<HashMap<PathBuf, String>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("running git cat-file")?;
    let mut stdin = child.stdin.take().context("git stdin")?;
    let ids: Vec<String> = wanted.iter().map(|(_, id)| id.clone()).collect();
    // Written from another thread so a full stdout pipe cannot stall us.
    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        for id in ids {
            writeln!(stdin, "{id}")?;
        }
        Ok(())
    });
    let mut out = BufReader::new(child.stdout.take().context("git stdout")?);
    let mut texts = HashMap::new();
    for (path, _) in wanted {
        // `<id> <type> <size>\n<contents>\n`
        let mut header = String::new();
        out.read_line(&mut header)?;
        let size: usize = header.split_whitespace().nth(2).and_then(|s| s.parse().ok()).context("git cat-file header")?;
        let mut body = vec![0; size + 1];
        out.read_exact(&mut body)?;
        body.pop();
        if let Ok(text) = String::from_utf8(body) {
            texts.insert(path.clone(), text);
        }
    }
    writer.join().map_err(|_| anyhow::anyhow!("git cat-file writer panicked"))??;
    child.wait()?;
    Ok(texts)
}

impl Files for Revision {
    fn sources(&self, roots: &[PathBuf]) -> Vec<(PathBuf, Option<SystemTime>)> {
        let mut out: Vec<_> = self
            .texts
            .keys()
            .filter(|p| lang::for_path(p).is_some() && roots.iter().any(|r| p.starts_with(r)))
            // A commit never changes.
            .map(|p| (p.clone(), Some(SystemTime::UNIX_EPOCH)))
            .collect();
        out.sort();
        out
    }

    fn read(&self, path: &Path) -> Option<String> {
        self.texts.get(path).cloned()
    }

    fn is_file(&self, path: &Path) -> bool {
        self.texts.contains_key(path) || self.others.contains(path)
    }

    fn files_in(&self, dir: &Path) -> Vec<PathBuf> {
        self.dirs.get(dir).cloned().unwrap_or_default()
    }
}
