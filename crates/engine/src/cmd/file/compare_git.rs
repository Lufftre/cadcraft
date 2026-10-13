//! COMPARE against a version in git: the drawing's own file as committed on a branch, a tag or a
//! commit (`main`, `HEAD`, `HEAD~2`, `v1.0`, `a1b2c3d`), read with the `git` program of the
//! system. Desktop only: the web build has no files and no `git`.

use super::super::*;
use crate::Session;

/// A revision name is at most this long.
const MAX_REVISION: usize = 256;
/// Branches offered at the Compare with Branch prompt.
const MAX_BRANCHES: usize = 20;
/// A drawing read from git is at most this large (bytes).
#[cfg(not(target_arch = "wasm32"))]
const MAX_BYTES: u64 = 1 << 30;

/// The current drawing's file, which git is asked about.
pub(super) fn drawing_path(s: &Session, cmd: &str) -> Result<String> {
    s.state()?.path.clone().ok_or_else(|| bad(cmd, "the drawing has no file yet: save it in a git repository first"))
}

/// `rev` checked as a revision name: not empty, no whitespace, control characters or `:` (the
/// path is ours to add), and not starting with `-` (git would read an option).
pub(super) fn check_revision(cmd: &str, rev: &str) -> Result<()> {
    let ok = !rev.is_empty()
        && rev.len() <= MAX_REVISION
        && !rev.starts_with('-')
        && !rev.chars().any(|c| c.is_whitespace() || c.is_control() || c == ':');
    if ok { Ok(()) } else { Err(bad(cmd, format!("`{rev}` is not a branch, tag or commit name"))) }
}

/// The bytes of `path` as committed in revision `rev` of its git repository.
pub(super) fn read_revision(cmd: &str, path: &str, rev: &str) -> Result<Vec<u8>> {
    check_revision(cmd, rev)?;
    native::read_revision(cmd, path, rev)
}

/// What the drawing at `path` can be compared with in its git repository: the local branches
/// other than the current one (`main` or `master` first, then the most recently committed to),
/// then `HEAD` (the last commit on the current branch). Empty outside a repository or without
/// `git`.
pub fn branches(path: &str) -> Vec<String> {
    let mut out = native::branches(path);
    out.truncate(MAX_BRANCHES);
    if !out.is_empty() {
        out.push("HEAD".into());
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::Read;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use super::*;

    /// `git -C <folder of path> …`.
    fn git(path: &str) -> Command {
        let dir = Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut c = Command::new("git");
        c.arg("-C").arg(dir);
        c
    }

    pub(super) fn read_revision(cmd: &str, path: &str, rev: &str) -> Result<Vec<u8>> {
        let file = Path::new(path).file_name().and_then(|f| f.to_str()).ok_or_else(|| bad(cmd, format!("{path}: no file name")))?;
        let mut child = git(path)
            .arg("cat-file")
            .arg("blob")
            .arg(format!("{rev}:./{file}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| bad(cmd, format!("can't run git ({e}): is it installed?")))?;
        let mut bytes = Vec::new();
        if let Some(out) = child.stdout.take() {
            out.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| bad(cmd, format!("git: {e}")))?;
        }
        let mut err = String::new();
        if let Some(e) = child.stderr.take() {
            let _ = e.take(4096).read_to_string(&mut err);
        }
        let status = child.wait().map_err(|e| bad(cmd, format!("git: {e}")))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(bad(cmd, format!("{file} on {rev} is larger than {} MB", MAX_BYTES >> 20)));
        }
        if !status.success() {
            let why = err.lines().next().unwrap_or("").trim().trim_start_matches("fatal: ");
            return Err(bad(cmd, format!("no {file} on `{rev}` in git ({why})")));
        }
        Ok(bytes)
    }

    pub(super) fn branches(path: &str) -> Vec<String> {
        let out = git(path).args(["for-each-ref", "--sort=-committerdate", "--format=%(refname:short)", "refs/heads"]).output();
        let Some(out) = out.ok().filter(|o| o.status.success()) else { return Vec::new() };
        let current = git(path).args(["branch", "--show-current"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|b| check_revision("compare", b).is_ok())
            .map(str::to_string)
            .take(MAX_BRANCHES * 4)
            .collect();
        if let Some(c) = current {
            names.retain(|b| *b != c);
        }
        names.sort_by_key(|b| !matches!(b.as_str(), "main" | "master"));
        names
    }
}

#[cfg(target_arch = "wasm32")]
mod native {
    use super::*;

    pub(super) fn read_revision(cmd: &str, _path: &str, _rev: &str) -> Result<Vec<u8>> {
        Err(bad(cmd, "git versions are not available on the web"))
    }

    pub(super) fn branches(_path: &str) -> Vec<String> {
        Vec::new()
    }
}
