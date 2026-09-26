//! The ncdu-style list of the current folder.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::{App, Target};
use crate::disk::human;
use crate::scan::Kind;

#[derive(Clone)]
pub struct Row {
    /// Index among the current folder's children.
    pub index: usize,
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
    pub kind: Kind,
    pub mtime: i64,
    pub unreadable: bool,
    pub boundary: bool,
    /// Share of the current folder.
    pub frac: f64,
}

fn row_of(item: &gtk::ListItem) -> Option<Row> {
    let obj = item.item()?.downcast::<glib::BoxedAnyObject>().ok()?;
    Some(obj.borrow::<Row>().clone())
}

fn column<W: IsA<gtk::Widget>>(
    title: &str,
    expand: bool,
    setup: impl Fn(&gtk::ListItem) -> W + 'static,
    bind: impl Fn(&gtk::ListItem, &W, &Row) + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
        item.set_child(Some(&setup(item)));
    });
    factory.connect_bind(move |_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let (Some(row), Some(child)) = (row_of(item), item.child().and_then(|c| c.downcast::<W>().ok())) else {
            return;
        };
        bind(item, &child, &row);
    });
    gtk::ColumnViewColumn::builder().title(title).factory(&factory).expand(expand).build()
}

pub fn setup(app: &Rc<App>) {
    let weak = Rc::downgrade(app);
    let mark = column(
        "",
        false,
        move |item| {
            let check = gtk::CheckButton::builder().tooltip_text("Mark (Space)").build();
            if let Some(app) = weak.upgrade() {
                app.checks.borrow_mut().push((check.downgrade(), item.downgrade()));
            }
            let (weak, item) = (weak.clone(), item.downgrade());
            check.connect_toggled(move |c| {
                let (Some(app), Some(row)) = (weak.upgrade(), item.upgrade().and_then(|i| row_of(&i))) else { return };
                if app.state.borrow().marks.contains_key(&row.path) != c.is_active() {
                    app.set_mark(&row.path, row.size, c.is_active());
                }
            });
            check
        },
        {
            let weak = Rc::downgrade(app);
            move |_, check: &gtk::CheckButton, row| {
                let Some(app) = weak.upgrade() else { return };
                check.set_active(app.state.borrow().marks.contains_key(&row.path));
            }
        },
    );

    let weak = Rc::downgrade(app);
    let name = column(
        "Name",
        true,
        move |item| {
            let bx = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            bx.append(&gtk::Image::new());
            bx.append(&gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Middle).build());
            let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
            let (weak, item) = (weak.clone(), item.downgrade());
            click.connect_pressed(move |g, _, x, y| {
                let (Some(app), Some(item)) = (weak.upgrade(), item.upgrade()) else { return };
                let Some(row) = row_of(&item) else { return };
                app.selection.set_selected(item.position());
                if let Some(w) = g.widget() {
                    context_menu(&app, &w, &row, x, y);
                }
            });
            bx.add_controller(click);
            bx
        },
        |_, bx: &gtk::Box, row| {
            let icon = bx.first_child().unwrap().downcast::<gtk::Image>().unwrap();
            let label = bx.last_child().unwrap().downcast::<gtk::Label>().unwrap();
            icon.set_icon_name(Some(match (row.kind, row.unreadable) {
                (_, true) => "action-unavailable-symbolic",
                (Kind::Dir, _) if row.boundary => "drive-harddisk-symbolic",
                (Kind::Dir, _) => "folder-symbolic",
                (Kind::File, _) => "text-x-generic-symbolic",
                (Kind::Other, _) => "emblem-symbolic-link-symbolic",
            }));
            let suffix = if row.kind == Kind::Dir { "/" } else { "" };
            label.set_text(&format!("{}{suffix}", row.name));
            let tip = if row.unreadable {
                "Not readable by your user — size unknown".to_string()
            } else if row.boundary {
                "Separate btrfs subvolume or mount".to_string()
            } else {
                row.path.display().to_string()
            };
            bx.set_tooltip_text(Some(&tip));
            if row.unreadable { bx.add_css_class("dim-label") } else { bx.remove_css_class("dim-label") }
        },
    );

    let size = column(
        "Size",
        false,
        |_| {
            let bx = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            bx.append(&gtk::ProgressBar::builder().css_classes(["sizebar"]).valign(gtk::Align::Center).build());
            bx.append(&gtk::Label::builder().xalign(1.0).width_chars(9).css_classes(["numeric"]).build());
            bx
        },
        |_, bx: &gtk::Box, row| {
            let bar = bx.first_child().unwrap().downcast::<gtk::ProgressBar>().unwrap();
            let label = bx.last_child().unwrap().downcast::<gtk::Label>().unwrap();
            bar.set_fraction(row.frac);
            bar.set_tooltip_text(Some(&format!("{:.1}% of this folder", row.frac * 100.0)));
            label.set_text(&if row.unreadable { "?".into() } else { human(row.size) });
        },
    );

    let files = column(
        "Files",
        false,
        |_| gtk::Label::builder().xalign(1.0).width_chars(9).css_classes(["numeric", "dim-label"]).build(),
        |_, label: &gtk::Label, row| {
            label.set_text(&if row.kind == Kind::Dir { super::group(row.files) } else { String::new() })
        },
    );

    let modified = column(
        "Modified",
        false,
        |_| gtk::Label::builder().xalign(0.0).css_classes(["numeric", "dim-label"]).build(),
        |_, label: &gtk::Label, row| {
            let text = glib::DateTime::from_unix_local(row.mtime)
                .and_then(|d| d.format("%Y-%m-%d"))
                .map(|s| s.to_string())
                .unwrap_or_default();
            label.set_text(&text);
        },
    );

    for c in [mark, name, size, files, modified] {
        app.view.append_column(&c);
    }

    let weak = Rc::downgrade(app);
    app.view.connect_activate(move |_, pos| {
        let Some(app) = weak.upgrade() else { return };
        app.selection.set_selected(pos);
        if let Some(r) = app.selected_row() {
            app.enter(r.index);
        }
    });
}

