//! Main window: header, disk bar, list + treemap, action bar.

mod list;
mod ops;
mod treemap_view;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::disk::{self, human};
use crate::{scan, theme};

pub struct State {
    pub scan: Option<scan::ScanResult>,
    /// Child-index chain from the scan root to the folder being shown.
    pub cwd: Vec<usize>,
    /// Marked paths and their apparent sizes.
    pub marks: BTreeMap<PathBuf, u64>,
    pub mounts: disk::MountTable,
    pub home: PathBuf,
}

impl State {
    pub fn cwd_node(&self) -> Option<&scan::Node> {
        self.scan.as_ref()?.root.at(&self.cwd)
    }

    /// Absolute path of the node at `idx`.
    pub fn path_of(&self, idx: &[usize]) -> Option<PathBuf> {
        let scan = self.scan.as_ref()?;
        let mut path = scan.root_path.clone();
        let mut node = &scan.root;
        for &i in idx {
            node = node.children.get(i)?;
            path.push(&*node.name);
        }
        Some(path)
    }

    pub fn cwd_path(&self) -> Option<PathBuf> {
        self.path_of(&self.cwd)
    }

    /// Resolve an absolute path back to a child-index chain.
    pub fn index_of(&self, path: &Path) -> Option<Vec<usize>> {
        let scan = self.scan.as_ref()?;
        scan.root.find(path.strip_prefix(&scan.root_path).ok()?)
    }

    /// Marked paths without those already covered by a marked ancestor.
    fn effective_marks(&self) -> Vec<(PathBuf, u64)> {
        let mut out: Vec<(PathBuf, u64)> = Vec::new();
        for (p, &s) in &self.marks {
            // BTreeMap order puts ancestors before descendants.
            if !out.iter().any(|(a, _)| p.starts_with(a)) {
                out.push((p.clone(), s));
            }
        }
        out
    }
}

/// What an action operates on.
#[derive(Clone)]
pub struct Target {
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
}

pub struct App {
    pub window: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    progress_label: gtk::Label,
    crumbs: gtk::Box,
    up_button: gtk::Button,
    disk_bar: gtk::DrawingArea,
    disk_label: gtk::Label,
    scan_label: gtk::Label,
    unreadable_button: gtk::MenuButton,
    pub store: gio::ListStore,
    pub selection: gtk::SingleSelection,
    pub view: gtk::ColumnView,
    pub treemap: gtk::DrawingArea,
    target_label: gtk::Label,
    clear_button: gtk::Button,
    pub state: RefCell<State>,
    pub palette: theme::Palette,
    progress: RefCell<Option<Arc<scan::Progress>>>,
    usage: Cell<Option<disk::Usage>>,
    pub has_compsize: bool,
    /// Set while a row's context menu is open, so actions hit that row.
    pub context_target: RefCell<Option<Target>>,
    pub tm: treemap_view::TreemapState,
    /// Every check box the list created, with its list item.
    checks: RefCell<Vec<(glib::WeakRef<gtk::CheckButton>, glib::WeakRef<gtk::ListItem>)>>,
}

