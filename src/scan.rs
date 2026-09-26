//! Parallel directory scanner that builds an owned size tree.
//!
//! Sizes are allocated bytes (`st_blocks * 512`), i.e. what `du` reports. On a
//! compressed btrfs this is the *uncompressed* allocation, so it overstates real
//! disk use; the UI says so and measures real frees with statvfs instead.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

use crate::disk::MountTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Other,
}

#[derive(Debug)]
pub struct Node {
    pub name: Box<str>,
    /// Allocated bytes, including all descendants.
    pub size: u64,
    /// Number of files (non-directories), including all descendants.
    pub files: u64,
    pub mtime: i64,
    pub kind: Kind,
    /// Directory could not be read (permission denied etc.).
    pub unreadable: bool,
    /// Directory is the root of another btrfs subvolume or a mount point.
    pub boundary: bool,
    /// Sorted by size, largest first.
    pub children: Vec<Node>,
}

impl Node {
    fn leaf(name: Box<str>, kind: Kind, size: u64, mtime: i64) -> Node {
        Node {
            name,
            size,
            files: u64::from(kind != Kind::Dir),
            mtime,
            kind,
            unreadable: false,
            boundary: false,
            children: Vec::new(),
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    /// Follow a chain of child indices.
    pub fn at(&self, idx: &[usize]) -> Option<&Node> {
        idx.iter().try_fold(self, |n, &i| n.children.get(i))
    }

    /// Follow a chain of child names, returning the index chain.
    pub fn find(&self, rel: &Path) -> Option<Vec<usize>> {
        let mut node = self;
        let mut out = Vec::new();
        for comp in rel.components() {
            let name = comp.as_os_str().to_str()?;
            let i = node.children.iter().position(|c| &*c.name == name)?;
            out.push(i);
            node = &node.children[i];
        }
        Some(out)
    }

    fn at_mut(&mut self, idx: &[usize]) -> Option<&mut Node> {
        idx.iter().try_fold(self, |n, &i| n.children.get_mut(i))
    }

    /// Remove the node at `idx`, subtract its totals from every ancestor and
    /// re-sort the ancestors' siblings (sizes changed). Indices below the
    /// root may shift afterwards, so re-resolve paths with `find`.
    pub fn remove(&mut self, idx: &[usize]) -> Option<Node> {
        let (&last, parents) = idx.split_last()?;
        let target = self.at(idx)?;
        let (size, files) = (target.size, target.files);
        for d in 0..=parents.len() {
            let n = self.at_mut(&parents[..d])?;
            n.size -= size;
            n.files -= files;
        }
        let removed = self.at_mut(parents)?.children.remove(last);
        for d in (0..=parents.len()).rev() {
            self.at_mut(&parents[..d])?.sort_children();
        }
        Some(removed)
    }

    fn sort_children(&mut self) {
        self.children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    }
}

/// Live counters the UI polls while a scan runs.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
}

pub struct ScanResult {
    pub root_path: PathBuf,
    pub root: Node,
    /// Directories we could not read (capped), e.g. /.snapshots.
    pub unreadable: Vec<PathBuf>,
    pub unreadable_count: u64,
}

const UNREADABLE_CAP: usize = 500;

struct Ctx<'a> {
    progress: &'a Progress,
    mounts: &'a MountTable,
    root_source: Option<String>,
    hardlinks: Mutex<HashSet<(u64, u64)>>,
    unreadable: Mutex<(Vec<PathBuf>, u64)>,
}

pub fn scan(root: &Path, progress: &Progress) -> ScanResult {
    let mounts = MountTable::load();
    let ctx = Ctx {
        progress,
        root_source: mounts.containing(root).map(|m| m.source.clone()),
        mounts: &mounts,
        hardlinks: Mutex::new(HashSet::new()),
        unreadable: Mutex::new((Vec::new(), 0)),
    };
    let meta = fs::symlink_metadata(root);
    let mtime = meta.as_ref().map(|m| m.mtime()).unwrap_or(0);
    let dev = meta.as_ref().map(|m| m.dev()).unwrap_or(0);
    let mut node = scan_dir(root, root.display().to_string().into(), mtime, dev, &ctx);
    node.boundary = false;
    let (unreadable, unreadable_count) = ctx.unreadable.into_inner().unwrap();
    ScanResult {
        root_path: root.to_path_buf(),
        root: node,
        unreadable,
        unreadable_count,
    }
}

/// Should the walk descend into `path`, whose device differs from its parent's
/// or which is a mount point? Nested btrfs subvolumes get a fresh `st_dev` but
/// never appear in mountinfo, so "not a mount point" means "same filesystem".
fn same_disk(path: &Path, ctx: &Ctx) -> bool {
    match ctx.mounts.at(path) {
        None => true,
        Some(m) => Some(&m.source) == ctx.root_source.as_ref() && m.fstype != "tmpfs",
    }
}

