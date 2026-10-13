//! COMPARE as a review page: one self-contained HTML file (inline SVG and CSS, no scripts) to
//! attach to a pull request, publish as a CI artifact or mail. It holds the counts, an overview
//! of where the drawing changed, both versions side by side, a close-up of every area with
//! changes, and the list of changed objects. It opens from disk in any browser; zoom is the
//! browser's (the drawings are vector).

use std::collections::HashSet;
use std::fmt::Write as _;

use cadcraft_doc::{Drawing, Handle, Space, entity_bounds};
use cadcraft_geom::{Bounds2, Vec2};
use cadcraft_render::Kind;
use serde_json::{Value, json};

use super::super::*;
use super::compare::Comparison;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Areas shown close up; the rest are listed only.
const MAX_AREAS: usize = 40;
/// Objects listed per area and in the full list.
const MAX_LISTED: usize = 500;
/// Width of each picture in CSS pixels; its level of detail follows.
const PICTURE_PX: f64 = 800.0;

const RED: &str = "#d33a2c";
const GREEN: &str = "#16a34a";
const AREA: &str = "#f59e0b";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("compare.export", "Export Review...", run_export)
            .params(
                "{path?: .html (default: the drawing's name + `-review.html`)} → {path, bytes, areas}: the comparison as a self-contained HTML page",
            )
            .noundo()
            .interactive(|s| Ok(Box::new(ExportM { default: default_path(s) }))),
    ]
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "compare.export";
    let st = s.state()?;
    let c = s.compare.as_ref().filter(|c| c.uid == st.uid).ok_or_else(|| bad(CMD, "no comparison: run COMPARE first"))?;
    let path = str_param(p, "path").map(str::to_string).unwrap_or_else(|| default_path(s));
    let html = page(&st.doc, &st.title, c);
    write_file(CMD, &path, html.as_bytes())?;
    Ok(json!({ "path": path, "bytes": html.len(), "areas": merged(&c.regions, floor(&st.doc, &c.other)).len().min(MAX_AREAS) }))
}

/// `<drawing>-review.html` next to the drawing (or in the current folder for a new one).
fn default_path(s: &Session) -> String {
    let Ok(st) = s.state() else { return "review.html".into() };
    let base = st.path.clone().unwrap_or_else(|| st.title.clone());
    let stem = base.rsplit_once('.').map_or(base.as_str(), |(a, _)| a).to_string();
    format!("{stem}-review.html")
}

fn write_file(cmd: &str, path: &str, bytes: &[u8]) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::write(path, bytes).map_err(|e| bad(cmd, format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let deliver = super::deliver().ok_or_else(|| bad(cmd, "files can't be written in this build"))?;
        deliver(name, "text/html", bytes).map_err(|e| bad(cmd, format!("{name}: {e}")))
    }
}

/// Export Review at the command line: where to write the page.
struct ExportM {
    default: String,
}

impl Interactive for ExportM {
    fn name(&self) -> &'static str {
        "COMPARE.EXPORT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Enter the review page to write", Accept::TEXT).default(self.default.clone())
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let path = match i {
            Input::Text(t) if !t.trim().is_empty() => t.trim().trim_matches('"').to_string(),
            Input::Text(_) | Input::Enter => self.default.clone(),
            _ => return Ok(Step::Done),
        };
        let r = run_export(s, &json!({ "path": path }))?;
        s.echo(format!("Review written to {path} ({} KB).", r["bytes"].as_u64().unwrap_or(0) / 1024));
        Ok(Step::Done)
    }
}