pub fn build(application: &adw::Application, root: PathBuf) {
    let palette = theme::apply(&gdk::Display::default().expect("no display"));
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Disk Map")
        .default_width(1280)
        .default_height(800)
        .build();

    // Header: up, rescan | breadcrumb | scan menu.
    let header = adw::HeaderBar::new();
    let up_button = gtk::Button::builder()
        .icon_name("go-up-symbolic")
        .tooltip_text("Up (Backspace)")
        .action_name("win.up")
        .build();
    let rescan = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Rescan (F5)")
        .action_name("win.rescan")
        .build();
    header.pack_start(&up_button);
    header.pack_start(&rescan);
    let scan_menu = gio::Menu::new();
    scan_menu.append(Some("Scan whole disk  ( / )"), Some("win.scan-root"));
    scan_menu.append(Some("Scan home folder"), Some("win.scan-home"));
    scan_menu.append(Some("Scan folder…"), Some("win.scan-choose"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("drive-harddisk-symbolic")
            .tooltip_text("Choose what to scan")
            .menu_model(&scan_menu)
            .build(),
    );
    let crumbs = gtk::Box::builder().css_classes(["breadcrumb"]).spacing(0).build();
    header.set_title_widget(Some(
        &gtk::ScrolledWindow::builder()
            .child(&crumbs)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::External)
            .propagate_natural_width(true)
            .build(),
    ));

    // Disk summary.
    let disk_label = gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["diskbar-label", "heading"]).build();
    let unreadable_button = gtk::MenuButton::builder().css_classes(["flat", "caption"]).visible(false).build();
    let info_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    info_row.append(&disk_label);
    info_row.append(&unreadable_button);
    let disk_bar = gtk::DrawingArea::builder().content_height(10).hexpand(true).build();
    let scan_label = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label", "caption"])
        .build();
    let summary = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(14)
        .margin_end(14)
        .build();
    summary.append(&info_row);
    summary.append(&disk_bar);
    summary.append(&scan_label);

    // List + treemap.
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let selection = gtk::SingleSelection::builder().model(&store).autoselect(true).build();
    let view = gtk::ColumnView::builder()
        .model(&selection)
        .show_row_separators(false)
        .css_classes(["data-table"])
        .build();
    let list_scroll = gtk::ScrolledWindow::builder()
        .child(&view)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let treemap = gtk::DrawingArea::builder().hexpand(true).vexpand(true).has_tooltip(true).build();
    let paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&list_scroll)
        .end_child(&treemap)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .position(620)
        .build();
    // Side by side on a wide window, stacked when tiled narrow.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 1000sp").unwrap());
    narrow.add_setter(&paned, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
    narrow.add_setter(&paned, "position", Some(&420.to_value()));
    window.add_breakpoint(narrow);
    window.set_size_request(360, 480);

    let main_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_page.append(&summary);
    main_page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    main_page.append(&paned);

    // Scanning page.
    let progress_label = gtk::Label::builder().css_classes(["numeric", "title-4"]).build();
    let scanning = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .build();
    scanning.append(&adw::Spinner::builder().width_request(48).height_request(48).build());
    scanning.append(&progress_label);
    scanning.append(
        &gtk::Button::builder()
            .label("Cancel")
            .action_name("win.cancel")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build(),
    );
    let empty = adw::StatusPage::builder()
        .icon_name("drive-harddisk-symbolic")
        .title("Nothing scanned")
        .description("Pick something to scan from the disk menu.")
        .build();

    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&scanning, Some("scanning"));
    stack.add_named(&main_page, Some("main"));
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&stack));

    // Action bar: acts on marked items, or on the selected row if none are marked.
    let target_label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .css_classes(["numeric"])
        .build();
    let clear_button = gtk::Button::builder()
        .label("Clear marks")
        .action_name("win.clear-marks")
        .css_classes(["flat"])
        .visible(false)
        .build();
    let action_bar = gtk::ActionBar::new();
    action_bar.pack_start(&target_label);
    action_bar.pack_start(&clear_button);
    let button = |label: &str, action: &str, tip: &str| {
        gtk::Button::builder().label(label).action_name(action).tooltip_text(tip).build()
    };
    let delete_button = button("Delete…", "win.delete", "Delete permanently (Shift+Del)");
    delete_button.add_css_class("destructive-action");
    action_bar.pack_end(&delete_button);
    action_bar.pack_end(&button("Trash", "win.trash", "Move to Trash (T)"));
    action_bar.pack_end(&button("Copy command", "win.copy", "Copy rm commands to paste in a terminal (C)"));
    action_bar.pack_end(&button("Open", "win.open", "Show in file manager (O)"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&toasts));
    toolbar.add_bottom_bar(&action_bar);
    window.set_content(Some(&toolbar));
    stack.connect_visible_child_name_notify(glib::clone!(
        #[weak]
        action_bar,
        move |s| action_bar.set_revealed(s.visible_child_name().as_deref() == Some("main"))
    ));

    let app = Rc::new(App {
        window,
        toasts,
        stack,
        progress_label,
        crumbs,
        up_button,
        disk_bar,
        disk_label,
        scan_label,
        unreadable_button,
        store,
        selection,
        view,
        treemap,
        target_label,
        clear_button,
        state: RefCell::new(State {
            scan: None,
            cwd: Vec::new(),
            marks: BTreeMap::new(),
            mounts: disk::MountTable::load(),
            home: glib::home_dir(),
        }),
        palette,
        progress: RefCell::new(None),
        usage: Cell::new(None),
        has_compsize: glib::find_program_in_path("compsize").is_some(),
        context_target: RefCell::new(None),
        tm: Default::default(),
        checks: RefCell::new(Vec::new()),
    });

    list::setup(&app);
    treemap_view::setup(&app);
    ops::install_actions(&app);
    install_keys(&app);
    setup_disk_bar(&app);

    let weak = Rc::downgrade(&app);
    app.selection.connect_selected_notify(move |_| {
        if let Some(app) = weak.upgrade() {
            app.update_targets();
            app.treemap.queue_draw();
        }
    });

    // Callbacks hold weak refs; the window owns the one strong ref.
    let keep = app.clone();
    app.window.connect_destroy(move |_| {
        let _ = &keep;
    });

    app.window.present();
    app.start_scan(root);
}

