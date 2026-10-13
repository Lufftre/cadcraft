//! COMPARE: compare the drawing with another one (a file read through the io hooks, or an open
//! drawing). Objects are matched by geometry and properties first (preferring the same handle),
//! then by handle: the same handle with other geometry or properties is a change. What is left
//! is only in this drawing or only in the other. The comparison stays on the session for the
//! canvas overlay and next/previous navigation until COMPARECLOSE.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cadcraft_doc::{Drawing, Entity, EntityKind, Handle, MAX_BLOCK_DEPTH, Space, entity_bounds};
use cadcraft_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

use super::super::*;
use super::{base64_decode, io};
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Objects compared per drawing; more are left out (and the result says so).
pub const MAX_OBJECTS: usize = 200_000;
/// Objects listed per kind of difference in the JSON result.
const MAX_LISTED: usize = 1000;
/// Block definition objects hashed into a reference's signature, per comparison.
const MAX_BLOCK_WORK: usize = 1_000_000;
/// The change areas: the extents are cut into at most this many cells per side.
const GRID: usize = 32;

/// A drawing comparison: what differs between the current drawing and another one.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// The drawing compared ([`crate::DocState::uid`]).
    pub uid: u64,
    /// The space compared (the tab shown when COMPARE ran).
    pub space: Space,
    /// The other drawing's name.
    pub other_name: String,
    /// The other drawing (its objects are drawn where they differ).
    pub other: Arc<Drawing>,
    /// Only in this drawing (handles here).
    pub added: Vec<Handle>,
    /// Only in the other drawing (handles there).
    pub removed: Vec<Handle>,
    /// The same handle in both with other geometry or properties: (here, there).
    pub changed: Vec<(Handle, Handle)>,
    /// Objects found in both.
    pub common: usize,
    /// More than [`MAX_OBJECTS`] objects: the rest were not compared.
    pub truncated: bool,
    /// Areas holding differences, in reading order (top to bottom, left to right).
    pub regions: Vec<Bounds2>,
    /// The area shown last by next/previous (`None` before the first).
    pub at: Option<usize>,
    /// Shown side by side: the other drawing to the left, this one to the right, with one view.
    pub side_by_side: bool,
}

impl Comparison {
    pub fn differences(&self) -> usize {
        self.added.len() + self.removed.len() + self.changed.len()
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("compare", "Compare Drawings...", run_compare)
            .menu(&["Tools", "Compare Drawings..."])
            .params(
                "{path} | {data: base64, name} | {document: index or title} | {revision: git branch, tag or commit of the drawing's own file}, sideBySide?: bool → {other, added: [{handle, type}], removed, \
                 changed: [{handle, other, type, what}], counts: {added, removed, changed, common}, regions, truncated, message}",
            )
            .noundo()
            .interactive(|_| Ok(Box::new(CompareM))),
        CommandSpec::new("compare.branch", "Compare with Branch...", |s, p| run_compare(s, &json!({ "revision": str_param(p, "revision").unwrap_or("HEAD") })))
            .menu(&["Tools", "Compare with Branch..."])
            .params("{revision?: git branch, tag or commit (default HEAD, the last commit)} → as compare; branches: compare.branches")
            .noundo()
            .interactive(|s| Ok(Box::new(CompareBranchM::new(s)))),
        CommandSpec::new("compare.branches", "Branches to Compare With", run_branches)
            .params("→ {branches}: the git branches the drawing's file can be compared with (`HEAD` last)")
            .noundo(),
        CommandSpec::new("compare.sidebyside", "Side by Side", run_side_by_side)
            .params("{on?: bool (default: switch)} → {sideBySide}: show the other drawing to the left of this one, with one view")
            .noundo(),
        CommandSpec::new("compare.next", "Next Change", |s, _| step(s, 1)).params("→ {region, of}: zoom to the next area with changes").noundo(),
        CommandSpec::new("compare.previous", "Previous Change", |s, _| step(s, -1)).params("→ {region, of}").noundo(),
        CommandSpec::new("compareclose", "Close Comparison", run_close).params("{}: end the comparison and its overlay").noundo(),
    ]
}