/// HTML-escaped `t`.
fn esc(t: &str) -> String {
    let mut o = String::with_capacity(t.len());
    for ch in t.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c if c.is_control() => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// The review page of comparison `c` of drawing `cur` (titled `title`).
pub fn page(cur: &Drawing, title: &str, c: &Comparison) -> String {
    let ours: HashSet<Handle> = c.added.iter().copied().chain(c.changed.iter().map(|(a, _)| *a)).collect();
    let theirs: HashSet<Handle> = c.removed.iter().copied().chain(c.changed.iter().map(|(_, b)| *b)).collect();
    let all_areas = merged(&c.regions, floor(cur, &c.other));
    let areas: Vec<Bounds2> = all_areas.iter().take(MAX_AREAS).copied().collect();
    let other = &c.other;
    let mut h = String::new();
    let (name, other_name) = (esc(title), esc(&c.other_name));
    let _ = write!(
        h,
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Review: {name} vs {other_name}</title><style>
:root{{color-scheme:light;--fg:#1f2328;--dim:#59636e;--line:#d1d9e0;--bg:#f6f8fa}}
body{{margin:0;font:14px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;color:var(--fg);background:#fff}}
main{{max-width:1680px;margin:0 auto;padding:16px}}
h1{{font-size:22px;margin:8px 0}} h2{{font-size:17px;margin:28px 0 8px;border-bottom:1px solid var(--line);padding-bottom:4px}}
.counts span{{display:inline-block;margin-right:18px}} .sw{{display:inline-block;width:10px;height:10px;border-radius:2px;margin-right:6px;vertical-align:baseline}}
.pair{{display:grid;grid-template-columns:1fr 1fr;gap:12px}} @media(max-width:900px){{.pair{{grid-template-columns:1fr}}}}
figure{{margin:0;border:1px solid var(--line);border-radius:6px;overflow:hidden;background:#fff}}
figcaption{{font-size:12px;color:var(--dim);padding:4px 8px;background:var(--bg);border-bottom:1px solid var(--line)}}
svg{{display:block;width:100%;height:auto}}
table{{border-collapse:collapse;width:100%;font-size:13px}} th,td{{text-align:left;padding:3px 8px;border-bottom:1px solid var(--line)}} th{{background:var(--bg)}}
code{{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12px}} .dim{{color:var(--dim)}}
.add{{color:{GREEN}}} .rem{{color:{RED}}}
</style></head><body><main>
<h1>{name} <span class="dim">compared with</span> {other_name}</h1>
<p class="counts"><span><i class="sw" style="background:{GREEN}"></i>Only in {name}: <b>{}</b></span><span><i class="sw" style="background:{RED}"></i>Only in {other_name}: <b>{}</b></span><span>Changed: <b>{}</b></span><span class="dim">Unchanged: {}</span></p>"#,
        c.added.len(),
        c.removed.len(),
        c.changed.len(),
        c.common
    );
    if c.truncated {
        h.push_str(r#"<p class="dim">The drawings are too large to compare completely: only their first objects were compared.</p>"#);
    }
    if c.differences() == 0 {
        h.push_str("<p>No differences.</p></main></body></html>");
        return h;
    }
    // Overview: every area with changes, numbered, with some of the drawing around them; then a
    // close-up of each. Each drawing is drawn once, as finely as the closest close-up needs, and
    // every picture takes what it shows from that.
    let all = areas.iter().fold(Bounds2::EMPTY, |a, b| a.union(b));
    let overview = framed(all, cur, other, 0.15);
    let closeups: Vec<Bounds2> = areas.iter().map(|a| framed(*a, cur, other, 0.6)).collect();
    let finest = closeups.iter().map(Bounds2::width).fold(overview.width(), f64::min) / PICTURE_PX;
    let reach = closeups.iter().fold(overview, |a, b| a.union(b));
    let (theirs_list, ours_list) = both(|| drawn(other, &c.space, reach, finest), || drawn(cur, &c.space, reach, finest));
    h.push_str("<h2>Overview</h2><div class=\"pair\">");
    figure(&mut h, &theirs_list, overview, &theirs, RED, &areas, &format!("{other_name} (before)"));
    figure(&mut h, &ours_list, overview, &ours, GREEN, &areas, &format!("{name} (after)"));
    h.push_str("</div>");
    for (i, (a, w)) in areas.iter().zip(&closeups).enumerate() {
        let _ = write!(h, "<h2>Area {}</h2>", i + 1);
        let rows = listed(cur, other, c, Some(*a));
        changes_table(&mut h, &rows);
        h.push_str("<div class=\"pair\">");
        figure(&mut h, &theirs_list, *w, &theirs, RED, &[], &format!("{other_name} (before)"));
        figure(&mut h, &ours_list, *w, &ours, GREEN, &[], &format!("{name} (after)"));
        h.push_str("</div>");
    }
    if all_areas.len() > areas.len() {
        let _ = write!(h, "<p class=\"dim\">{} more areas with changes are not shown close up.</p>", all_areas.len() - areas.len());
    }
    h.push_str("<h2>All changes</h2>");
    changes_table(&mut h, &listed(cur, other, c, None));
    let _ = write!(h, r#"<p class="dim">Made by CADCraft COMPARE. Space: {}.</p></main></body></html>"#, esc(&space_name(&c.space)));
    h
}

/// The areas with changes, those that would share a close-up (an object moved a little: its
/// old and new place) merged, in reading order. COMPARE's areas are few (a grid of at most
/// 32 × 32 cells), so this stays cheap.
fn merged(regions: &[Bounds2], floor: f64) -> Vec<Bounds2> {
    let mut out: Vec<Bounds2> = Vec::new();
    for r in regions {
        let near = |g: &Bounds2| g.expand(g.width().max(g.height()).max(r.width().max(r.height())).max(floor) * 0.6).intersects(r);
        match out.iter_mut().find(|g| near(g)) {
            Some(g) => *g = g.union(r),
            None => out.push(*r),
        }
    }
    out.sort_by(|a, b| b.max.y.total_cmp(&a.max.y).then(a.min.x.total_cmp(&b.min.x)));
    out
}

fn space_name(s: &Space) -> String {
    match s {
        Space::Model => "Model".into(),
        Space::Paper(n) => format!("layout {n}"),
    }
}

/// One row of a change list: kind ("added", "removed", "changed"), object type, handle, layer, what changed.
type Row = (&'static str, String, String, String, String);

/// The changed objects, those within `area` when given.
fn listed(cur: &Drawing, other: &Drawing, c: &Comparison, area: Option<Bounds2>) -> Vec<Row> {
    let inside = |d: &Drawing, h: Handle| {
        area.is_none_or(|a| {
            d.entity(h).is_some_and(|e| {
                let b = entity_bounds(d, e, 0);
                !b.is_empty() && b.intersects(&a)
            })
        })
    };
    let row = |kind: &'static str, d: &Drawing, h: Handle, what: &str| -> Option<Row> {
        let e = d.entity(h)?;
        Some((kind, e.kind.type_name().to_string(), h.hex(), e.common.layer.clone(), what.to_string()))
    };
    let mut rows: Vec<Row> = Vec::new();
    rows.extend(c.added.iter().filter(|h| inside(cur, **h)).filter_map(|h| row("added", cur, *h, "")));
    rows.extend(c.removed.iter().filter(|h| inside(other, **h)).filter_map(|h| row("removed", other, *h, "")));
    rows.extend(c.changed.iter().filter(|(a, b)| inside(cur, *a) || inside(other, *b)).filter_map(|(a, b)| {
        let (x, y) = (cur.entity(*a)?, other.entity(*b)?);
        let what = super::compare::what_changed(x, y);
        row("changed", cur, *a, what)
    }));
    rows.truncate(MAX_LISTED);
    rows
}

fn changes_table(h: &mut String, rows: &[Row]) {
    if rows.is_empty() {
        return;
    }
    h.push_str("<table><tr><th></th><th>Object</th><th>Handle</th><th>Layer</th><th>What changed</th></tr>");
    for (kind, ty, handle, layer, what) in rows {
        let (class, sign) = match *kind {
            "added" => ("add", "+"),
            "removed" => ("rem", "−"),
            _ => ("", "~"),
        };
        let _ = write!(
            h,
            r#"<tr><td class="{class}"><b>{sign}</b></td><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td></tr>"#,
            esc(ty),
            esc(handle),
            esc(layer),
            esc(what)
        );
    }
    h.push_str("</table>");
}

/// The smallest area a close-up shows: a 200th of both drawings' extents.
fn floor(cur: &Drawing, other: &Drawing) -> f64 {
    let ext = cur.extents(&Space::Model).union(&other.extents(&Space::Model));
    if ext.is_empty() { 1.0 } else { ext.width().max(ext.height()) / 200.0 }
}

/// `b` with a margin of `pad` of its size (and at least a sliver of both drawings' extents, so a
/// tiny change keeps some surroundings), widened to 3:2.
fn framed(b: Bounds2, cur: &Drawing, other: &Drawing, pad: f64) -> Bounds2 {
    let ext = cur.extents(&Space::Model).union(&other.extents(&Space::Model));
    let size = b.width().max(b.height()).max(floor(cur, other));
    let b = if b.is_empty() { ext } else { b.expand(size * pad) };
    if b.is_empty() {
        return Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0));
    }
    let (w, ht) = (b.width().max(1e-9), b.height().max(1e-9));
    let c = b.center();
    let (w, ht) = if w / ht < 1.5 { (ht * 1.5, ht) } else { (w, w / 1.5) };
    Bounds2::new(Vec2::new(c.x - w / 2.0, c.y - ht / 2.0), Vec2::new(c.x + w / 2.0, c.y + ht / 2.0))
}

/// `a` and `b`, at the same time where there are threads. A worker that fails gives the default
/// (an empty picture) rather than taking the session down.
fn both<A: Send + Default, B: Send>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send) -> (A, B) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::scope(|sc| {
            let ta = sc.spawn(a);
            let rb = b();
            (ta.join().unwrap_or_default(), rb)
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        (a(), b())
    }
}

/// Drawing `d` within `reach`, curves within half of `px` (drawing units per picture pixel).
fn drawn(d: &Drawing, space: &Space, reach: Bounds2, px: f64) -> cadcraft_render::DisplayList {
    let opts = cadcraft_render::Options {
        tolerance: px * 0.5,
        min_dash: px * 2.0,
        text: true,
        fill: true,
        view: Some(reach.expand(reach.width().max(reach.height()) * 0.05)),
        ..Default::default()
    };
    cadcraft_render::build(d, space, &opts)
}

/// A figure: `list` in `window`, faded except the objects `marked` (in `color`), the `areas`
/// outlined and numbered.
fn figure(
    h: &mut String,
    list: &cadcraft_render::DisplayList,
    window: Bounds2,
    marked: &HashSet<Handle>,
    color: &str,
    areas: &[Bounds2],
    caption: &str,
) {
    let _ = write!(h, "<figure><figcaption>{caption}</figcaption>");
    svg(h, list, window, marked, color, areas);
    h.push_str("</figure>");
}

fn svg(h: &mut String, list: &cadcraft_render::DisplayList, window: Bounds2, marked: &HashSet<Handle>, color: &str, areas: &[Bounds2]) {
    let px = window.width() / PICTURE_PX;
    let (x0, y1) = (window.min.x, window.max.y);
    let (vw, vh) = (window.width(), window.height());
    let tx = |v: Vec2| ((v.x - x0) / px, (y1 - v.y) / px);
    let (w, ht) = (PICTURE_PX, vh / px);
    let _ = write!(h, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {ht:.0}" preserveAspectRatio="xMidYMid meet">"#);
    let _ = write!(h, r#"<rect width="100%" height="100%" fill="white"/>"#);
    let visible = window.expand(vw.max(vh) * 0.01);
    // Faded first, then the marked objects over them.
    for pass in [false, true] {
        let mut lines = String::new();
        let mut fills = String::new();
        for p in list.prims.iter().filter(|p| marked.contains(&p.handle) == pass) {
            let pts = list.points(p);
            if pts.is_empty() || !Bounds2::from_points(pts.iter().copied()).intersects(&visible) {
                continue;
            }
            match p.kind {
                Kind::Polyline => {
                    // Points closer than half a pixel to the last one written add nothing.
                    let mut last: Option<(f64, f64)> = None;
                    for (i, q) in pts.iter().enumerate() {
                        let (x, y) = tx(*q);
                        let end = i + 1 == pts.len();
                        if let Some((lx, ly)) = last
                            && !end
                            && (x - lx).abs() < 0.5
                            && (y - ly).abs() < 0.5
                        {
                            continue;
                        }
                        let _ = write!(lines, "{}{x:.1} {y:.1}", if last.is_none() { "M" } else { "L" });
                        last = Some((x, y));
                    }
                }
                Kind::Tris if !pass => {
                    for t in pts.chunks(3) {
                        if let [a, b, cc] = t {
                            let ((ax, ay), (bx, by), (cx, cy)) = (tx(*a), tx(*b), tx(*cc));
                            let _ = write!(fills, "M{ax:.1} {ay:.1}L{bx:.1} {by:.1}L{cx:.1} {cy:.1}Z");
                        }
                    }
                }
                _ => {}
            }
        }
        if pass {
            if !lines.is_empty() {
                let _ =
                    write!(h, r#"<path d="{lines}" fill="none" stroke="{color}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>"#);
            }
        } else {
            if !fills.is_empty() {
                let _ = write!(h, r##"<path d="{fills}" fill="#9aa3ad" opacity=".25"/>"##);
            }
            if !lines.is_empty() {
                let _ = write!(h, r##"<path d="{lines}" fill="none" stroke="#8c959f" stroke-width=".6" stroke-linejoin="round"/>"##);
            }
        }
    }
    for (i, a) in areas.iter().enumerate() {
        let (ax, ay) = tx(Vec2::new(a.min.x, a.max.y));
        let (bx, by) = tx(Vec2::new(a.max.x, a.min.y));
        let (x, y, rw, rh) = (ax - 6.0, ay - 6.0, (bx - ax) + 12.0, (by - ay) + 12.0);
        let _ = write!(
            h,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{rw:.1}" height="{rh:.1}" rx="4" fill="none" stroke="{AREA}" stroke-width="1.5"/><text x="{:.1}" y="{:.1}" font-size="13" font-family="sans-serif" font-weight="bold" fill="{AREA}">{}</text>"#,
            x,
            (y - 4.0).max(12.0),
            i + 1
        );
    }
    h.push_str("</svg>");
}
