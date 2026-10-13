//! The COMPARE overlay: what both drawings share is dimmed, the differences are drawn over it in
//! the comparison colours (only in this drawing / only in the other one), their areas are
//! outlined, and a bar shows the counts with side by side, previous / next area and close.
//!
//! Side by side (`compare.sidebyside`), the canvas splits: the other drawing to the left with
//! what is only there, this one to the right with what is only here, both with the view of this
//! drawing. Panning and zooming either half moves both; the left half is not edited.

use std::sync::Arc;

use cadcraft_engine::doc::Handle;
use cadcraft_engine::geom::Vec2;
use cadcraft_render::DisplayList;
use egui::{Rect, RichText, Stroke, pos2, vec2};
use serde_json::json;

use super::{Xf, draw_highlight};
use crate::CadApp;
use crate::theme::Tokens;

/// Objects drawn per side; a larger difference shows its areas only.
const MAX_DRAWN: usize = 20_000;

/// The display lists of the differences, rebuilt when the drawing, the comparison or the zoom
/// band changes.
#[derive(Clone, Default)]
struct Cache {
    key: (u64, u64, usize, usize, i64),
    ours: Option<Arc<DisplayList>>,
    theirs: Option<Arc<DisplayList>>,
}

/// The other drawing for the left half: its display list for a zoom band and area, the mesh
/// on the GPU, and the background build of the next one. Built off the UI thread like the
/// canvas's own list (see [`super::rebuild`]); the old list stays on screen until it lands.
#[derive(Default)]
pub(crate) struct Pane {
    key: Option<super::ListKey>,
    window: Option<cadcraft_engine::geom::Bounds2>,
    list: Option<Arc<DisplayList>>,
    slot: crate::gpu::MeshSlot,
    mesh: Option<(u64, Vec2)>,
    job: Option<super::rebuild::Job>,
}

/// Side by side: the left half of `full` for the other drawing and the right half for this one;
/// otherwise all of `full` for this one. Only for a comparison of the model tab shown, without
/// tiled viewports, on a canvas wide enough to split.
pub(super) fn split(app: &CadApp, full: Rect) -> (Option<Rect>, Rect) {
    let Ok(st) = app.session.state() else { return (None, full) };
    let on = app.session.compare.as_ref().is_some_and(|c| c.side_by_side && c.uid == st.uid && c.space == st.space)
        && !st.space.is_paper()
        && st.mspace.is_none()
        && app.session.model_tiles().is_empty()
        && full.width() >= 400.0;
    if !on {
        return (None, full);
    }
    let mid = full.center().x;
    (Some(Rect::from_min_max(full.min, pos2(mid - 1.0, full.max.y))), Rect::from_min_max(pos2(mid + 1.0, full.min.y), full.max))
}

/// The left half's transform: this drawing's view, in `left`.
fn other_xf(xf: &Xf, left: Rect) -> Xf {
    Xf { rect: left, center: xf.center, scale: xf.scale }
}

/// Pan and zoom from the left half: the shared view moves as it would on the right.
pub(super) fn navigate(app: &mut CadApp, ui: &egui::Ui, left: Rect, xf: &Xf) {
    let resp = ui.interact(left, ui.id().with("compare-left"), egui::Sense::drag());
    let Some(hp) = ui.input(|i| i.pointer.hover_pos()).filter(|p| left.contains(*p) && resp.hovered()) else {
        app.canvas.compare_pan = None;
        return;
    };
    let lxf = other_xf(xf, left);
    let scale = xf.scale.max(1e-12);
    let pan = |app: &mut CadApp, d: egui::Vec2| {
        if let Ok(st) = app.session.state_mut() {
            let v = st.view();
            st.set_view_quiet(cadcraft_engine::View {
                center: v.center - Vec2::new(f64::from(d.x) / scale, -f64::from(d.y) / scale),
                height: v.height,
            });
        }
    };
    let scroll = ui.input(|i| i.smooth_scroll_delta);
    if app.canvas.scroll_pans && scroll != egui::Vec2::ZERO {
        pan(app, scroll);
    }
    let opts = ui.ctx().options(|o| o.input_options);
    let wheel = ui.input(|i| super::wheel_zoom_points(&i.raw.events, &opts, left.height()));
    if !app.canvas.scroll_pans && wheel != 0.0 {
        let f = app.session.wheel_zoom_factor(f64::from(wheel));
        let _ = app.session.zoom_about(f, lxf.to_world(hp));
    }
    let pinch = ui.input(|i| crate::animation::zoom_delta(i, &opts, app.shows_animations()));
    if (pinch - 1.0).abs() > 1e-4 {
        let _ = app.session.zoom_about(f64::from(pinch), lxf.to_world(hp));
    }
    // Middle-drag (or any drag) pans.
    if resp.dragged() || ui.input(|i| i.pointer.middle_down()) {
        if let Some(last) = app.canvas.compare_pan {
            pan(app, hp - last);
        }
        app.canvas.compare_pan = Some(hp);
    } else {
        app.canvas.compare_pan = None;
    }
}