/// Read the drawing to compare with from `{path}`, `{data, name}`, `{document}` or `{revision}`.
fn other_drawing(s: &Session, p: &Value) -> Result<(Arc<Drawing>, String)> {
    const CMD: &str = "compare";
    if let Some(rev) = str_param(p, "revision") {
        let rev = rev.trim();
        let path = super::compare_git::drawing_path(s, CMD)?;
        let hooks = io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
        let bytes = super::compare_git::read_revision(CMD, &path, rev)?;
        let file = path.rsplit(['/', '\\']).next().unwrap_or(&path);
        let name = format!("{rev}:{file}");
        let d = (hooks.read)(&bytes, &path).map_err(|e| bad(CMD, format!("{name}: {e}")))?;
        return Ok((Arc::new(d), name));
    }
    if let Some(d) = p.get("document").filter(|v| !v.is_null()) {
        let st = match (d.as_u64(), d.as_str()) {
            (Some(i), _) => usize::try_from(i).ok().and_then(|i| s.docs.get(i)),
            (_, Some(t)) => s.docs.iter().find(|st| st.title.eq_ignore_ascii_case(t.trim())),
            _ => None,
        };
        let st = st.ok_or_else(|| bad(CMD, format!("no open drawing {d}")))?;
        return Ok((st.doc.clone(), st.title.clone()));
    }
    let hooks = io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    if let Some(path) = str_param(p, "path") {
        let bytes = read_path(path)?;
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
        let d = (hooks.read)(&bytes, path).map_err(|e| bad(CMD, format!("{name}: {e}")))?;
        return Ok((Arc::new(d), name));
    }
    let data = str_param(p, "data").ok_or_else(|| bad(CMD, "`path`, `data` (base64) or `document` is required"))?;
    let bytes = base64_decode(data).ok_or_else(|| bad(CMD, "invalid base64"))?;
    let name = str_param(p, "name").unwrap_or("Drawing.dxf").to_string();
    let d = (hooks.read)(&bytes, &name).map_err(|e| bad(CMD, format!("{name}: {e}")))?;
    Ok((Arc::new(d), name))
}