fn scan_dir(path: &Path, name: Box<str>, mtime: i64, dev: u64, ctx: &Ctx) -> Node {
    let mut node = Node::leaf(name, Kind::Dir, 0, mtime);
    if ctx.progress.cancel.load(Ordering::Relaxed) {
        return node;
    }
    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => {
            node.unreadable = true;
            let mut u = ctx.unreadable.lock().unwrap();
            u.1 += 1;
            if u.0.len() < UNREADABLE_CAP {
                u.0.push(path.to_path_buf());
            }
            return node;
        }
    };

    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue }; // lstat: never follows symlinks
        let name: Box<str> = entry.file_name().to_string_lossy().into();
        if meta.is_dir() {
            let child = entry.path();
            let crossed = meta.dev() != dev || ctx.mounts.at(&child).is_some();
            if crossed && !same_disk(&child, ctx) {
                continue;
            }
            subdirs.push((child, name, meta.mtime(), meta.dev(), crossed));
            continue;
        }
        let kind = if meta.is_file() { Kind::File } else { Kind::Other };
        let mut size = meta.blocks() * 512;
        if meta.nlink() > 1 && !ctx.hardlinks.lock().unwrap().insert((meta.dev(), meta.ino())) {
            size = 0; // already counted via another link
        }
        ctx.progress.files.fetch_add(1, Ordering::Relaxed);
        ctx.progress.bytes.fetch_add(size, Ordering::Relaxed);
        node.children.push(Node::leaf(name, kind, size, meta.mtime()));
    }

    let dirs: Vec<Node> = subdirs
        .into_par_iter()
        .map(|(p, n, m, d, crossed)| {
            let mut child = scan_dir(&p, n, m, d, ctx);
            child.boundary = crossed;
            child
        })
        .collect();
    node.children.extend(dirs);
    node.sort_children();
    node.size = node.children.iter().map(|c| c.size).sum();
    node.files = node.children.iter().map(|c| c.files).sum();
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, bytes: usize) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(&vec![7u8; bytes]).unwrap();
        f.sync_all().unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::create_dir_all(d.join("big/inner")).unwrap();
        fs::create_dir_all(d.join("small")).unwrap();
        write(&d.join("big/inner/a.bin"), 1 << 20);
        write(&d.join("big/b.bin"), 256 << 10);
        write(&d.join("small/c.txt"), 10);
        fs::hard_link(d.join("big/b.bin"), d.join("small/b-link.bin")).unwrap();
        std::os::unix::fs::symlink(d.join("big"), d.join("small/loop")).unwrap();
        dir
    }

    #[test]
    fn sizes_roll_up_and_sort() {
        let dir = fixture();
        let r = scan(dir.path(), &Progress::default());
        assert_eq!(&*r.root.children[0].name, "big");
        let big = &r.root.children[0];
        assert!(big.size >= 1 << 20);
        assert_eq!(r.root.size, r.root.children.iter().map(|c| c.size).sum::<u64>());
        assert_eq!(r.root.files, 5); // a, b, c, b-link, loop
    }

    #[test]
    fn hardlinks_counted_once_and_symlinks_not_followed() {
        let dir = fixture();
        let r = scan(dir.path(), &Progress::default());
        // b.bin and small/b-link.bin share an inode: whichever is seen first carries it.
        assert!(r.root.size < (1 << 20) + 2 * (256 << 10), "root = {}", r.root.size);
        let small = r.root.at(&r.root.find(Path::new("small")).unwrap()).unwrap();
        let lp = small.children.iter().find(|c| &*c.name == "loop").unwrap();
        assert_eq!(lp.kind, Kind::Other);
        assert!(lp.children.is_empty());
    }

    #[test]
    fn remove_updates_ancestors() {
        let dir = fixture();
        let mut r = scan(dir.path(), &Progress::default());
        let before = r.root.size;
        let idx = r.root.find(Path::new("big/inner/a.bin")).unwrap();
        let removed = r.root.remove(&idx).unwrap();
        assert_eq!(r.root.size, before - removed.size);
        let inner = r.root.at(&r.root.find(Path::new("big/inner")).unwrap()).unwrap();
        assert_eq!(inner.size, 0);
        assert_eq!(inner.files, 0);
    }

    #[test]
    fn remove_keeps_siblings_sorted() {
        let dir = fixture();
        let mut r = scan(dir.path(), &Progress::default());
        assert_eq!(&*r.root.children[0].name, "big");
        let idx = r.root.find(Path::new("big/inner")).unwrap();
        r.root.remove(&idx);
        let sizes: Vec<u64> = r.root.children.iter().map(|c| c.size).collect();
        assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "{sizes:?}");
    }

    #[test]
    fn unreadable_dirs_reported() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = fixture();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let r = scan(dir.path(), &Progress::default());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r.unreadable_count, 1);
        assert_eq!(r.unreadable, vec![locked]);
    }
}
