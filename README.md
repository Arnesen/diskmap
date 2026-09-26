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

**Select, then act.** Delete, Trash and Copy command act on the selection and nothing
else. The selection is never the highlighted list row and never inferred.

- **Select:** click a tile in the treemap (big tiles or the ones nested inside them), tick
  a row's checkbox, or press Space on a row. Selected tiles are tinted red with a heavy border.
- **See what's selected:** the bottom bar says e.g. `Selected: ~/.cache/huggingface · 309 GiB`
  and the button reads `Delete huggingface` / `Delete 3 items`. Click the "Selected" label for
  the full list, with × to unselect single items.
- **Delete** deletes the selection immediately. There is no confirmation dialog; the button
  label *is* the confirmation.

| Key / mouse | Action |
|---|---|
| click tile | select / unselect that tile |
| double-click tile, Enter, → | open folder |
| Backspace / ← | up |
| Space | select / unselect the highlighted row (moves down) |
| Esc | clear selection |
| C | copy `rm -rf -- '…'` for the selection (sh **and** fish safe; `sudo` where needed) |
| T | move the selection to Trash |
| Delete | delete the selection permanently |
| O | open the highlighted row in the file manager |
| F5 / R | rescan |

Right-clicking a row gives Select, Open, Copy path and **Measure real size**
(`pkexec compsize`, shown only if `compsize` is installed).

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
