//! A drawing as stable, line-oriented text, for version control: `git diff` of DWG/DXF files
//! through a textconv driver (`cadcraft-cli textconv`), review tools and agents. One line per
//! object, sorted (tables and blocks by name, objects by handle), coordinates rounded and derived
//! data left out (see [`super::compare::normalized`]): the same drawing always gives the same text,
//! also after a write and read back, and an edit changes only the lines of the objects it touched.
//!
//! ```text
//! [layers]
//! "A-WALL" {"color":…,"linetype":"Continuous",…}
//! [block "DOOR"] {"base":…}
//! 1F Line layer="0" {"a":…,"b":…}
//! [model]
//! 2A3 Circle layer="FURN" {"color":{"index":1}} {"center":…,"radius":400.0}
//! [layout "Layout1"] {…page setup…}
//! ```
//!
//! Not listed: header variables (save timestamps, the current view and other session state
//! change on every save) and the generated geometry of dimensions (`*D` blocks). Anonymous
//! blocks (`*U12`, dynamic block variants and the like) are renumbered by every save, in
//! AutoCAD as here, so they are named by their contents instead (`*U#5f3a09c1`), in their
//! section and in the references to them; their objects are listed without handles.

use std::collections::HashMap;

use cadcraft_doc::{Common, Drawing, Entity, EntityKind, EntityStore, MAX_BLOCK_DEPTH};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::super::*;
use super::compare::{normalized, round};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("drawing.listing", "Drawing Listing", run_listing)
            .params("{} → {text, lines}: the drawing as stable text, one line per object (for `git diff` and review)")
            .noundo(),
    ]
}

fn run_listing(s: &mut Session, _p: &Value) -> Result<Value> {
    let text = listing(&s.state()?.doc);
    Ok(json!({ "lines": text.lines().count(), "text": text }))
}

/// The drawing as text, one line per table entry and object; see the module documentation.
pub fn listing(d: &Drawing) -> String {
    let mut out = String::new();
    table(&mut out, "layers", &d.layers, |l| &l.name);
    table(&mut out, "linetypes", &d.linetypes, |l| &l.name);
    table(&mut out, "text styles", &d.text_styles, |t| &t.name);
    table(&mut out, "dimension styles", &d.dim_styles, |t| &t.name);
    table(&mut out, "multileader styles", &d.mleader_styles, |t| &t.name);
    table(&mut out, "table styles", &d.table_styles, |t| &t.name);
    table(&mut out, "multiline styles", &d.mline_styles, |t| &t.name);
    table(&mut out, "groups", &d.groups, |g| &g.name);
    let names = anonymous_names(d);
    let mut blocks: Vec<_> = d.blocks.values().filter(|b| !is_dimension_block(&b.name)).map(|b| (shown_name(&names, &b.name), b)).collect();
    blocks.sort_by(|(a, _), (b, _)| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()).then_with(|| a.cmp(b)));
    for (name, b) in blocks {
        section(&mut out, &format!("block {}", quote(&name)), Some(fields(&**b, &[])));
        objects(&mut out, &b.entities, &names, names.contains_key(&b.name.to_ascii_lowercase()));
    }
    section(&mut out, "model", None);
    objects(&mut out, &d.model, &names, false);
    let mut layouts: Vec<_> = d.layouts.iter().collect();
    layouts.sort_by_key(|l| (l.tab_order, l.name.to_ascii_lowercase()));
    for l in layouts {
        section(&mut out, &format!("layout {}", quote(&l.name)), Some(fields(l, &["tab_order", "tabOrder", "view"])));
        objects(&mut out, &l.entities, &names, false);
    }
    out
}

/// Passes naming anonymous blocks: a block nested this deep in other anonymous blocks still gets
/// a name independent of the numbering.
const NAMING_PASSES: usize = 8;

/// Anonymous block name (lower case) → its name by contents: `*U#` and a hash of its objects.
/// References to other anonymous blocks inside count by those blocks' names by contents: every
/// pass names one more level of nesting, from `*U?` (any anonymous block) at the start.
fn anonymous_names(d: &Drawing) -> HashMap<String, String> {
    let anon: Vec<_> = d.blocks.values().filter(|b| b.anonymous && !is_dimension_block(&b.name) && !is_space_block(&b.name)).collect();
    let mut names: HashMap<String, String> = anon.iter().map(|b| (b.name.to_ascii_lowercase(), "*U?".to_string())).collect();
    for _ in 0..NAMING_PASSES.min(MAX_BLOCK_DEPTH) {
        let next: HashMap<String, String> = anon
            .iter()
            .map(|b| {
                let mut body = fields(&***b, &[]).to_string();
                objects(&mut body, &b.entities, &names, true);
                let kind: String = b.name.chars().take(2).collect();
                (b.name.to_ascii_lowercase(), format!("{kind}#{:08x}", fnv1a(body.as_bytes()) as u32))
            })
            .collect();
        if next == names {
            break;
        }
        names = next;
    }
    names
}

