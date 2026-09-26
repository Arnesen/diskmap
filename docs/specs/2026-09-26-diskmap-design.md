# diskmap — native SSD usage triage tool for zflowCachy

## Context
The SSD is 80% full (752G used of 950G, one btrfs partition). The user wants a clean, simple, native tool that shows where the space goes and lets them act on it right away. Their choices:
- **GTK4/libadwaita GUI**.
- **Actions:** open in the file manager, trash, **delete permanently**, and **copy a ready-to-run CLI command** (their mid-turn addition).

Facts about this machine that shape the design:
- **Compression.** btrfs uses `compress=zstd:1`, so apparent/`st_blocks` sizes run about 2x higher than real use (see memory `btrfs_zstd_du_overstates`). The tool must say so honestly and measure real space freed from `statvfs`, not from the file sizes.
- **Subvolumes.** `/`, `/home`, `/var/cache`, `/var/log`, `/var/tmp`, `/srv` and `/root` are separate btrfs subvolumes. Each has its own `st_dev`, so a naive "stay on one filesystem" check would stop at `/home`. Subvolumes on the same device have to be recognised as one disk.
- **Snapshots.** snapper is installed. `/.snapshots` is root-only and not readable by the user, so snapshot space appears only as "unaccounted".
- **Available tools.** Rust 1.97, gtk4 4.22, libadwaita 1.9, and caelestia's `~/.local/state/caelestia/scheme.json` (Material colours) for theming. `compsize` is in the repos.

## Inspiration → what we take
- **ncdu / dua**: a fast drill-down list sorted by size with inline bars, keyboard-first, and mark-several-then-act.
- **baobab / WizTree / SpaceSniffer**: a squarified treemap of the current folder next to the list.
- **Filelight**: clickable breadcrumb navigation.
- **Our own addition**: an "unaccounted" bar (df used − scanned). It shows how much space is invisible to the scan, i.e. snapshots, root-only dirs and metadata.

## Stack
Rust + `gtk4` 0.10 + `libadwaita` 0.8 crates, plus `rayon` for the parallel scan. Trashing uses `gio::File::trash` (no extra dependency), and the clipboard uses `gdk::Clipboard`. Rust fits because a scan of millions of files has to be fast, and it keeps everything in one static binary.

Repo: `~/code/diskmap/` (user convention: projects live in `~/code/`).

## Layout (single window, Hyprland-friendly, also touch/pen-usable)
```
┌ HeaderBar: [⟳ Rescan] [root ▾ / | ~]  breadcrumb: / › home › txcb › code      [≡] ┐
│ Disk bar: ██████ scanned 610G ▓▓ unaccounted 142G ░░ free 195G   (950G, zstd)    │
├──────────────────────────────┬────────────────────────────────────────────────────┤
│ List (sorted by size)        │ Treemap of current dir (squarified, colour=depth/  │
│ ☐ ▇▇▇▇▇▇ 312G  code/         │ kind, hover=tooltip, click=select, dbl=enter)      │
│ ☐ ▇▇▇    120G  .cache/       │                                                     │
│ ☐ ▇▇      64G  Games/        │                                                     │
├──────────────────────────────┴────────────────────────────────────────────────────┤
│ Action bar (visible when marked): 3 marked · 188G apparent                         │
│   [Open folder] [Copy command] [Move to Trash] [Delete permanently…]               │
└────────────────────────────────────────────────────────────────────────────────────┘
```
- **Keys:** Enter/→ enter · Backspace/← up · Space mark · `o` open · `c` copy command · `t` trash · Shift+Del delete · `r` rescan.
- The list is a `gtk::ColumnView` backed by `gio::ListStore` (virtualised, so huge folders stay smooth). Each row shows an inline size bar relative to its parent, the item count, and modified time.
- **Theming:** at startup, read `scheme.json` and inject CSS for accent, bar and treemap colours so the app matches caelestia. If the file is missing, fall back to plain libadwaita. It follows dark/light mode via `adw::StyleManager`.