fn read_path(_path: &str) -> Result<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::read(_path).map_err(|e| bad("compare", format!("{_path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    Err(bad("compare", "paths are not available on the web: open the other drawing and compare with it by name (`document`)"))
}

/// What differs between two matched objects, in COMPARE's words: "geometry", "properties",
/// "text" (only a text's words), combinations of these, or "block contents" (a block
/// reference whose block definition changed).
pub(super) fn what_changed(x: &Entity, y: &Entity) -> &'static str {
    let geom = normalized(&x.kind) != normalized(&y.kind);
    let props = properties(x) != properties(y);
    let words = only_words_differ(&x.kind, &y.kind);
    match (geom, props) {
        (true, true) if words => "text and properties",
        (true, false) if words => "text",
        (true, true) => "geometry and properties",
        (true, false) => "geometry",
        (false, true) => "properties",
        (false, false) => "block contents",
    }
}

/// Two texts (text, mtext, attribute definition) that differ only in their words.
fn only_words_differ(a: &EntityKind, b: &EntityKind) -> bool {
    match (a, b) {
        (EntityKind::Text(x), EntityKind::Text(y)) => x.value != y.value && cadcraft_doc::Text { value: y.value.clone(), ..x.clone() } == *y,
        (EntityKind::MText(x), EntityKind::MText(y)) => {
            x.contents != y.contents && cadcraft_doc::MText { contents: y.contents.clone(), ..x.clone() } == *y
        }
        (EntityKind::AttDef(x), EntityKind::AttDef(y)) => {
            let mut z = x.clone();
            z.text.value.clone_from(&y.text.value);
            x.text.value != y.text.value && z == *y
        }
        _ => false,
    }
}

/// An object's geometry as JSON, without what is derived from it (a dimension's generated block,
/// its cached measurement, and its text position unless the user placed the text) and with
/// coordinates rounded: equal for equal geometry, also across a write and read back.
pub(super) fn normalized(k: &EntityKind) -> Value {
    let mut v = serde_json::to_value(k).unwrap_or(Value::Null);
    if let (Some(o), EntityKind::Dimension(dm)) = (v.as_object_mut(), k) {
        o.remove("block");
        o.remove("measurement");
        if !dm.user_text_pos {
            o.remove("textMid");
        }
    }
    round(&mut v, 0);
    v
}

/// Rounds coordinates (1e-6) so that a drawing written and read back still matches itself.
pub(super) fn round(v: &mut Value, depth: usize) {
    if depth > 64 {
        return;
    }
    match v {
        Value::Number(n) if n.is_f64() => {
            if let Some(f) = n.as_f64() {
                let r = (f * 1e6).round() / 1e6;
                *v = json!(if r == 0.0 { 0.0 } else { r });
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| round(x, depth + 1)),
        Value::Object(o) => o.values_mut().for_each(|x| round(x, depth + 1)),
        _ => {}
    }
}

/// Signatures of objects: geometry and properties hashed separately; references include their
/// block definition's contents.
struct Signer<'a> {
    d: &'a Drawing,
    blocks: HashMap<String, u64>,
    work: usize,
}

impl<'a> Signer<'a> {
    fn new(d: &'a Drawing) -> Self {
        Signer { d, blocks: HashMap::new(), work: 0 }
    }
    fn geometry(&mut self, k: &EntityKind, depth: usize) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut v = normalized(k);
        if let EntityKind::Insert(i) = k {
            // An anonymous block (`*U12`, a dynamic block's variant) is renumbered by every save:
            // it counts by its contents, hashed below, not its name.
            if is_anonymous(&i.block)
                && let Some(o) = v.as_object_mut()
            {
                o.remove("block");
            }
            self.block(&i.block, depth).hash(&mut h);
        }
        v.to_string().hash(&mut h);
        h.finish()
    }
    fn block(&mut self, name: &str, depth: usize) -> u64 {
        let key = name.to_ascii_lowercase();
        if let Some(v) = self.blocks.get(&key) {
            return *v;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if !is_anonymous(name) {
            key.hash(&mut h);
        }
        if depth < MAX_BLOCK_DEPTH
            && let Some(b) = self.d.block(name)
        {
            for e in b.entities.iter() {
                if self.work >= MAX_BLOCK_WORK {
                    break;
                }
                self.work += 1;
                self.geometry(&e.kind, depth + 1).hash(&mut h);
                properties(e).hash(&mut h);
            }
        }
        let v = h.finish();
        self.blocks.insert(key, v);
        v
    }
}

/// `*U12`, `*X3`…: a block a save may renumber (not the `*Model_Space`/`*Paper_Space` blocks).
fn is_anonymous(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with('*') && !n.starts_with("*model_space") && !n.starts_with("*paper_space")
}

fn properties(e: &Entity) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let c = &e.common;
    c.layer.to_ascii_lowercase().hash(&mut h);
    format!("{:?}|{}|{:?}|{}", c.color, c.linetype.to_ascii_lowercase(), c.lineweight, (c.ltscale * 1e6).round()).hash(&mut h);
    h.finish()
}

struct Obj<'a> {
    e: &'a Arc<Entity>,
    geom: u64,
    props: u64,
}

fn objects<'a>(d: &'a Drawing, space: &Space, truncated: &mut bool) -> Vec<Obj<'a>> {
    let mut sg = Signer::new(d);
    let Some(store) = d.space(space) else { return Vec::new() };
    let mut out = Vec::new();
    for e in store.iter() {
        if out.len() >= MAX_OBJECTS {
            *truncated = true;
            break;
        }
        out.push(Obj { e, geom: sg.geometry(&e.kind, 0), props: properties(e) });
    }
    out
}

