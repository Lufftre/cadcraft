//! `drawing.listing`: the drawing as stable text for `git diff` (one line per object, sorted,
//! rounded, without derived data).

use cadcraft_engine::Session;
use cadcraft_engine::cmd::file::listing::listing;
use cadcraft_engine::doc::Drawing;
use serde_json::json;

fn text(s: &mut Session) -> String {
    s.execute("drawing.listing", &json!({})).unwrap()["text"].as_str().unwrap().to_string()
}

fn base() -> Session {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    s.execute("circle", &json!({"center": [5, 5], "radius": 2})).unwrap();
    s.execute("text", &json!({"at": [0, 10], "height": 1, "text": "Kitchen\nsink"})).unwrap();
    s
}

#[test]
fn one_line_per_object_sorted_by_handle() {
    let mut s = base();
    let t = text(&mut s);
    let model: Vec<&str> = t.lines().skip_while(|l| *l != "[model]").skip(1).take_while(|l| !l.starts_with('[')).collect();
    assert_eq!(model.len(), 3, "{t}");
    assert!(model[0].contains(" Line layer=\"0\" "), "{t}");
    assert!(model[1].contains(" Circle "), "{t}");
    assert!(model[2].contains("Kitchen\\nsink"), "a newline in a text stays on its line: {t}");
    assert!(t.contains("[layers]\n\"0\" "), "{t}");
}

#[test]
fn the_same_drawing_gives_the_same_text() {
    let mut s = base();
    let a = text(&mut s);
    let d: Drawing = s.doc().unwrap().clone();
    s.open_drawing(d, "Copy", None);
    // Session state (view, selection) is not part of the listing.
    s.execute("zoom", &json!({"mode": "extents"})).ok();
    assert_eq!(a, text(&mut s));
}

#[test]
fn an_edit_changes_only_its_objects_line() {
    let mut s = base();
    let a = text(&mut s);
    let circle = s.doc().unwrap().model.iter().find(|e| e.kind.type_name() == "Circle").unwrap().handle.hex();
    s.execute("move", &json!({"handles": [circle], "delta": [1, 0]})).unwrap();
    let b = text(&mut s);
    let differ: Vec<(&str, &str)> = a.lines().zip(b.lines()).filter(|(x, y)| x != y).collect();
    assert_eq!(a.lines().count(), b.lines().count());
    assert_eq!(differ.len(), 1, "{differ:?}");
    assert!(differ[0].0.starts_with(&format!("{circle} Circle")), "{differ:?}");
}

#[test]
fn properties_show_only_when_not_default() {
    let mut s = base();
    let l = s.doc().unwrap().model.iter().find(|e| e.kind.type_name() == "Line").unwrap().handle.hex();
    let before = text(&mut s);
    let line = |t: &str| t.lines().find(|x| x.starts_with(&format!("{l} "))).unwrap().to_string();
    assert!(!line(&before).contains("color"), "{before}");
    s.execute("properties.set", &json!({"handles": [l], "color": "red"})).unwrap();
    assert!(line(&text(&mut s)).contains("\"color\""));
}

#[test]
fn hostile_names_stay_on_one_line() {
    let mut d = Drawing::new_imperial();
    let mut layer = d.layers[0].clone();
    layer.name = "a]\n[model]\r\u{0}".into();
    d.layers.push(layer);
    let t = listing(&d);
    assert_eq!(t.lines().filter(|l| *l == "[model]").count(), 1, "{t}");
}

fn insert(block: &str) -> cadcraft_engine::doc::EntityKind {
    use cadcraft_engine::doc::{EntityKind, Insert, geom::Vec3};
    EntityKind::Insert(Insert {
        block: block.into(),
        insert: Vec3::ZERO,
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
    })
}

/// `outer` holds a reference to `inner`, which holds a line; the model references `outer`.
/// `first` numbers the objects inside the blocks.
fn nested(outer: &str, inner: &str, first: u64) -> Drawing {
    use cadcraft_engine::doc::{Block, Common, Entity, EntityKind, Handle, Line, Space, geom::Vec3};
    use std::sync::Arc;
    let mut d = Drawing::new_imperial();
    let mut b = Block::new(inner);
    b.entities.push(Entity::new(Handle(first), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(1.0, 0.0, 0.0) })));
    d.blocks.insert(inner.into(), Arc::new(b));
    let mut b = Block::new(outer);
    b.entities.push(Entity::new(Handle(first + 1), insert(inner)));
    d.blocks.insert(outer.into(), Arc::new(b));
    d.handseed = 0x100;
    d.add(&Space::Model, Common::default(), insert(outer)).unwrap();
    d
}

#[test]
fn renumbered_anonymous_blocks_are_not_a_change() {
    let a = listing(&nested("*U1", "*U2", 0x500));
    assert!(!a.contains("*U1") && !a.contains("*U2") && a.contains("*U#"), "{a}");
    assert!(!a.contains("500 Line"), "no handles inside anonymous blocks: {a}");
    assert_eq!(a, listing(&nested("*U7", "*U3", 0x900)));
    // Different contents: a different name.
    let mut d = nested("*U1", "*U2", 0x500);
    d.blocks.remove("*U2");
    let mut b = cadcraft_engine::doc::Block::new("*U2");
    b.entities.push(cadcraft_engine::doc::Entity::new(cadcraft_engine::doc::Handle(0x500), insert("NAMED")));
    d.blocks.insert("*U2".into(), std::sync::Arc::new(b));
    assert_ne!(a, listing(&d));
}

#[test]
fn self_referencing_anonymous_blocks_terminate() {
    let d = nested("*U1", "*U1", 0x500);
    let t = listing(&d);
    assert!(t.contains("[model]"), "{t}");
}
