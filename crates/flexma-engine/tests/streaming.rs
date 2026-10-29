//! Opening a file page by page (`flexma_doc::parts::stream`) ends in the
//! same state as loading it whole, and our edits reach the server as made.
//! Found by Codex audits of 4e85629, f4234fa and 2567922.

use flexma_doc::{parts, Document, Node, NodeType, Op, ROOT_ID};
use flexma_engine::editor::Editor;
use serde_json::json;

fn add(doc: &mut Document, id: &str, kind: NodeType, parent: &str) {
    let mut node = Node::new(id, kind);
    node.parent = Some(parent.into());
    node.index = doc.index_at_end(parent);
    doc.apply(&Op::Create { node }).unwrap();
}

/// Page one is empty; page two has a rectangle.
fn sample() -> Document {
    let mut doc = Document::new_empty();
    add(&mut doc, "p:2", NodeType::Page, ROOT_ID);
    add(&mut doc, "r:2", NodeType::Rectangle, "p:2");
    doc.clear_fresh();
    doc
}

fn update(doc: &mut Document, node: Node) {
    let props = node.to_json().as_object().unwrap().iter().filter(|(k, _)| k.as_str() != "id" && k.as_str() != "type").map(|(k, v)| (k.clone(), v.clone())).collect();
    doc.apply(&Op::Update { id: node.id.clone(), props }).unwrap();
}

/// Open `doc` on page one, streaming page two in after `between` ran.
fn open_streamed(ed: &mut Editor, doc: &Document, client: &str, between: impl FnOnce(&mut Editor)) {
    let (head, rest) = parts::stream(doc, "0:1");
    ed.load_first(&head, client, true, "0:1").unwrap();
    between(ed);
    for part in rest {
        ed.merge_page(&part).unwrap();
    }
}

/// `server` after everything we have to send reaches it.
fn deliver(ed: &mut Editor, server: &Document) -> Document {
    let mut server = server.clone();
    while let Some(batch) = ed.sync.flush() {
        let batch: serde_json::Value = serde_json::from_str(&batch).unwrap();
        for op in serde_json::from_value::<Vec<Op>>(batch["ops"].clone()).unwrap() {
            server.apply(&op).unwrap();
        }
    }
    server
}

#[test]
fn reconnecting_with_unacknowledged_edits_loads_the_whole_file() {
    let mut server = sample();
    add(&mut server, "c:2", NodeType::Component, "p:2");
    add(&mut server, "c:2/r", NodeType::Rectangle, "c:2");
    server.clear_fresh();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    // Offline: edit page two and a component there, then go back to page one.
    ed.set_page("p:2");
    ed.apply_local(vec![Op::update1("r:2", "name", json!("offline edit")), Op::update1("c:2", "name", json!("offline edit")), Op::Delete { id: "c:2/r".into() }]);
    ed.set_page("0:1");
    // Streamed, they'd be replayed on part of the file.
    let (head, _) = parts::stream(&server, "0:1");
    assert!(ed.load_first(&head, "new", true, "0:1").is_err());
    assert!(ed.doc.contains("r:2") && ed.has_unacked(), "left as it was");
    ed.load(&server.to_json(), "new", true).unwrap();
    let after = deliver(&mut ed, &server);
    for doc in [&ed.doc, &after] {
        assert_eq!((doc.get("r:2").unwrap().name.as_str(), doc.get("c:2").unwrap().name.as_str()), ("offline edit", "offline edit"));
        assert!(!doc.contains("c:2/r"));
    }
}

#[test]
fn reconnecting_while_pages_stream_in_loads_the_whole_file() {
    let server = sample();
    let mut ed = Editor::new(None, None);
    let (head, _) = parts::stream(&server, "0:1");
    ed.load_first(&head, "client", true, "0:1").unwrap();
    // An edit while page two is on its way, then the socket drops.
    ed.apply_local(vec![Op::update1("0:1", "name", json!("edited"))]);
    ed.sync.flush();
    assert!(ed.has_unacked());
    ed.load(&server.to_json(), "again", true).unwrap();
    assert!(ed.pending_pages.is_empty() && ed.doc.contains("r:2"));
    assert_eq!(ed.doc.get("0:1").unwrap().name, "edited");
    assert_eq!(deliver(&mut ed, &server).get("0:1").unwrap().name, "edited");
}