impl App {
    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::builder().title(glib::markup_escape_text(text)).timeout(6).build());
    }

    pub fn start_scan(self: &Rc<Self>, root: PathBuf) {
        if let Some(old) = self.progress.borrow().as_ref() {
            old.cancel.store(true, Ordering::Relaxed);
        }
        let progress = Arc::new(scan::Progress::default());
        *self.progress.borrow_mut() = Some(progress.clone());
        self.progress_label.set_text(&format!("Scanning {}…", root.display()));
        self.stack.set_visible_child_name("scanning");

        let (tx, rx) = async_channel::bounded(1);
        let (p, r) = (progress.clone(), root.clone());
        std::thread::spawn(move || {
            let _ = tx.send_blocking(scan::scan(&r, &p));
        });

        let weak = Rc::downgrade(self);
        let ticker = progress.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(app) = weak.upgrade() else { return glib::ControlFlow::Break };
            if !app.progress.borrow().as_ref().is_some_and(|p| Arc::ptr_eq(p, &ticker)) {
                return glib::ControlFlow::Break;
            }
            let files = ticker.files.load(Ordering::Relaxed);
            let bytes = ticker.bytes.load(Ordering::Relaxed);
            app.progress_label.set_text(&format!("{} files · {}", group(files), human(bytes)));
            glib::ControlFlow::Continue
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            let Some(app) = weak.upgrade() else { return };
            if progress.cancel.load(Ordering::Relaxed) {
                return; // cancelled or superseded; cancel() already switched pages
            }
            *app.progress.borrow_mut() = None;
            {
                let mut st = app.state.borrow_mut();
                st.scan = Some(result);
                st.cwd.clear();
                st.marks.clear();
                st.mounts = disk::MountTable::load();
            }
            app.stack.set_visible_child_name("main");
            app.refresh(Some(0));
            app.view.grab_focus();
        });
    }

    pub fn cancel_scan(&self) {
        if let Some(p) = self.progress.borrow_mut().take() {
            p.cancel.store(true, Ordering::Relaxed);
        }
        let page = if self.state.borrow().scan.is_some() { "main" } else { "empty" };
        self.stack.set_visible_child_name(page);
    }

    /// Rebuild the list for the current folder and select row `select`.
    pub fn refresh(self: &Rc<Self>, select: Option<usize>) {
        let rows: Vec<glib::BoxedAnyObject> = {
            let st = self.state.borrow();
            let (Some(node), Some(base)) = (st.cwd_node(), st.cwd_path()) else { return };
            node.children
                .iter()
                .enumerate()
                .map(|(index, c)| {
                    glib::BoxedAnyObject::new(list::Row {
                        index,
                        name: c.name.to_string(),
                        path: base.join(&*c.name),
                        size: c.size,
                        files: c.files,
                        kind: c.kind,
                        mtime: c.mtime,
                        unreadable: c.unreadable,
                        boundary: c.boundary,
                        frac: if node.size > 0 { c.size as f64 / node.size as f64 } else { 0.0 },
                    })
                })
                .collect()
        };
        self.store.splice(0, self.store.n_items(), &rows);
        self.rebuild_crumbs();
        self.up_button.set_sensitive(!self.state.borrow().cwd.is_empty());
        if let Some(i) = select.filter(|&i| (i as u32) < self.store.n_items()) {
            self.selection.set_selected(i as u32);
            self.view.scroll_to(i as u32, None, gtk::ListScrollFlags::FOCUS, None);
        }
        self.update_summary();
        self.update_targets();
        self.treemap.queue_draw();
    }

    pub fn enter(self: &Rc<Self>, child: usize) {
        {
            let mut st = self.state.borrow_mut();
            let Some(node) = st.cwd_node().and_then(|n| n.children.get(child)) else { return };
            if !node.is_dir() || node.children.is_empty() {
                return;
            }
            st.cwd.push(child);
        }
        self.refresh(Some(0));
    }

    pub fn go_up(self: &Rc<Self>) {
        let from = self.state.borrow_mut().cwd.pop();
        if from.is_some() {
            self.refresh(from);
        }
    }

    fn rebuild_crumbs(self: &Rc<Self>) {
        while let Some(c) = self.crumbs.first_child() {
            self.crumbs.remove(&c);
        }
        let st = self.state.borrow();
        let Some(scan) = &st.scan else { return };
        let root = scan.root_path.display().to_string();
        let home = st.home.display().to_string();
        let root_label = if root == home { "~".to_string() } else { root.replacen(&home, "~", 1) };
        let mut labels = vec![root_label];
        let mut node = &scan.root;
        for &i in &st.cwd {
            node = &node.children[i];
            labels.push(node.name.to_string());
        }
        let depth = labels.len();
        for (d, label) in labels.into_iter().enumerate() {
            if d > 0 {
                self.crumbs.append(&gtk::Label::builder().label("›").css_classes(["dim-label"]).build());
            }
            let b = gtk::Button::builder().label(label).css_classes(["flat"]).build();
            if d + 1 == depth {
                b.add_css_class("heading");
            }
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                let Some(app) = weak.upgrade() else { return };
                let next = {
                    let mut st = app.state.borrow_mut();
                    let next = st.cwd.get(d).copied();
                    st.cwd.truncate(d);
                    next
                };
                app.refresh(next.or(Some(0)));
            });
            self.crumbs.append(&b);
        }
    }

    pub fn update_summary(&self) {
        let st = self.state.borrow();
        let Some(scan) = &st.scan else { return };
        let usage = disk::usage(&scan.root_path);
        self.usage.set(usage);
        let (desc, compressed) = disk::describe(&st.mounts, &scan.root_path);
        if let Some(u) = usage {
            self.disk_label.set_text(&format!(
                "{} used  ·  {} free  ·  {} disk  ({desc})",
                human(u.used),
                human(u.free),
                human(u.total)
            ));
        }
        let cwd_size = st.cwd_node().map(|n| n.size).unwrap_or(0);
        let mut text = format!(
            "Scanned {} in {} files under {}.  This folder: {}.",
            human(scan.root.size),
            group(scan.root.files),
            scan.root_path.display(),
            human(cwd_size)
        );
        if compressed {
            text.push_str(
                "  Sizes are apparent (uncompressed); with compression the space actually freed is usually less — \
                 diskmap measures it after each delete.",
            );
        }
        self.scan_label.set_text(&text);
        self.disk_bar.queue_draw();

        let n = scan.unreadable_count;
        self.unreadable_button.set_visible(n > 0);
        if n > 0 {
            self.unreadable_button.set_label(&format!("⚠ {} unreadable folder{}", group(n), if n == 1 { "" } else { "s" }));
            self.unreadable_button.set_popover(Some(&unreadable_popover(&scan.unreadable, n)));
        }
    }

    /// What the action bar and shortcuts will act on.
    pub fn targets(&self) -> Vec<Target> {
        if let Some(t) = self.context_target.borrow().clone() {
            return vec![t];
        }
        let st = self.state.borrow();
        if !st.marks.is_empty() {
            return st
                .effective_marks()
                .into_iter()
                .map(|(path, size)| Target { is_dir: path.is_dir(), path, size })
                .collect();
        }
        self.selected_row()
            .map(|r| vec![Target { is_dir: r.kind == scan::Kind::Dir, path: r.path, size: r.size }])
            .unwrap_or_default()
    }

    pub fn selected_row(&self) -> Option<list::Row> {
        let obj = self.selection.selected_item()?.downcast::<glib::BoxedAnyObject>().ok()?;
        Some(obj.borrow::<list::Row>().clone())
    }

    pub fn update_targets(&self) {
        let marked = self.state.borrow().effective_marks();
        self.clear_button.set_visible(!marked.is_empty());
        let text = if marked.is_empty() {
            match self.selected_row() {
                Some(r) => format!("{}  ·  {}", r.name, human(r.size)),
                None => String::new(),
            }
        } else {
            let total: u64 = marked.iter().map(|(_, s)| s).sum();
            format!("{} marked  ·  {} apparent", marked.len(), human(total))
        };
        self.target_label.set_text(&text);
        let any = !self.targets().is_empty();
        for name in ["open", "copy", "trash", "delete", "copy-path"] {
            if let Some(a) = self.window.lookup_action(name) {
                a.downcast::<gio::SimpleAction>().unwrap().set_enabled(any);
            }
        }
    }

    pub fn set_mark(&self, path: &Path, size: u64, on: bool) {
        {
            let mut st = self.state.borrow_mut();
            if on {
                st.marks.insert(path.to_path_buf(), size);
            } else {
                st.marks.remove(path);
            }
        }
        self.sync_checks();
        self.update_targets();
        self.treemap.queue_draw();
    }

    pub fn toggle_selected_mark(self: &Rc<Self>) {
        let Some(r) = self.selected_row() else { return };
        let on = !self.state.borrow().marks.contains_key(&r.path);
        self.set_mark(&r.path, r.size, on);
        let next = self.selection.selected() + 1;
        if on && next < self.store.n_items() {
            self.selection.set_selected(next);
            self.view.scroll_to(next, None, gtk::ListScrollFlags::FOCUS, None);
        }
    }
}