fn id() -> egui::Id {
    egui::Id::new("cadcraft-compare-overlay")
}

/// Draw the comparison over the canvas when it was made for the drawing and tab shown.
pub(super) fn paint(app: &mut CadApp, ctx: &egui::Context, p: &egui::Painter, xf: &Xf, rect: Rect, left: Option<Rect>) {
    let t = Tokens::get();
    let Ok(st) = app.session.state() else { return };
    let Some(c) = app.session.compare.as_ref().filter(|c| c.uid == st.uid && c.space == st.space && st.mspace.is_none()) else { return };
    // Dim everything; the differences are drawn on top.
    p.rect_filled(rect, 0.0, t.canvas.gamma_multiply(0.75));
    let band = xf.scale.max(1e-300).log2().round() as i64;
    let key = (st.uid, st.revision, Arc::as_ptr(&c.other) as usize, c.differences(), band);
    let mut cache: Cache = ctx.data_mut(|d| d.get_temp(id())).unwrap_or_default();
    if cache.key != key || cache.ours.is_none() {
        let opts = cadcraft_render::Options { tolerance: 0.5 / xf.scale.max(1e-12), ..Default::default() };
        let ours: Vec<Handle> = c.added.iter().copied().chain(c.changed.iter().map(|(a, _)| *a)).take(MAX_DRAWN).collect();
        let theirs: Vec<Handle> = c.removed.iter().copied().chain(c.changed.iter().map(|(_, b)| *b)).take(MAX_DRAWN).collect();
        let list = |d: &cadcraft_engine::doc::Drawing, hs: &[Handle]| {
            Arc::new(cadcraft_render::build_entities(d, hs.iter().filter_map(|h| d.entity(*h)).map(|e| e.as_ref()), &opts))
        };
        cache = Cache { key, ours: Some(list(&st.doc, &ours)), theirs: Some(list(&c.other, &theirs)) };
        ctx.data_mut(|d| d.insert_temp(id(), cache.clone()));
    }
    // Side by side, what is only there goes on the left; overlaid, both go here.
    let here: &[(&Option<Arc<DisplayList>>, egui::Color32)] =
        if left.is_some() { &[(&cache.ours, t.compare_this)] } else { &[(&cache.theirs, t.compare_other), (&cache.ours, t.compare_this)] };
    for (list, color) in here {
        if let Some(l) = list {
            let all: Vec<usize> = (0..l.prims.len()).collect();
            draw_highlight(p, xf, l, &all, *color, super::Look { width: 2.0, dashed: false });
        }
    }
    let other_name = c.other_name.clone();
    if let Some(left) = left {
        let other = c.other.clone();
        let lp = egui::Painter::new(ctx.clone(), p.layer_id(), left);
        let lxf = other_xf(xf, left);
        paint_other(app, ctx, &lp, &lxf, &other, cache.theirs.as_deref());
        let regions = app.session.compare.as_ref().map(|c| (c.regions.clone(), c.at)).unwrap_or_default();
        outline_areas(&lp, &lxf, left, &regions.0, regions.1);
        pane_label(&lp, left, &other_name);
        pane_label(p, rect, crate::tl!("This drawing"));
    }
    let Some(c) = app.session.compare.as_ref() else { return };
    outline_areas(p, xf, rect, &c.regions, c.at);
    // The comparison bar.
    let (added, removed, changed, name, areas, at, side) =
        (c.added.len(), c.removed.len(), c.changed.len(), c.other_name.clone(), c.regions.len(), c.at, c.side_by_side);
    let mut action = None;
    let mut start = None;
    let top = left.map_or(rect.center_top(), |l| pos2((l.left() + rect.right()) / 2.0, rect.top()));
    egui::Area::new(id().with("bar")).order(egui::Order::Foreground).fixed_pos(top + vec2(-240.0, 8.0)).show(ctx, |ui| {
        egui::Frame::new().fill(t.cmd_bg).stroke(Stroke::new(1.0, t.cmd_border)).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("■").color(t.compare_this));
                ui.label(RichText::new(crate::tf!("Only in this drawing: {n}", n = added)).color(t.text));
                ui.label(RichText::new("■").color(t.compare_other));
                ui.label(RichText::new(crate::tf!("Only in {name}: {n}", name = name, n = removed)).color(t.text));
                ui.label(RichText::new(crate::tf!("Changed: {n}", n = changed)).color(t.text));
                ui.separator();
                if ui.selectable_label(side, crate::tl!("Side by Side")).clicked() {
                    action = Some("compare.sidebyside");
                }
                if areas > 0 {
                    let shown = at.map_or_else(|| "–".to_string(), |i| (i + 1).to_string());
                    ui.label(RichText::new(format!("{shown}/{areas}")).color(t.text_dim));
                }
                if ui.button("◀").on_hover_text(crate::tl!("Previous Change")).clicked() {
                    action = Some("compare.previous");
                }
                if ui.button("▶").on_hover_text(crate::tl!("Next Change")).clicked() {
                    action = Some("compare.next");
                }
                if ui.button(crate::tl!("Export Review...")).on_hover_text(crate::tl!("Write the comparison as an HTML page")).clicked() {
                    start = Some("compare.export");
                }
                if ui.button("×").on_hover_text(crate::tl!("Close Comparison")).clicked() {
                    action = Some("compareclose");
                }
            });
        });
    });
    if let Some(a) = action {
        let _ = app.run(a, json!({}));
    }
    // Asks where to write the page at the command line.
    if let Some(c) = start {
        app.start(c);
    }
}