fn shown_name(names: &HashMap<String, String>, name: &str) -> String {
    names.get(&name.to_ascii_lowercase()).cloned().unwrap_or_else(|| name.to_string())
}

/// FNV-1a: the same value on every platform and Rust version (unlike `DefaultHasher`).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn is_space_block(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("*model_space") || n.starts_with("*paper_space")
}

/// `*D12`: a dimension's generated geometry, rebuilt from the dimension.
fn is_dimension_block(name: &str) -> bool {
    name.strip_prefix("*D").or_else(|| name.strip_prefix("*d")).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn quote(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

fn section(out: &mut String, title: &str, extra: Option<Value>) {
    out.push('[');
    out.push_str(title);
    out.push(']');
    if let Some(v) = extra.filter(|v| v.as_object().is_none_or(|o| !o.is_empty())) {
        out.push(' ');
        out.push_str(&v.to_string());
    }
    out.push('\n');
}

/// `v` as rounded JSON without `name` and the `skip` keys.
fn fields<T: Serialize>(v: &T, skip: &[&str]) -> Value {
    let mut v = serde_json::to_value(v).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("name");
        skip.iter().for_each(|k| {
            o.remove(*k);
        });
    }
    round(&mut v, 0);
    v
}

fn table<T: Serialize>(out: &mut String, title: &str, rows: &[T], name: fn(&T) -> &str) {
    if rows.is_empty() {
        return;
    }
    section(out, title, None);
    let mut rows: Vec<&T> = rows.iter().collect();
    rows.sort_by_key(|r| name(r).to_ascii_lowercase());
    for r in rows {
        out.push_str(&quote(name(r)));
        out.push(' ');
        out.push_str(&fields(r, &[]).to_string());
        out.push('\n');
    }
}

/// The objects of `store`, one line each, sorted by handle. In an anonymous block (`anonymous`),
/// which a save regenerates with new handles, the handle is left out (`-`), as are handles in
/// the objects' xdata (1005), and the lines are sorted by content.
fn objects(out: &mut String, store: &EntityStore, names: &HashMap<String, String>, anonymous: bool) {
    let mut es: Vec<&Entity> = store.iter().map(|e| &**e).collect();
    es.sort_by_key(|e| e.handle);
    let defaults = serde_json::to_value(Common::default()).unwrap_or(Value::Null);
    let mut lines: Vec<String> = es.into_iter().map(|e| object(e, names, &defaults, anonymous)).collect();
    if anonymous {
        lines.sort();
    }
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
}

fn object(e: &Entity, names: &HashMap<String, String>, defaults: &Value, anonymous: bool) -> String {
    let mut out = if anonymous { "-".to_string() } else { e.handle.hex() };
    out.push(' ');
    out.push_str(e.kind.type_name());
    out.push_str(" layer=");
    out.push_str(&quote(&e.common.layer));
    let mut props = changed_properties(&e.common, defaults);
    if let (true, Some(Value::Array(xdata))) = (anonymous, props.get_mut("xdata")) {
        for t in xdata.iter_mut().filter(|t| t.get("code").and_then(Value::as_i64) == Some(1005)) {
            if let Some(o) = t.as_object_mut() {
                o.insert("value".into(), Value::String("-".into()));
            }
        }
    }
    if !props.is_empty() {
        out.push(' ');
        out.push_str(&Value::Object(props).to_string());
    }
    out.push(' ');
    let mut g = normalized(&e.kind);
    if let (EntityKind::Insert(i), Some(o)) = (&e.kind, g.as_object_mut())
        && let Some(n) = names.get(&i.block.to_ascii_lowercase())
    {
        o.insert("block".into(), Value::String(n.clone()));
    }
    if let (EntityKind::Unknown(_), Some(Value::Array(tags))) = (&e.kind, g.get_mut("tags")) {
        drop_handles(tags);
    }
    out.push_str(&g.to_string());
    out
}

/// An object kept as raw DXF tags: its own handle (5) and its owner (the first 330), which a
/// save renumbers, are left out.
fn drop_handles(tags: &mut Vec<Value>) {
    let code = |t: &Value| t.get("code").and_then(Value::as_i64);
    tags.retain(|t| code(t) != Some(5));
    if let Some(i) = tags.iter().position(|t| code(t) == Some(330)) {
        tags.remove(i);
    }
}

/// The object's properties other than the layer that differ from the defaults (ByLayer colour,
/// linetype and lineweight, …), so a typical object's line shows only its geometry.
fn changed_properties(c: &Common, defaults: &Value) -> Map<String, Value> {
    let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
    round(&mut v, 0);
    let Value::Object(mut o) = v else { return Map::new() };
    o.remove("layer");
    o.retain(|k, v| defaults.get(k) != Some(v));
    o
}
