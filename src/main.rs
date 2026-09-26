mod actions;
mod disk;
mod scan;
mod theme;
mod treemap;
mod ui;

use std::path::PathBuf;

use adw::prelude::*;

fn main() -> gtk::glib::ExitCode {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_else(|| PathBuf::from("/"));
    let app = adw::Application::builder()
        .application_id("dev.txcb.DiskMap")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| ui::build(app, root.clone()));
    // Arguments are ours, not GApplication's.
    app.run_with_args::<&str>(&[])
}