/// What differs between two drawings' spaces.
struct Diff {
    added: Vec<Handle>,
    removed: Vec<Handle>,
    /// (here, there, geometry differs, properties differ).
    changed: Vec<(Handle, Handle, bool, bool)>,
    common: usize,
    truncated: bool,
}

/// Compare `cur`'s `space` with the same space of `other`.
fn diff(cur: &Drawing, other: &Drawing, space: &Space) -> Diff {
    let mut truncated = false;
    let a = objects(cur, space, &mut truncated);
    let b = objects(other, space, &mut truncated);
    let mut b_left: Vec<bool> = vec![true; b.len()];
    let mut a_left: Vec<bool> = vec![true; a.len()];
    let by_handle: HashMap<Handle, usize> = b.iter().enumerate().map(|(i, o)| (o.e.handle, i)).collect();
    let mut common = 0;
    // 1. The same handle, geometry and properties.
    for (i, o) in a.iter().enumerate() {
        if let Some(&j) = by_handle.get(&o.e.handle)
            && let Some(q) = b.get(j)
            && q.geom == o.geom
            && q.props == o.props
            && let (Some(al), Some(bl)) = (a_left.get_mut(i), b_left.get_mut(j))
        {
            (*al, *bl) = (false, false);
            common += 1;
        }
    }
    // 2. Equal geometry and properties under another handle (a file from another program).
    let mut by_sig: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (j, q) in b.iter().enumerate().rev() {
        if b_left.get(j).copied().unwrap_or(false) {
            by_sig.entry((q.geom, q.props)).or_default().push(j);
        }
    }
    for (i, o) in a.iter().enumerate() {
        if !a_left.get(i).copied().unwrap_or(false) {
            continue;
        }
        if let Some(j) = by_sig.get_mut(&(o.geom, o.props)).and_then(Vec::pop)
            && let (Some(al), Some(bl)) = (a_left.get_mut(i), b_left.get_mut(j))
        {
            (*al, *bl) = (false, false);
            common += 1;
        }
    }
    // 3. The same handle and type, other geometry or properties: changed.
    let mut changed = Vec::new();
    for (i, o) in a.iter().enumerate() {
        if !a_left.get(i).copied().unwrap_or(false) {
            continue;
        }
        if let Some(&j) = by_handle.get(&o.e.handle)
            && b_left.get(j).copied().unwrap_or(false)
            && let Some(q) = b.get(j)
            && q.e.kind.type_name() == o.e.kind.type_name()
            && let (Some(al), Some(bl)) = (a_left.get_mut(i), b_left.get_mut(j))
        {
            (*al, *bl) = (false, false);
            changed.push((o.e.handle, q.e.handle, o.geom != q.geom, o.props != q.props));
        }
    }
    let added = a.iter().zip(&a_left).filter(|(_, l)| **l).map(|(o, _)| o.e.handle).collect();
    let removed = b.iter().zip(&b_left).filter(|(_, l)| **l).map(|(o, _)| o.e.handle).collect();
    Diff { added, removed, changed, common, truncated }
}

/// Group the differences into areas: one per occupied cell of a grid over their extents.
fn regions(boxes: &[Bounds2]) -> Vec<Bounds2> {
    let mut all = Bounds2::EMPTY;
    for b in boxes {
        all = all.union(b);
    }
    if all.is_empty() {
        return Vec::new();
    }
    let (w, h) = (all.width().max(1e-9), all.height().max(1e-9));
    let mut cells: HashMap<(usize, usize), Bounds2> = HashMap::new();
    for b in boxes.iter().filter(|b| !b.is_empty()) {
        let c = b.center();
        let cell = |v: f64, lo: f64, size: f64| (((v - lo) / size * GRID as f64).floor().max(0.0) as usize).min(GRID - 1);
        let k = (cell(c.x, all.min.x, w), cell(c.y, all.min.y, h));
        let cell = cells.entry(k).or_insert(Bounds2::EMPTY);
        *cell = cell.union(b);
    }
    let mut out: Vec<((usize, usize), Bounds2)> = cells.into_iter().collect();
    // Reading order: top row first, left to right.
    out.sort_by(|a, b| b.0.1.cmp(&a.0.1).then(a.0.0.cmp(&b.0.0)));
    out.into_iter().map(|(_, b)| b).collect()
}

