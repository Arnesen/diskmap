# diskmap

A small native GTK4/libadwaita disk-usage triage tool for zflowCachy (CachyOS + Hyprland).
It takes the fast size-sorted drill-down list from ncdu/dua and puts a two-level squarified
treemap next to it (baobab/WizTree). You can act on what you find straight away.

```
diskmap            # scan the whole disk (/), crossing into /home, /var/cache, … subvolumes
diskmap ~/code     # scan one folder
./install.sh       # build + install to ~/.local/bin and the app launcher ("Disk Map")
```

## Using it

| Key | Action |
|---|---|
| Enter / → / double-click | open folder |
| Backspace / ← | up |
| Space | mark / unmark (moves down) |
| Esc | clear marks |
| O | open in file manager |
| C | copy ready-to-paste `rm -rf -- '…'` commands (sh **and** fish safe; `sudo` added where needed) |
| T | move to Trash |
| Delete | delete permanently (confirmation dialog; Cancel is the default) |
| F5 / R | rescan |

Actions apply to the marked items. If nothing is marked, they apply to the selected row.
Right-clicking a row applies them to that row only, and also offers **Copy path** and
**Measure real size** (`pkexec compsize`, shown only if `compsize` is installed).

## Honest numbers on btrfs + zstd

Sizes in the list and treemap are *apparent* (`st_blocks`, i.e. what `du` shows). With
`compress=zstd` they overstate real use, often by about 2x on compressible data. So diskmap:

- shows real used/free from `statvfs` in the header,
- after a permanent delete, calls `syncfs` and reports **the actual `df` delta** ("freed X on disk"),
- can measure a folder's true compressed size with `compsize`.

Trash does not free anything until the Trash is emptied, and the toast says so.

## Safety

The GUI refuses to trash or delete `/`, top-level dirs (`/usr`, `/var`, …), your home or any of
its parents, mount points and btrfs subvolume roots. For those, **Copy command** still emits the
line, commented out with the reason. Failed deletes (root-owned files) offer a
"Copy sudo command" button.

Folders your user cannot read (`/.snapshots`, `/root`, …) are listed under the ⚠ button. Their
size is not counted, so snapper snapshots only show up as the difference between the header's
"used" and what you can see.

## Layout

- `src/scan.rs`: parallel (rayon) walker. It dedupes hardlinks and never follows symlinks.
  It descends into nested btrfs subvolumes and same-device mounts, but not into other filesystems.
- `src/disk.rs`: mountinfo, statvfs, compsize parsing, human sizes.
- `src/treemap.rs`: squarified layout (pure, tested).
- `src/actions.rs`: protection guard, shell quoting, command builder, delete.
- `src/theme.rs`: reads caelestia's `~/.local/state/caelestia/scheme.json` so the app follows
  the wallpaper colours. It falls back to plain libadwaita.
- `src/ui/`: window, list, treemap widget, actions.

`cargo test` runs the unit tests (scanner fixture, layout, quoting through sh and fish, guard).
