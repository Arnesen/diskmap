//! Window actions: navigation, scanning, open / copy / trash / delete / compsize.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use super::{App, Target};
use crate::actions;
use crate::disk::{self, human};

fn entry(app: &Rc<App>, name: &str, f: impl Fn(&Rc<App>) + 'static) -> gio::ActionEntry<adw::ApplicationWindow> {
    let weak = Rc::downgrade(app);
    gio::ActionEntry::builder(name)
        .activate(move |_, _, _| {
            if let Some(app) = weak.upgrade() {
                f(&app);
            }
        })
        .build()
}

pub fn install_actions(app: &Rc<App>) {
    let entries = [
        entry(app, "up", |a| a.go_up()),
        entry(app, "cancel", |a| a.cancel_scan()),
        entry(app, "rescan", |a| {
            let root = a.state.borrow().scan.as_ref().map(|s| s.root_path.clone());
            if let Some(root) = root {
                a.start_scan(root);
            }
        }),
        entry(app, "scan-root", |a| a.start_scan(PathBuf::from("/"))),
        entry(app, "scan-home", |a| a.start_scan(glib::home_dir())),
        entry(app, "scan-choose", choose_folder),
        entry(app, "clear-marks", |a| {
            a.state.borrow_mut().marks.clear();
            a.sync_checks();
            a.update_targets();
            a.treemap.queue_draw();
        }),
        entry(app, "open", open),
        entry(app, "copy-path", |a| {
            let Some(t) = a.focused() else { return };
            a.window.clipboard().set_text(&t.path.display().to_string());
            a.toast("Path copied");
        }),
        entry(app, "copy", copy_commands),
        entry(app, "trash", trash),
        entry(app, "delete", delete),
        entry(app, "compsize", compsize),
        entry(app, "toggle-focused", |a| {
            if let Some(t) = a.focused() {
                let on = !a.state.borrow().marks.contains_key(&t.path);
                a.set_mark(&t.path, t.size, on);
            }
        }),
    ];
    app.window.add_action_entries(entries);
}

fn choose_folder(app: &Rc<App>) {
    let dialog = gtk::FileDialog::builder().title("Scan folder").modal(true).build();
    let weak = Rc::downgrade(app);
    dialog.select_folder(Some(&app.window), None::<&gio::Cancellable>, move |res| {
        if let (Some(app), Ok(file)) = (weak.upgrade(), res) {
            if let Some(path) = file.path() {
                app.start_scan(path);
            }
        }
    });
}

fn open(app: &Rc<App>) {
    let Some(t) = app.focused() else { return };
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&t.path)));
    let weak = Rc::downgrade(app);
    let done = move |res: Result<(), glib::Error>| {
        if let (Some(app), Err(e)) = (weak.upgrade(), res) {
            app.toast(&format!("Could not open: {}", e.message()));
        }
    };
    if t.is_dir {
        launcher.launch(Some(&app.window), None::<&gio::Cancellable>, done);
    } else {
        launcher.open_containing_folder(Some(&app.window), None::<&gio::Cancellable>, done);
    }
}