fn listed(d: &Drawing, hs: &[Handle]) -> Vec<Value> {
    hs.iter().take(MAX_LISTED).filter_map(|h| d.entity(*h)).map(|e| json!({ "handle": e.handle.hex(), "type": e.kind.type_name() })).collect()
}

fn run_compare(s: &mut Session, p: &Value) -> Result<Value> {
    let (other, name) = other_drawing(s, p)?;
    let st = s.state()?;
    let (uid, space, cur) = (st.uid, st.space.clone(), st.doc.clone());
    if Arc::ptr_eq(&cur, &other) {
        return Err(bad("compare", "a drawing can't be compared with itself"));
    }
    let Diff { added, removed, changed, common, truncated } = diff(&cur, &other, &space);
    let mut boxes = Vec::new();
    let mut bound = |d: &Drawing, h: Handle| {
        if let Some(b) = d.entity(h).map(|e| entity_bounds(d, e, 0)).filter(|b| !b.is_empty() && b.min.is_finite() && b.max.is_finite()) {
            boxes.push(b);
        }
    };
    added.iter().for_each(|h| bound(&cur, *h));
    removed.iter().for_each(|h| bound(&other, *h));
    for (a, b, _, _) in &changed {
        bound(&cur, *a);
        bound(&other, *b);
    }
    let regions = regions(&boxes);
    let ch: Vec<Value> = changed
        .iter()
        .take(MAX_LISTED)
        .filter_map(|(a, b, geom, props)| {
            let x = cur.entity(*a)?;
            let words = other.entity(*b).is_some_and(|y| only_words_differ(&x.kind, &y.kind));
            let what = match (geom, props) {
                (true, true) if words => "text and properties",
                (true, false) if words => "text",
                (true, true) => "geometry and properties",
                (true, false) => "geometry",
                _ => "properties",
            };
            Some(json!({ "handle": a.hex(), "other": b.hex(), "type": x.kind.type_name(), "what": what }))
        })
        .collect();
    let message = format!(
        "Compared with {name}: {} only in this drawing, {} only in {name}, {} changed, {common} the same{}.",
        added.len(),
        removed.len(),
        changed.len(),
        if truncated { format!(" (only the first {MAX_OBJECTS} objects of each were compared)") } else { String::new() }
    );
    let out = json!({
        "other": name,
        "added": listed(&cur, &added),
        "removed": listed(&other, &removed),
        "changed": ch,
        "counts": { "added": added.len(), "removed": removed.len(), "changed": changed.len(), "common": common },
        "regions": regions.iter().map(|b| json!([[b.min.x, b.min.y], [b.max.x, b.max.y]])).collect::<Vec<_>>(),
        "truncated": truncated,
        "message": message,
    });
    let changed = changed.into_iter().map(|(a, b, _, _)| (a, b)).collect();
    // Side by side stays as it was for a new comparison; `sideBySide` sets it.
    let side_by_side = p.get("sideBySide").and_then(Value::as_bool).unwrap_or_else(|| s.compare.as_ref().is_some_and(|c| c.side_by_side));
    s.compare = Some(Comparison { uid, space, other_name: name, other, added, removed, changed, common, truncated, regions, at: None, side_by_side });
    s.touch();
    Ok(out)
}

