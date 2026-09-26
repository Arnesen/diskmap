//! Filesystem-level facts: mount table and real (statvfs) usage.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Mount {
    pub point: PathBuf,
    pub source: String,
    pub fstype: String,
    pub options: String,
}

#[derive(Debug, Default)]
pub struct MountTable {
    mounts: Vec<Mount>,
}

impl MountTable {
    pub fn load() -> MountTable {
        std::fs::read_to_string("/proc/self/mountinfo")
            .map(|s| MountTable::parse(&s))
            .unwrap_or_default()
    }

    /// Parse /proc/self/mountinfo. Later lines shadow earlier ones on the same point.
    pub fn parse(text: &str) -> MountTable {
        let mut mounts: Vec<Mount> = Vec::new();
        for line in text.lines() {
            let Some((left, right)) = line.split_once(" - ") else { continue };
            let Some(point) = left.split(' ').nth(4) else { continue };
            let mut r = right.split(' ');
            let (Some(fstype), Some(source)) = (r.next(), r.next()) else { continue };
            let point = PathBuf::from(unescape(point));
            mounts.retain(|m| m.point != point);
            mounts.push(Mount {
                point,
                source: unescape(source),
                fstype: fstype.to_string(),
                options: r.next().unwrap_or("").to_string(),
            });
        }
        MountTable { mounts }
    }

    /// The mount whose mount point is exactly `path`.
    pub fn at(&self, path: &Path) -> Option<&Mount> {
        self.mounts.iter().find(|m| m.point == path)
    }

    /// The mount that contains `path` (longest mount-point prefix).
    pub fn containing(&self, path: &Path) -> Option<&Mount> {
        self.mounts
            .iter()
            .filter(|m| path.starts_with(&m.point))
            .max_by_key(|m| m.point.as_os_str().len())
    }
}

/// mountinfo escapes space, tab, newline and backslash as \ooo.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub total: u64,
    pub used: u64,
    pub free: u64,
}

/// Real usage of the filesystem holding `path`, like `df`.
pub fn usage(path: &Path) -> Option<Usage> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let bs = s.f_frsize as u64;
    Some(Usage {
        total: s.f_blocks as u64 * bs,
        used: (s.f_blocks - s.f_bfree) as u64 * bs,
        free: s.f_bavail as u64 * bs,
    })
}

/// Commit pending btrfs transactions so freed extents show up in statvfs.
pub fn sync_fs(path: &Path) {
    if let Ok(f) = std::fs::File::open(path) {
        use std::os::fd::AsRawFd;
        unsafe { libc::syncfs(f.as_raw_fd()) };
    }
}

/// e.g. "btrfs, zstd:1" for the header, and whether sizes are compressed.
pub fn describe(mounts: &MountTable, path: &Path) -> (String, bool) {
    let Some(m) = mounts.containing(path) else { return (String::new(), false) };
    let compress = m
        .options
        .split(',')
        .find_map(|o| o.strip_prefix("compress=").or_else(|| o.strip_prefix("compress-force=")));
    match compress {
        Some(c) => (format!("{}, {c}", m.fstype), true),
        None => (m.fstype.clone(), false),
    }
}

/// `compsize -b` TOTAL line -> (on-disk bytes, uncompressed bytes).
pub fn parse_compsize(out: &str) -> Option<(u64, u64)> {
    let line = out.lines().find(|l| l.starts_with("TOTAL"))?;
    let f: Vec<&str> = line.split_whitespace().collect();
    Some((f.get(2)?.parse().ok()?, f.get(3)?.parse().ok()?))
}

/// Human size, binary units, du -h style: "312 GiB", "4.2 MiB".
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else if v < 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.0} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
42 1 0:35 /@ / rw,noatime shared:1 - btrfs /dev/mapper/luks-x rw,compress=zstd:1,ssd,subvol=/@
57 42 0:35 /@home /home rw,noatime shared:154 - btrfs /dev/mapper/luks-x rw,compress=zstd:1,ssd,subvol=/@home
50 42 0:24 / /proc rw,nosuid - proc proc rw
218 42 259:1 / /boot rw,relatime - vfat /dev/nvme0n1p1 rw
99 42 0:60 / /mnt/my\\040disk rw - ext4 /dev/sdb1 rw
";

    #[test]
    fn parses_and_resolves_mounts() {
        let t = MountTable::parse(SAMPLE);
        assert_eq!(t.at(Path::new("/home")).unwrap().source, "/dev/mapper/luks-x");
        assert!(t.at(Path::new("/home/txcb")).is_none());
        assert_eq!(t.containing(Path::new("/home/txcb/x")).unwrap().point, Path::new("/home"));
        assert_eq!(t.containing(Path::new("/usr")).unwrap().point, Path::new("/"));
        assert_eq!(t.at(Path::new("/mnt/my disk")).unwrap().fstype, "ext4");
    }

    #[test]
    fn describes_compression() {
        let t = MountTable::parse(SAMPLE);
        assert_eq!(describe(&t, Path::new("/home/a")), ("btrfs, zstd:1".into(), true));
        assert_eq!(describe(&t, Path::new("/boot/x")), ("vfat".into(), false));
    }

    #[test]
    fn parses_compsize() {
        let out = "Processed 3 files, 3 regular extents (3 refs), 0 inline.\n\
                   Type       Perc     Disk Usage   Uncompressed Referenced\n\
                   TOTAL       13%      2273280      16347136     16347136\n\
                   zstd        13%      2273280      16347136     16347136\n";
        assert_eq!(parse_compsize(out), Some((2273280, 16347136)));
        assert_eq!(parse_compsize("ERROR: nothing"), None);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(4404019), "4.2 MiB");
        assert_eq!(human(312 << 30), "312 GiB");
    }
}