#[test]
fn a_reconnect_replays_edits_in_order() {
    let mut server = sample();
    add(&mut server, "source", NodeType::Frame, "0:1");
    add(&mut server, "rect", NodeType::Rectangle, "source");
    add(&mut server, "destination", NodeType::Frame, "p:2");
    server.clear_fresh();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    // Offline: move the rectangle to page two, then delete its old frame.
    ed.apply_local(vec![Op::update1("rect", "parent", json!("destination"))]);
    ed.apply_local(vec![Op::Delete { id: "source".into() }]);
    ed.load(&server.to_json(), "new", true).unwrap();
    let after = deliver(&mut ed, &server);
    assert_eq!((ed.doc.parent_of("rect"), after.parent_of("rect")), (Some("destination"), Some("destination")));
    assert!(!ed.doc.contains("source") && !after.contains("source"));
}

#[test]
fn updates_are_not_merged_across_a_delete() {
    let mut server = sample();
    add(&mut server, "frame", NodeType::Frame, "0:1");
    add(&mut server, "rect", NodeType::Rectangle, "frame");
    server.clear_fresh();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    // Move the rectangle out, delete the frame, undo that, move it back:
    // each sent on its own, none acknowledged before the socket dropped.
    ed.apply_local(vec![Op::update1("rect", "parent", json!("0:1"))]);
    ed.sync.flush();
    ed.apply_local(vec![Op::Delete { id: "frame".into() }]);
    ed.sync.flush();
    ed.undo();
    ed.sync.flush();
    ed.apply_local(vec![Op::update1("rect", "parent", json!("frame"))]);
    ed.sync.flush();
    // The replay queues them as one batch.
    ed.load(&server.to_json(), "new", true).unwrap();
    let after = deliver(&mut ed, &server);
    assert_eq!((ed.doc.parent_of("rect"), after.parent_of("rect")), (Some("frame"), Some("frame")));
}

#[test]
fn edits_sent_while_pages_arrive_survive_their_page_frame() {
    let mut server = sample();
    add(&mut server, "c:2", NodeType::Component, "p:2");
    server.clear_fresh();
    let mut ed = Editor::new(None, None);
    // Page two's frame repeats the component as the server had it.
    open_streamed(&mut ed, &server, "client", |ed| {
        ed.apply_local(vec![Op::update1("c:2", "name", json!("renamed"))]);
        assert!(ed.sync.flush().is_some());
    });
    assert_eq!(ed.doc.get("c:2").unwrap().name, "renamed");
}

#[test]
fn undoing_a_component_delete_survives_its_page_frame() {
    let mut server = sample();
    add(&mut server, "c:2", NodeType::Component, "p:2");
    add(&mut server, "c:2/r", NodeType::Rectangle, "c:2");
    server.clear_fresh();
    let mut ed = Editor::new(None, None);
    open_streamed(&mut ed, &server, "client", |ed| {
        ed.apply_local(vec![Op::Delete { id: "c:2/r".into() }]);
        ed.undo();
    });
    assert!(ed.doc.contains("c:2/r"));
    assert!(deliver(&mut ed, &server).contains("c:2/r"));
}

#[test]
fn a_page_added_while_pages_arrive_is_not_waited_for() {
    let server = painted();
    let state = json!({ "seq": 0, "layers": { "paint:2": { "tiles": [], "strokes": [] } } });
    let mut ed = Editor::new(None, None);
    let mut added = String::new();
    open_streamed(&mut ed, &server, "client", |ed| {
        added = ed.add_page("New page");
        ed.load_paint_state(state.clone());
    });
    assert!(ed.pending_pages.is_empty() && ed.waiting_paint.is_none(), "{:?}", ed.pending_pages);
    assert!(ed.doc.contains(&added) && deliver(&mut ed, &server).contains(&added));
}