/// Zoom to the next (`dir` 1) or previous (-1) area with changes.
fn step(s: &mut Session, dir: i64) -> Result<Value> {
    let uid = s.state()?.uid;
    let c = s.compare.as_mut().filter(|c| c.uid == uid).ok_or_else(|| bad("compare", "no comparison: run COMPARE first"))?;
    let n = c.regions.len();
    if n == 0 {
        return Ok(json!({ "region": Value::Null, "of": 0, "message": "No differences." }));
    }
    let i = match (c.at, dir > 0) {
        (None, true) => 0,
        (None, false) => n - 1,
        (Some(i), true) => (i + 1) % n,
        (Some(i), false) => (i + n - 1) % n,
    };
    c.at = Some(i);
    let Some(b) = c.regions.get(i).copied() else { return Err(bad("compare", "no such area")) };
    // The area with as much room around it (and a size for a single point).
    let pad = b.width().max(b.height()).max(1e-3);
    let (p1, p2) = (b.min - Vec2::new(pad, pad), b.max + Vec2::new(pad, pad));
    s.execute("zoom", &json!({ "mode": "window", "p1": [p1.x, p1.y], "p2": [p2.x, p2.y] }))?;
    Ok(json!({ "region": i, "of": n, "message": format!("Change area {} of {n}.", i + 1) }))
}

fn run_close(s: &mut Session, _p: &Value) -> Result<Value> {
    let had = s.compare.take().is_some();
    s.touch();
    Ok(json!({ "closed": had }))
}

fn run_side_by_side(s: &mut Session, p: &Value) -> Result<Value> {
    let uid = s.state()?.uid;
    let c = s.compare.as_mut().filter(|c| c.uid == uid).ok_or_else(|| bad("compare.sidebyside", "no comparison: run COMPARE first"))?;
    c.side_by_side = p.get("on").and_then(Value::as_bool).unwrap_or(!c.side_by_side);
    Ok(json!({ "sideBySide": c.side_by_side }))
}

fn run_branches(s: &mut Session, _p: &Value) -> Result<Value> {
    let path = super::compare_git::drawing_path(s, "compare.branches")?;
    Ok(json!({ "branches": super::compare_git::branches(&path) }))
}

/// COMPARE at the command line: the drawing to compare with, an open drawing's name, a file path,
/// or a git branch, tag or commit of the drawing's own file.
struct CompareM;

impl Interactive for CompareM {
    fn name(&self) -> &'static str {
        "COMPARE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Enter the drawing to compare with (an open drawing's name, a file path, or a git branch or commit)", Accept::TEXT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let Input::Text(t) = i else { return Ok(Step::Done) };
        let t = t.trim().trim_matches('"');
        if t.is_empty() {
            return Ok(Step::Done);
        }
        let open = s.docs.iter().enumerate().any(|(i, d)| i != s.active && d.title.eq_ignore_ascii_case(t));
        let p = if open {
            json!({ "document": t })
        } else if is_file(t) || s.state()?.path.is_none() || super::compare_git::check_revision("compare", t).is_err() {
            json!({ "path": t })
        } else {
            json!({ "revision": t })
        };
        let r = run_compare(s, &p)?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        Ok(Step::Done)
    }
}

fn is_file(_t: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return std::path::Path::new(_t).is_file();
    #[cfg(target_arch = "wasm32")]
    false
}

/// Compare with Branch: the branches of the drawing's git repository as options (`main` first,
/// `HEAD` for the last commit); any other branch, tag or commit can be typed.
struct CompareBranchM {
    branches: Vec<String>,
}

impl CompareBranchM {
    fn new(s: &Session) -> Self {
        let branches = s.state().ok().and_then(|st| st.path.clone()).map(|p| super::compare_git::branches(&p)).unwrap_or_default();
        CompareBranchM { branches }
    }
}

impl Interactive for CompareBranchM {
    fn name(&self) -> &'static str {
        "COMPARE"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        let kws: Vec<&str> = self.branches.iter().map(String::as_str).collect();
        let p = Prompt::new("Compare with which branch or commit?", Accept::TEXT).kw(&kws);
        match self.branches.first() {
            Some(b) => p.default(b.clone()),
            None => p,
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let rev = match i {
            Input::Text(t) | Input::Keyword(t) if !t.trim().is_empty() => t.trim().to_string(),
            Input::Text(_) | Input::Enter => match self.branches.first() {
                Some(b) => b.clone(),
                None => return Ok(Step::Done),
            },
            _ => return Ok(Step::Done),
        };
        let r = run_compare(s, &json!({ "revision": rev }))?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        Ok(Step::Done)
    }
}