impl App {
    /// Make every visible check box reflect the marks.
    pub fn sync_checks(&self) {
        let marks = &self.state.borrow().marks;
        self.checks.borrow_mut().retain(|(check, item)| {
            let (Some(check), Some(item)) = (check.upgrade(), item.upgrade()) else { return false };
            if let Some(row) = row_of(&item) {
                check.set_active(marks.contains_key(&row.path));
            }
            true
        });
    }
}

fn context_menu(app: &Rc<App>, widget: &gtk::Widget, row: &Row, x: f64, y: f64) {
    *app.context_target.borrow_mut() =
        Some(Target { path: row.path.clone(), size: row.size, is_dir: row.kind == Kind::Dir });
    app.update_targets();

    let menu = gio::Menu::new();
    let top = gio::Menu::new();
    top.append(Some("Open in file manager"), Some("win.open"));
    top.append(Some("Copy path"), Some("win.copy-path"));
    top.append(Some("Copy delete command"), Some("win.copy"));
    if app.has_compsize && row.kind == Kind::Dir {
        top.append(Some("Measure real size (compsize)"), Some("win.compsize"));
    }
    menu.append_section(None, &top);
    let danger = gio::Menu::new();
    danger.append(Some("Move to Trash"), Some("win.trash"));
    danger.append(Some("Delete permanently…"), Some("win.delete"));
    menu.append_section(None, &danger);

    let popover = gtk::PopoverMenu::builder().menu_model(&menu).has_arrow(false).build();
    popover.set_parent(widget);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let weak = Rc::downgrade(app);
    popover.connect_closed(move |p| {
        let p = p.clone();
        let weak = weak.clone();
        // Actions fire after "closed"; clear the override once they have run.
        glib::idle_add_local_once(move || {
            p.unparent();
            if let Some(app) = weak.upgrade() {
                app.context_target.borrow_mut().take();
                app.update_targets();
            }
        });
    });
    popover.popup();
}