fn unreadable_popover(paths: &[PathBuf], total: u64) -> gtk::Popover {
    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();
    bx.append(
        &gtk::Label::builder()
            .label(
                "These folders are not readable by your user, so their size is not counted.\n\
                 Btrfs snapshots under /.snapshots are the usual big one — check them with\n\
                 `sudo snapper list` or Btrfs Assistant.",
            )
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(60)
            .css_classes(["caption"])
            .build(),
    );
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    for p in paths.iter().take(200) {
        list.append(
            &gtk::Label::builder()
                .label(p.display().to_string())
                .xalign(0.0)
                .selectable(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes(["monospace", "caption"])
                .build(),
        );
    }
    if total as usize > paths.len().min(200) {
        list.append(&gtk::Label::builder().label(format!("… and {} more", total as usize - paths.len().min(200))).xalign(0.0).build());
    }
    bx.append(
        &gtk::ScrolledWindow::builder()
            .child(&list)
            .min_content_height(220)
            .max_content_height(360)
            .propagate_natural_height(true)
            .min_content_width(460)
            .build(),
    );
    gtk::Popover::builder().child(&bx).build()
}

fn setup_disk_bar(app: &Rc<App>) {
    let weak = Rc::downgrade(app);
    app.disk_bar.set_draw_func(move |_, cr, w, h| {
        let Some(app) = weak.upgrade() else { return };
        let Some(u) = app.usage.get() else { return };
        let (w, h) = (w as f64, h as f64);
        let r = h / 2.0;
        let pill = |x: f64, width: f64| {
            cr.new_sub_path();
            cr.arc(x + r, r, r, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
            cr.arc(x + width - r, r, r, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            cr.close_path();
        };
        let p = &app.palette;
        cr.set_source_color(&p.free);
        pill(0.0, w);
        let _ = cr.fill();
        let used = if u.total > 0 { (u.used as f64 / u.total as f64).clamp(0.0, 1.0) } else { 0.0 };
        cr.set_source_color(if used > 0.9 { &p.mark } else { &p.accent });
        pill(0.0, (w * used).max(h));
        let _ = cr.fill();
    });
}

fn install_keys(app: &Rc<App>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(app);
    keys.connect_key_pressed(move |_, key, _, mods| {
        let Some(app) = weak.upgrade() else { return glib::Propagation::Proceed };
        if app.window.visible_dialog().is_some() || app.stack.visible_child_name().as_deref() != Some("main") {
            return glib::Propagation::Proceed;
        }
        let plain = !mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK);
        let act = |name: &str| {
            let _ = WidgetExt::activate_action(&app.window, name, None);
        };
        match key {
            gdk::Key::BackSpace | gdk::Key::Left if plain => app.go_up(),
            gdk::Key::Right if plain => {
                if let Some(r) = app.selected_row() {
                    app.enter(r.index);
                }
            }
            gdk::Key::space if plain => app.toggle_selected_mark(),
            gdk::Key::Escape => act("win.clear-marks"),
            gdk::Key::o if plain => act("win.open"),
            gdk::Key::c if plain => act("win.copy"),
            gdk::Key::t if plain => act("win.trash"),
            gdk::Key::Delete | gdk::Key::KP_Delete => act("win.delete"),
            gdk::Key::F5 => act("win.rescan"),
            gdk::Key::r if plain => act("win.rescan"),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    app.window.add_controller(keys);
}

/// 1938530 -> "1 938 530"
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    out
}