#[test]
fn components_keep_their_place_when_their_page_streams_in() {
    use flexma_doc::paint::{ConstraintType, LayoutMode, SizingMode};
    let mut server = sample();
    // A hugging frame on page two holds a tall layer and a bottom-anchored
    // component; the first part has the frame without the tall layer.
    add(&mut server, "f:2", NodeType::Frame, "p:2");
    add(&mut server, "c:2", NodeType::Component, "f:2");
    add(&mut server, "flow:2", NodeType::Rectangle, "f:2");
    let mut f = server.get("f:2").unwrap().clone();
    (f.layout_mode, f.layout_sizing_vertical, f.height) = (LayoutMode::Vertical, SizingMode::Hug, 300.0);
    update(&mut server, f);
    let mut c = server.get("c:2").unwrap().clone();
    (c.layout_absolute, c.constraints.vertical, c.y, c.height) = (true, ConstraintType::Max, 250.0, 50.0);
    update(&mut server, c);
    let mut flow = server.get("flow:2").unwrap().clone();
    flow.height = 300.0;
    update(&mut server, flow);
    server.clear_fresh();
    flexma_doc::layout::relayout_all(&mut server);
    assert_eq!(server.get("c:2").unwrap().y, 250.0);
    let mut ed = Editor::new(None, None);
    open_streamed(&mut ed, &server, "client", |_| {});
    assert_eq!(ed.doc.get("c:2").unwrap().y, 250.0, "the same geometry as the server's document");
}

/// Paint layers on both pages.
fn painted() -> Document {
    let mut doc = sample();
    add(&mut doc, "paint:1", NodeType::Paint, "0:1");
    add(&mut doc, "paint:2", NodeType::Paint, "p:2");
    doc.apply(&Op::update("paint:2", json!({ "width": 256.0, "height": 256.0 }).as_object().unwrap().clone())).unwrap();
    doc.clear_fresh();
    doc
}

/// The editor draws between page frames: paint keeps the layers it saw.
fn render(ed: &mut Editor) {
    ed.paint.sync_layers(&ed.doc, ed.doc_version);
}

#[test]
fn paint_on_a_later_page_survives_arriving_first() {
    let server = painted();
    let stroke: flexma_doc::raster::Stroke = serde_json::from_value(json!({ "id": "saved-stroke", "layer": "paint:2", "brush": {}, "points": [[50.0, 50.0, 1.0]] })).unwrap();
    let state = json!({ "seq": 1, "layers": { "paint:2": { "tiles": [], "strokes": [{ "seq": 1, "stroke": stroke, "undone": false }] } } });
    let mut ed = Editor::new(None, None);
    // The paint state's request returns before page two's frame.
    open_streamed(&mut ed, &server, "client", |ed| {
        render(ed);
        ed.load_paint_state(state.clone());
    });
    assert!(ed.paint.has_layer("paint:2"));
    assert!(ed.paint.undo("saved-stroke", 0.0), "the saved stroke is there");
}

#[test]
fn unacknowledged_strokes_on_a_later_page_are_sent_again() {
    let server = painted();
    let state = json!({ "seq": 0, "layers": { "paint:1": { "tiles": [], "strokes": [] }, "paint:2": { "tiles": [], "strokes": [] } } });
    let stroke: flexma_doc::raster::Stroke = serde_json::from_value(json!({ "id": "offline-stroke", "layer": "paint:2", "brush": {}, "points": [[50.0, 50.0, 1.0]] })).unwrap();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    ed.load_paint_state(state.clone());
    ed.paint.begin(stroke, 0.0);
    assert_eq!(ed.paint.end().as_deref(), Some("offline-stroke"));
    ed.paint.take_outgoing(); // sent, never acknowledged
    assert!(!ed.has_unacked(), "only paint: the pages stream in");
    open_streamed(&mut ed, &server, "new", |ed| {
        render(ed);
        ed.load_paint_state(state.clone());
    });
    assert!(ed.paint.take_outgoing().iter().any(|m| m.contains("offline-stroke")));
}

