//! Two-level squarified treemap of the current folder.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{cairo, gdk};

use super::App;
use crate::disk::human;
use crate::scan::Node;
use crate::treemap::{Rect, squarify};

const MAX_TILES: usize = 400;
const MAX_INNER: usize = 120;
const HEADER: f64 = 20.0;

#[derive(Clone, Copy, PartialEq)]
struct Hit {
    child: usize,
    inner: Option<usize>,
}

#[derive(Default)]
pub struct TreemapState {
    tiles: RefCell<Vec<(Rect, Hit)>>,
    hover: Cell<Option<Hit>>,
}

impl TreemapState {
    /// Innermost tile under the point (tiles are stored outer before inner).
    fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        self.tiles.borrow().iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| *h)
    }
}

fn biggest(node: &Node, max: usize) -> Vec<f64> {
    node.children.iter().take(max).take_while(|c| c.size > 0).map(|c| c.size as f64).collect()
}

fn with_alpha(c: &gdk::RGBA, a: f32) -> gdk::RGBA {
    gdk::RGBA::new(c.red(), c.green(), c.blue(), a)
}

fn ink_for(bg: &gdk::RGBA) -> gdk::RGBA {
    let lum = 0.2126 * bg.red() + 0.7152 * bg.green() + 0.0722 * bg.blue();
    if lum > 0.55 { gdk::RGBA::new(0.0, 0.0, 0.0, 0.87) } else { gdk::RGBA::new(1.0, 1.0, 1.0, 0.95) }
}

fn label(widget: &gtk::DrawingArea, cr: &cairo::Context, text: &str, r: Rect, bold: bool, ink: &gdk::RGBA) {
    if r.w < 36.0 || r.h < 14.0 {
        return;
    }
    let layout = widget.create_pango_layout(Some(text));
    if bold {
        let attrs = gtk::pango::AttrList::new();
        attrs.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold));
        layout.set_attributes(Some(&attrs));
    }
    layout.set_width(((r.w - 8.0).max(1.0) * gtk::pango::SCALE as f64) as i32);
    layout.set_height(((r.h - 4.0).max(1.0) * gtk::pango::SCALE as f64) as i32);
    layout.set_ellipsize(gtk::pango::EllipsizeMode::End);
    cr.set_source_color(ink);
    cr.move_to(r.x + 4.0, r.y + 2.0);
    pangocairo::functions::show_layout(cr, &layout);
}

fn fill(cr: &cairo::Context, r: Rect, color: &gdk::RGBA) {
    cr.set_source_color(color);
    cr.rectangle(r.x, r.y, r.w, r.h);
    let _ = cr.fill();
}

fn stroke(cr: &cairo::Context, r: Rect, color: &gdk::RGBA, width: f64) {
    cr.set_source_color(color);
    cr.set_line_width(width);
    let h = width / 2.0;
    cr.rectangle(r.x + h, r.y + h, (r.w - width).max(0.0), (r.h - width).max(0.0));
    let _ = cr.stroke();
}