fn copy_commands(app: &Rc<App>) {
    let targets = app.selected();
    if targets.is_empty() {
        return;
    }
    let text = {
        let st = app.state.borrow();
        let paths: Vec<&Path> = targets.iter().map(|t| t.path.as_path()).collect();
        actions::commands(&paths, &st.home, &st.mounts, false)
    };
    app.window.clipboard().set_text(&text);
    let n = targets.len();
    app.toast(&format!("Copied delete command{} for {n} item{} — paste into a terminal", plural(n), plural(n)));
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Drop protected targets, telling the user why.
fn allowed(app: &Rc<App>, targets: Vec<Target>) -> Vec<Target> {
    let st = app.state.borrow();
    let (ok, blocked): (Vec<_>, Vec<_>) =
        targets.into_iter().partition(|t| actions::protection(&t.path, &st.home, &st.mounts).is_none());
    if let Some(b) = blocked.first() {
        let why = actions::protection(&b.path, &st.home, &st.mounts).unwrap_or("");
        drop(st);
        app.toast(&format!("{} is protected ({why}). Use Copy command if you really mean it.", b.path.display()));
    }
    ok
}

fn trash(app: &Rc<App>) {
    trash_list(app, allowed(app, app.selected()));
}

fn trash_list(app: &Rc<App>, targets: Vec<Target>) {
    if targets.is_empty() {
        return;
    }
    let weak = Rc::downgrade(app);
    glib::spawn_future_local(async move {
        let paths: Vec<PathBuf> = targets.iter().map(|t| t.path.clone()).collect();
        let results = gio::spawn_blocking(move || {
            paths
                .into_iter()
                .map(|p| {
                    let r = gio::File::for_path(&p).trash(None::<&gio::Cancellable>).map_err(|e| e.message().to_string());
                    (p, r)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let Some(app) = weak.upgrade() else { return };
        let (done, failed) = finish(&app, &targets, results);
        if done.0 > 0 {
            app.toast(&format!(
                "Moved {} item{} ({}) to Trash — the space is freed when you empty the Trash",
                done.0,
                plural(done.0),
                human(done.1)
            ));
        }
        if !failed.is_empty() {
            report_failures(&app, "move to Trash", failed);
        }
    });
}

/// Delete exactly the selection, straight away. The action bar names what
/// that is; there is deliberately no confirmation dialog.
fn delete(app: &Rc<App>) {
    let targets = allowed(app, app.selected());
    if targets.is_empty() {
        return;
    }
    let app = app.clone();
    glib::spawn_future_local(async move { delete_now(&app, targets).await });
}

async fn delete_now(app: &Rc<App>, targets: Vec<Target>) {
    let root = app.state.borrow().scan.as_ref().map(|s| s.root_path.clone()).unwrap_or_else(|| "/".into());
    let before = disk::usage(&root);
    let toast = adw::Toast::builder().title("Deleting…").timeout(0).build();
    app.toasts.add_toast(toast.clone());

    let paths: Vec<PathBuf> = targets.iter().map(|t| t.path.clone()).collect();
    let root2 = root.clone();
    let results = gio::spawn_blocking(move || {
        let r: Vec<_> = paths.into_iter().map(|p| {
            let res = actions::delete(&p).map_err(|e| e.to_string());
            (p, res)
        }).collect();
        disk::sync_fs(&root2);
        r
    })
    .await
    .unwrap_or_default();
    toast.dismiss();

    let after = disk::usage(&root);
    let (done, failed) = finish(app, &targets, results);
    if done.0 > 0 {
        let freed = match (before, after) {
            (Some(b), Some(a)) => a.free.saturating_sub(b.free),
            _ => 0,
        };
        app.toast(&format!(
            "Deleted {} item{} — freed {} on disk ({} apparent)",
            done.0,
            plural(done.0),
            human(freed),
            human(done.1)
        ));
        // btrfs can keep releasing extents for a little while; refresh the numbers.
        for secs in [5, 20] {
            let weak = Rc::downgrade(app);
            glib::timeout_add_local_once(Duration::from_secs(secs), move || {
                if let Some(app) = weak.upgrade() {
                    app.update_summary();
                }
            });
        }
    }
    if !failed.is_empty() {
        report_failures(app, "delete", failed);
    }
}

/// Drop removed paths from the tree and marks, re-resolve the current folder,
/// refresh. Returns ((count, apparent bytes) done, failures).
fn finish(
    app: &Rc<App>,
    targets: &[Target],
    results: Vec<(PathBuf, Result<(), String>)>,
) -> ((usize, u64), Vec<(PathBuf, String)>) {
    let mut done = (0, 0);
    let mut failed = Vec::new();
    {
        let mut st = app.state.borrow_mut();
        let cwd_path = st.cwd_path();
        for (path, res) in results {
            // A child of an already-removed path counts as done.
            let gone = !path.symlink_metadata().is_ok();
            match res {
                Err(e) if !gone => {
                    failed.push((path, e));
                    continue;
                }
                _ => {}
            }
            done.0 += 1;
            done.1 += targets.iter().find(|t| t.path == path).map(|t| t.size).unwrap_or(0);
            if let Some(idx) = st.index_of(&path) {
                if let Some(scan) = st.scan.as_mut() {
                    scan.root.remove(&idx);
                }
            }
            st.marks.retain(|m, _| !m.starts_with(&path));
        }
        // Walk back up until the folder we were in still exists.
        let mut p = cwd_path;
        st.cwd = loop {
            match p.as_deref().and_then(|p| st.index_of(p)) {
                Some(idx) => break idx,
                None => match p.as_ref().and_then(|x| x.parent().map(Path::to_path_buf)) {
                    Some(parent) => p = Some(parent),
                    None => break Vec::new(),
                },
            }
        };
    }
    *app.context_target.borrow_mut() = None;
    let sel = app.selection.selected() as usize;
    app.refresh(Some(sel.min(app.store.n_items().saturating_sub(1) as usize)));
    (done, failed)
}

fn report_failures(app: &Rc<App>, verb: &str, failed: Vec<(PathBuf, String)>) {
    let n = failed.len();
    let mut body: Vec<String> = failed.iter().take(6).map(|(p, e)| format!("{}: {e}", p.display())).collect();
    if n > 6 {
        body.push(format!("… and {} more", n - 6));
    }
    let dialog = adw::AlertDialog::new(Some(&format!("Couldn't {verb} {n} item{}", plural(n))), Some(&body.join("\n")));
    dialog.add_responses(&[("close", "Close"), ("copy", "Copy sudo command")]);
    dialog.set_response_appearance("copy", adw::ResponseAppearance::Suggested);
    let weak = Rc::downgrade(app);
    dialog.connect_response(None, move |_, resp| {
        let Some(app) = weak.upgrade() else { return };
        if resp == "copy" {
            let st = app.state.borrow();
            let paths: Vec<&Path> = failed.iter().map(|(p, _)| p.as_path()).collect();
            let text = actions::commands(&paths, &st.home, &st.mounts, true);
            drop(st);
            app.window.clipboard().set_text(&text);
            app.toast("Copied — paste into a terminal");
        }
    });
    dialog.present(Some(&app.window));
}

fn compsize(app: &Rc<App>) {
    let Some(t) = app.focused() else { return };
    let weak = Rc::downgrade(app);
    app.toast(&format!("Measuring real size of {}…", t.path.display()));
    glib::spawn_future_local(async move {
        let argv = [std::ffi::OsStr::new("pkexec"), "compsize".as_ref(), "-b".as_ref(), t.path.as_os_str()];
        let result = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE) {
            Ok(p) => p.communicate_utf8_future(None).await.map(|(out, _)| out.map(|s| s.to_string()).unwrap_or_default()),
            Err(e) => Err(e),
        };
        let Some(app) = weak.upgrade() else { return };
        let name = t.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match result.as_deref().ok().and_then(disk::parse_compsize) {
            Some((on_disk, raw)) => app.toasts.add_toast(
                adw::Toast::builder()
                    .title(glib::markup_escape_text(&format!(
                        "{name}: {} on disk ({} uncompressed, {}%)",
                        human(on_disk),
                        human(raw),
                        if raw > 0 { on_disk * 100 / raw } else { 100 }
                    )))
                    .timeout(0)
                    .build(),
            ),
            None => app.toast(&format!("compsize failed for {name}")),
        }
    });
}
