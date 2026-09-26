//! Safety guard, shell command builder and the actual delete.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::disk::MountTable;

/// Why `path` must not be trashed or deleted from the GUI, if it must not.
pub fn protection(path: &Path, home: &Path, mounts: &MountTable) -> Option<&'static str> {
    if path.components().count() <= 2 {
        return Some("top-level system directory");
    }
    if home.starts_with(path) {
        return Some("your home directory or one of its parents");
    }
    if mounts.at(path).is_some() {
        return Some("mount point");
    }
    let dev = |p: &Path| fs::symlink_metadata(p).map(|m| m.dev()).ok();
    if let (Some(d), Some(parent)) = (dev(path), path.parent()) {
        if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) && dev(parent) != Some(d) {
            return Some("btrfs subvolume root");
        }
    }
    None
}

/// Quote for pasting into sh, bash, zsh *and* fish: single-quote everything and
/// emit `'` and `\` outside the quotes, where both shell families agree.
pub fn quote(path: &Path) -> String {
    let s = path.to_string_lossy();
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        return s.into_owned();
    }
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("'\\''"),
            '\\' => out.push_str("'\\\\'"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn writable(dir: &Path) -> bool {
    CString::new(dir.as_os_str().as_bytes())
        .is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
}

/// Ready-to-paste commands that delete `paths`. `force_sudo` is for paths a
/// GUI delete already failed on (a root-owned file deep inside, say).
pub fn commands(paths: &[&Path], home: &Path, mounts: &MountTable, force_sudo: bool) -> String {
    let mut out = String::new();
    let mut trashable = Vec::new();
    for p in paths {
        let q = quote(p);
        if let Some(why) = protection(p, home, mounts) {
            out.push_str(&format!("# PROTECTED ({why}), think twice: rm -rf -- {q}\n"));
            continue;
        }
        let parent_ok = !force_sudo && p.parent().is_none_or(writable);
        let sudo = if parent_ok { "" } else { "sudo " };
        out.push_str(&format!("{sudo}rm -rf -- {q}\n"));
        if parent_ok {
            trashable.push(q);
        }
    }
    if !trashable.is_empty() {
        out.push_str(&format!("# or move to Trash instead: gio trash -- {}\n", trashable.join(" ")));
    }
    out
}

/// Permanently delete a file, symlink or whole directory tree.
pub fn delete(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sh(cmd: &str) -> String {
        let out = std::process::Command::new("sh").arg("-c").arg(cmd).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn quoting_round_trips_through_sh_and_fish() {
        let nasty = ["/plain/path.txt", "/a b/c", "/it's", "/back\\slash", "/-rf", "/new\nline", "/$HOME/`x`", "/æøå"];
        let has_fish = std::process::Command::new("fish").arg("-c").arg("true").status().is_ok();
        for n in nasty {
            let q = quote(Path::new(n));
            assert_eq!(sh(&format!("printf %s {q}")), n, "sh: {q}");
            if has_fish {
                let out = std::process::Command::new("fish")
                    .arg("-c")
                    .arg(format!("printf %s {q}"))
                    .output()
                    .unwrap();
                assert_eq!(String::from_utf8(out.stdout).unwrap(), n, "fish: {q}");
            }
        }
    }

    #[test]
    fn guard_rejects_protected_paths() {
        let home = PathBuf::from("/home/someone");
        let mounts = MountTable::parse(
            "57 42 0:35 /@home /home rw - btrfs /dev/x rw\n\
             72 42 0:35 /@cache /var/cache rw - btrfs /dev/x rw\n",
        );
        for p in ["/", "/usr", "/etc", "/home", "/home/someone", "/var/cache"] {
            assert!(protection(Path::new(p), &home, &mounts).is_some(), "{p} should be protected");
        }
        for p in ["/home/someone/code/target", "/var/cache/pacman/pkg"] {
            assert!(protection(Path::new(p), &home, &mounts).is_none(), "{p} should be allowed");
        }
    }

    #[test]
    fn commands_comment_out_protected_and_offer_trash() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("big dir");
        let home = PathBuf::from("/home/someone");
        let out = commands(&[&victim, Path::new("/usr")], &home, &MountTable::default(), false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], format!("rm -rf -- {}", quote(&victim)));
        assert!(lines[1].starts_with("# PROTECTED"));
        assert!(lines[2].starts_with("# or move to Trash instead: gio trash -- "));
    }

    #[test]
    fn delete_removes_trees_and_files_but_not_symlink_targets() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::create_dir_all(d.join("t/sub")).unwrap();
        fs::write(d.join("t/sub/f"), b"x").unwrap();
        fs::create_dir(d.join("keep")).unwrap();
        std::os::unix::fs::symlink(d.join("keep"), d.join("link")).unwrap();
        delete(&d.join("t")).unwrap();
        delete(&d.join("link")).unwrap();
        assert!(!d.join("t").exists());
        assert!(!d.join("link").exists());
        assert!(d.join("keep").is_dir());
    }
}