pub fn setup(app: &Rc<App>) {
    let weak = Rc::downgrade(app);
    app.treemap.set_draw_func(move |area, cr, w, h| {
        let Some(app) = weak.upgrade() else { return };
        let st = app.state.borrow();
        let (Some(node), Some(base)) = (st.cwd_node(), st.cwd_path()) else { return };
        let p = &app.palette;
        let selected = app.selected_row().map(|r| r.index);
        let hover = app.tm.hover.get();
        let mut tiles = Vec::new();

        let area_rect = Rect { x: 4.0, y: 4.0, w: w as f64 - 8.0, h: h as f64 - 8.0 };
        let outer = squarify(&biggest(node, MAX_TILES), area_rect);
        for (i, (child, r)) in node.children.iter().zip(outer).enumerate() {
            let r = r.inset(1.0, 1.0);
            if r.w < 1.0 || r.h < 1.0 {
                continue;
            }
            let hit = Hit { child: i, inner: None };
            tiles.push((r, hit));
            let base_color = if child.is_dir() { p.tiles[i % p.tiles.len()] } else { p.file_tile };
            let hovered = hover.is_some_and(|h| h.child == i);
            let nested = child.is_dir() && !child.children.is_empty() && r.w > 60.0 && r.h > 44.0;

            fill(cr, r, &with_alpha(&base_color, if hovered { 1.0 } else { 0.88 }));
            let caption = format!("{}  {}", child.name, human(child.size));
            if nested {
                let head = Rect { h: HEADER, ..r };
                label(area, cr, &caption, head, true, &ink_for(&base_color));
                let body = r.inset(HEADER, 3.0);
                // Darken the body so inner tiles read as contents.
                fill(cr, body, &gdk::RGBA::new(0.0, 0.0, 0.0, 0.35));
                let inner = squarify(&biggest(child, MAX_INNER), body);
                for (j, (g, ir)) in child.children.iter().zip(inner).enumerate() {
                    let ir = ir.inset(1.0, 1.0);
                    if ir.w < 1.0 || ir.h < 1.0 {
                        continue;
                    }
                    let hit = Hit { child: i, inner: Some(j) };
                    tiles.push((ir, hit));
                    // Inner folders get their own hue so the structure reads at a glance.
                    let c = if g.is_dir() { p.tiles[(i + 1 + j) % p.tiles.len()] } else { p.file_tile };
                    let alpha = if hover == Some(hit) { 1.0 } else if g.is_dir() { 0.78 } else { 0.5 };
                    fill(cr, ir, &with_alpha(&c, alpha));
                    if ir.w > 70.0 && ir.h > 34.0 {
                        let text = format!("{}\n{}", g.name, human(g.size));
                        label(area, cr, &text, ir, false, &ink_for(&c));
                    }
                    if st.marks.contains_key(&base.join(&*child.name).join(&*g.name)) {
                        stroke(cr, ir, &p.mark, 3.0);
                    }
                }
            } else if r.w > 50.0 && r.h > 34.0 {
                label(area, cr, &format!("{}\n{}", child.name, human(child.size)), r, true, &ink_for(&base_color));
            }
            if st.marks.contains_key(&base.join(&*child.name)) {
                stroke(cr, r, &p.mark, 3.0);
            }
            if selected == Some(i) {
                stroke(cr, r, &p.text, 2.0);
            }
        }
        drop(st);
        *app.tm.tiles.borrow_mut() = tiles;
    });

    // Hover highlight.
    let motion = gtk::EventControllerMotion::new();
    let weak = Rc::downgrade(app);
    motion.connect_motion(move |_, x, y| {
        let Some(app) = weak.upgrade() else { return };
        let hit = app.tm.hit(x, y);
        if hit != app.tm.hover.replace(hit) {
            app.treemap.queue_draw();
        }
    });
    let weak = Rc::downgrade(app);
    motion.connect_leave(move |_| {
        if let Some(app) = weak.upgrade() {
            app.tm.hover.set(None);
            app.treemap.queue_draw();
        }
    });
    app.treemap.add_controller(motion);

    // Tooltip with the full name and size.
    let weak = Rc::downgrade(app);
    app.treemap.connect_query_tooltip(move |_, x, y, _, tip| {
        let Some(app) = weak.upgrade() else { return false };
        let Some(hit) = app.tm.hit(x as f64, y as f64) else { return false };
        let st = app.state.borrow();
        let Some(child) = st.cwd_node().and_then(|n| n.children.get(hit.child)) else { return false };
        let node = match hit.inner {
            Some(j) => match child.children.get(j) {
                Some(g) => g,
                None => return false,
            },
            None => child,
        };
        let mut text = format!("{}\n{}", node.name, human(node.size));
        if node.is_dir() {
            text.push_str(&format!(" · {} files", super::group(node.files)));
        }
        if hit.inner.is_some() {
            text = format!("{}/{text}", child.name);
        }
        tip.set_text(Some(&text));
        true
    });

    // Click selects the row; double-click drills in (two levels if an inner tile).
    let click = gtk::GestureClick::new();
    let weak = Rc::downgrade(app);
    click.connect_pressed(move |_, n, x, y| {
        let Some(app) = weak.upgrade() else { return };
        let Some(hit) = app.tm.hit(x, y) else { return };
        if n == 1 {
            app.selection.set_selected(hit.child as u32);
            app.view.scroll_to(hit.child as u32, None, gtk::ListScrollFlags::FOCUS, None);
        } else if n == 2 {
            let depth = app.state.borrow().cwd.len();
            app.enter(hit.child);
            let entered = app.state.borrow().cwd.len() > depth;
            if let (true, Some(j)) = (entered, hit.inner) {
                app.select_or_enter(j);
            }
        }
    });
    app.treemap.add_controller(click);
}

impl App {
    /// After drilling into a folder from an inner tile: enter it if it is a
    /// folder, otherwise select it.
    fn select_or_enter(self: &Rc<Self>, j: usize) {
        let is_dir = self.state.borrow().cwd_node().and_then(|n| n.children.get(j)).is_some_and(|c| c.is_dir());
        if is_dir {
            self.enter(j);
        } else {
            self.selection.set_selected(j as u32);
            self.view.scroll_to(j as u32, None, gtk::ListScrollFlags::FOCUS, None);
        }
    }
}