/// The areas holding differences in `rect`; the one shown last by next/previous stands out.
fn outline_areas(p: &egui::Painter, xf: &Xf, rect: Rect, regions: &[cadcraft_engine::geom::Bounds2], at: Option<usize>) {
    let t = Tokens::get();
    for (i, b) in regions.iter().enumerate() {
        let r = Rect::from_two_pos(xf.to_screen(b.min), xf.to_screen(b.max)).expand(6.0);
        if r.intersects(rect) {
            let w = if at == Some(i) { 2.5 } else { 1.0 };
            p.rect_stroke(r, 4.0, Stroke::new(w, t.compare_area), egui::StrokeKind::Middle);
        }
    }
}

/// The name of the drawing a half shows, at its bottom left.
fn pane_label(p: &egui::Painter, rect: Rect, name: &str) {
    let t = Tokens::get();
    let g = p.layout_no_wrap(name.to_string(), crate::theme::small(), t.text);
    let r = Rect::from_min_size(rect.left_bottom() + vec2(8.0, -g.size().y - 12.0), g.size() + vec2(8.0, 4.0));
    p.rect_filled(r, 3.0, t.cmd_bg);
    p.galley(r.min + vec2(4.0, 2.0), g, t.text);
}

/// The other drawing in the left half, dimmed, with what is only there drawn over it.
fn paint_other(
    app: &mut CadApp,
    ctx: &egui::Context,
    p: &egui::Painter,
    xf: &Xf,
    other: &Arc<cadcraft_engine::doc::Drawing>,
    theirs: Option<&DisplayList>,
) {
    let t = Tokens::get();
    let bg = cadcraft_color::Rgb(t.canvas.r(), t.canvas.g(), t.canvas.b());
    p.rect_filled(xf.rect, 0.0, t.canvas);
    super::draw_grid(app, p, xf);
    let px = 1.0 / xf.scale.max(1e-300);
    let band = px.max(1e-300).log2().floor() as i32;
    let (lwdisplay, transparency) = (app.session.settings.lwdisplay, app.session.settings.transparency_display);
    let visible = xf.world_bounds();
    let view_height = f64::from(xf.rect.height()) / xf.scale.max(1e-300);
    let key: super::ListKey = (0, 0, band, band, lwdisplay, transparency, Arc::as_ptr(other) as usize, 0);
    let gpu = app.canvas.gpu.is_some();
    let device = app.canvas.gpu_device.clone();
    let pane = &mut app.canvas.compare_pane;
    // A finished build replaces the list, unless it is for another drawing.
    if let Some(job) = &pane.job {
        match job.poll() {
            super::rebuild::Poll::Pending => {}
            super::rebuild::Poll::Done(built) => {
                let (job_key, window) = (job.key, job.window);
                pane.job = None;
                if job_key.6 == key.6 {
                    install(pane, job_key, window, *built);
                }
            }
            super::rebuild::Poll::Lost => pane.job = None,
        }
    }
    let current = pane.key == Some(key) && pane.list.is_some() && pane.window.is_none_or(|w| visible.is_empty() || w.contains_box(&visible));
    if !current && !pane.job.as_ref().is_some_and(|j| j.serves(&key, &visible)) {
        pane.job = None;
        let opts = cadcraft_render::Options {
            tolerance: 2f64.powi(band) * 0.5,
            min_dash: 2f64.powi(band) * 2.0,
            text: true,
            fill: true,
            lineweights: lwdisplay,
            view: super::build_window(&[(visible, px)]),
            transparency,
            ..Default::default()
        };
        let window = opts.view;
        let mesh = gpu.then(|| super::rebuild::MeshRequest { view_center: xf.center, view_height, model_bg: bg, paper_bg: bg, lwdisplay, device });
        let req = super::rebuild::Request { doc: other.clone(), space: cadcraft_engine::doc::Space::Model, opts: opts.clone(), mesh };
        match super::rebuild::spawn(req, key, window, ctx) {
            Some(job) => pane.job = Some(job),
            // No threads (the web build): build here.
            None => {
                let list = cadcraft_render::build(other, &cadcraft_engine::doc::Space::Model, &opts);
                pane.list = Some(Arc::new(list));
                pane.key = Some(key);
                pane.window = window;
                pane.mesh = None;
            }
        }
    }
    match &pane.list {
        Some(l) if gpu => {
            let (mesh_key, origin) = match pane.mesh {
                Some(m) => m,
                None => {
                    let origin = crate::gpu::choose_origin(&l.bounds, xf.center, view_height);
                    if let Ok(mut slot) = pane.slot.lock() {
                        *slot = Some(crate::gpu::build_mesh(l, origin, bg, lwdisplay));
                    }
                    let m = (mesh_id(&key, Arc::as_ptr(l) as usize), origin);
                    pane.mesh = Some(m);
                    m
                }
            };
            let center = xf.rect.center();
            p.add(egui_wgpu::Callback::new_paint_callback(
                xf.rect,
                crate::gpu::CanvasCallback {
                    key: mesh_key,
                    slot: pane.slot.clone(),
                    offset: [(origin.x - xf.center.x) as f32, (origin.y - xf.center.y) as f32],
                    scale: xf.scale as f32,
                    center: [center.x, center.y],
                    layer: crate::gpu::LAYER_COMPARE,
                    xform: crate::gpu::IDENTITY_XFORM,
                    dash: 0.0,
                    stencil: crate::gpu::Stencil::Off,
                },
            ));
        }
        Some(l) => super::draw_list(p, xf, l, bg, lwdisplay),
        None => {
            let g = p.layout_no_wrap(crate::tl!("Loading…").to_string(), crate::theme::small(), t.text_dim);
            p.galley(xf.rect.center() - g.size() / 2.0, g, t.text_dim);
        }
    }
    p.rect_filled(xf.rect, 0.0, t.canvas.gamma_multiply(0.75));
    if let Some(l) = theirs {
        let all: Vec<usize> = (0..l.prims.len()).collect();
        draw_highlight(p, xf, l, &all, t.compare_other, super::Look { width: 2.0, dashed: false });
    }
}

/// A background build's list (and mesh) into the pane.
fn install(pane: &mut Pane, key: super::ListKey, window: Option<cadcraft_engine::geom::Bounds2>, built: super::rebuild::Built) {
    let list = Arc::new(built.list);
    pane.mesh = built.mesh.map(|m| {
        if let Ok(mut slot) = pane.slot.lock() {
            *slot = Some(m.data);
        }
        (mesh_id(&key, Arc::as_ptr(&list) as usize), m.origin)
    });
    pane.list = Some(list);
    pane.key = Some(key);
    pane.window = window;
}

/// The id of the mesh made from list `list` (its address) for `key`.
fn mesh_id(key: &super::ListKey, list: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (key, list, "compare").hash(&mut h);
    h.finish()
}