## Modules (`src/`)
- `scan.rs`: parallel walker (rayon) that builds an arena tree `Vec<Node{name, parent, size, count, children}>`.
  - Size is `st_blocks*512`.
  - Hardlinks are de-duplicated with an `(dev, ino)` set.
  - Symlinks are not followed.
  - It descends into subvolumes of the same btrfs filesystem. Same-filesystem is determined by comparing the `statfs` fsid, not `st_dev`.
  - It skips `/proc /sys /dev /run /tmp` and other filesystems.
  - It counts unreadable dirs.
  - It runs on a worker thread and sends progress to the UI with `glib` channels (`async_channel` + `glib::spawn_future_local`).
- `disk.rs`: `statvfs` for total/used/free, plus the compression flag from `/proc/self/mountinfo`.
- `model.rs`: GObject wrapper (`EntryObject`) for list rows, and the navigation stack.
- `treemap.rs`: squarified layout (pure function, unit-tested) drawn in a `gtk::DrawingArea` with cairo, with hit-testing for hover and click.
- `actions.rs`:
  - Open: `gio::AppInfo::launch_default_for_uri` on the parent folder, with the item selected where possible.
  - Trash: `gio::File::trash`.
  - Delete: `std::fs::remove_dir_all` / `remove_file` on a worker thread.
  - Command builder: `rm -rf -- '<path>'` with shell-safe quoting, one line per marked item. It prefixes `sudo` when the parent directory is not writable by the user, and puts `gio trash` in a commented-out alternative line.
  - Safety guard: refuses `/`, `$HOME`, `/home`, top-level system dirs (`/usr /etc /boot /var /bin /lib*`), and anything that is a mount point or subvolume root. For those it offers only the copied command, with a warning comment.
- `ui/window.rs`: window, header, disk bar, list, action bar, keyboard shortcuts.
- `ui/confirm.rs`: `adw::AlertDialog` that lists the marked paths with their apparent total. Delete permanently uses the destructive style and needs a second click. A permission error on any item offers "Copy sudo command" instead.

### After an action
The deleted nodes are removed from the tree in memory, so no rescan is needed. The sizes of their ancestors are updated, and the disk bar is refreshed from `statvfs`. A toast then shows **"Freed X on disk"**, the real `df` delta. Because this is btrfs, it adds "(apparent Y)". Trashed items are shown as "moved to Trash (still on disk)", since trash does not free space.

### Optional precision (small, lazily used)
The context menu has **"Measure real size (compsize)"** for the selected dir. It runs `pkexec compsize -b <path>` and shows the actual compressed size. The button is hidden if `compsize` is not installed; the README says `sudo pacman -S compsize`.

## Packaging / launch
- `cargo build --release`, then install the binary to `~/.local/bin/diskmap`.
- Add `~/.local/share/applications/diskmap.desktop` (Name "Disk Map", icon `drive-harddisk`) so caelestia's launcher finds it.
- `diskmap [PATH]` CLI argument, defaulting to `/`.
- A `Makefile`/`just`-free `install.sh` script.
- A README in the repo, and a short note in the Obsidian vault `~/all/zflowCachy/`, following that memory's convention.

## Process note
Following the brainstorming flow: once this plan is approved, I'll copy it into the repo as `docs/specs/2026-09-26-diskmap-design.md` and commit it, then implement in this order:
1. scan + disk (with tests)
2. treemap layout (with tests)
3. UI list/navigation
4. treemap widget
5. actions + guard (with tests)
6. theming
7. install

## Verification
- **Unit tests** (`cargo test`):
  - Squarified layout: areas proportional, no overlap, total equals the rect.
  - Hardlink de-duplication and symlink skipping, on a tempdir fixture.
  - Command quoting for paths with spaces, quotes, newlines and leading `-`.
  - The safety guard rejects the protected paths.
  - Same-filesystem check across a subvolume.
- **Scan sanity:** run `diskmap ~` and compare `~`'s total with `du -s --block-size=1 ~`; they should match within the hardlink difference. A scan of `/` should cross into `/home` and `/var/cache`.
- **Manual in-app check:**
  - Navigate with the keyboard and the treemap.
  - Mark items in a scratch dir (`~/diskmap-test` with generated files), then test copy-command (paste into a terminal and dry-run review), Trash (appears in `gio trash --list`), and Delete permanently (the toast shows the df delta).
  - Confirm that `/usr` and `~` are refused.
- Launch from the caelestia launcher and confirm the colours match the current scheme, then change the wallpaper/scheme and relaunch.
- Take a screenshot of the running window with `grim` to review the layout.