#[test]
fn narrowed_instance_text_is_measured_on_load() {
    use flexma_doc::paint::{LayoutMode, SizingMode, TextAutoResize};
    let mut doc = Document::new_empty();
    let mut component = Node::new("c:1", NodeType::Component);
    component.parent = Some("0:1".into());
    (component.width, component.layout_mode, component.layout_sizing_vertical) = (400.0, LayoutMode::Vertical, SizingMode::Hug);
    doc.apply(&Op::Create { node: component }).unwrap();
    let mut text = Node::new("t:1", NodeType::Text);
    text.parent = Some("c:1".into());
    text.characters = "A paragraph that wraps onto more lines when its column gets narrower".into();
    (text.font_size, text.line_height, text.width, text.height) = (14.0, Some(20.0), 400.0, 40.0);
    (text.measured_width, text.text_auto_resize, text.layout_sizing_horizontal) = (Some(400.0), TextAutoResize::Height, SizingMode::Fill);
    doc.apply(&Op::Create { node: text }).unwrap();
    let mut instance = Node::new("i:1", NodeType::Instance);
    instance.parent = Some("0:1".into());
    (instance.component_id, instance.width) = (Some("c:1".into()), 150.0);
    doc.apply(&Op::Create { node: instance }).unwrap();
    let mut ed = Editor::new(None, None);
    ed.load(&doc.to_json(), "client", true).unwrap();
    let text = ed.doc.get("i:1~t:1").unwrap();
    assert!(text.needs_measure && text.width == 150.0 && text.height > 40.0);
    assert!(ed.needs_measure.contains("i:1~t:1"), "queued for the browser to measure");
}

#[test]
fn text_on_a_later_page_is_queued_for_measuring() {
    use flexma_doc::paint::{LayoutMode, SizingMode, StyleType, TextAutoResize};
    let mut head = sample();
    let mut style = Node::new("s:1", NodeType::Style);
    style.parent = Some(ROOT_ID.into());
    (style.style_type, style.font_size) = (StyleType::Text, 30.0);
    head.apply(&Op::Create { node: style }).unwrap();
    let mut instance = Node::new("i:1", NodeType::Instance);
    instance.parent = Some("0:1".into());
    (instance.component_id, instance.width) = (Some("c:2".into()), 100.0);
    head.apply(&Op::Create { node: instance }).unwrap();
    // Page two brings the instance's component and a styled text.
    let mut component = Node::new("c:2", NodeType::Component);
    component.parent = Some("p:2".into());
    (component.width, component.layout_mode, component.layout_sizing_vertical) = (400.0, LayoutMode::Vertical, SizingMode::Hug);
    let mut text = Node::new("t:2", NodeType::Text);
    text.parent = Some("c:2".into());
    text.characters = "Text that wraps inside a narrower instance".into();
    (text.width, text.height, text.measured_width) = (400.0, 40.0, Some(400.0));
    (text.text_auto_resize, text.layout_sizing_horizontal) = (TextAutoResize::Height, SizingMode::Fill);
    let mut styled = Node::new("styled:2", NodeType::Text);
    styled.parent = Some("p:2".into());
    (styled.characters, styled.text_style_id) = ("Styled".into(), Some("s:1".into()));
    let mut ed = Editor::new(None, None);
    ed.load_first(&head.to_json(), "client", true, "0:1").unwrap();
    ed.merge_page(&json!({ "page": "p:2", "nodes": [component, text, styled] }).to_string()).unwrap();
    for id in ["i:1~t:2", "styled:2"] {
        assert!(ed.doc.get(id).unwrap().needs_measure && ed.needs_measure.contains(id), "{id}");
    }
}
