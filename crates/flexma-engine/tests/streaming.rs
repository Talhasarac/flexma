//! Opening a file page by page (`flexma_doc::parts::stream`) ends in the
//! same state as loading it whole. Found by Codex audits of 4e85629 and
//! f4234fa.

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

/// `server` after our next batch of edits reaches it.
fn deliver(ed: &mut Editor, server: &Document) -> Document {
    let mut server = server.clone();
    let batch: serde_json::Value = serde_json::from_str(&ed.take_outgoing().expect("a batch to send")).unwrap();
    for op in serde_json::from_value::<Vec<Op>>(batch["ops"].clone()).unwrap() {
        server.apply(&op).unwrap();
    }
    server
}

#[test]
fn reconnecting_keeps_unacknowledged_edits_on_other_pages() {
    let server = sample();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    // Offline: edit page two, then go back to page one, which reconnects first.
    ed.set_page("p:2");
    assert!(ed.apply_local(vec![Op::update1("r:2", "name", json!("offline edit"))]).is_empty());
    ed.set_page("0:1");
    open_streamed(&mut ed, &server, "new", |_| {});
    assert_eq!(ed.doc.get("r:2").unwrap().name, "offline edit");
    assert!(ed.sync.has_outgoing(), "and it's sent again");
}

#[test]
fn reconnecting_keeps_edits_to_components_on_other_pages() {
    // Components come in the first part and again with their page.
    let mut server = sample();
    add(&mut server, "c:2", NodeType::Component, "p:2");
    add(&mut server, "c:2/r", NodeType::Rectangle, "c:2");
    server.clear_fresh();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    ed.set_page("p:2");
    ed.apply_local(vec![Op::update1("c:2", "name", json!("offline edit")), Op::Delete { id: "c:2/r".into() }]);
    ed.set_page("0:1");
    open_streamed(&mut ed, &server, "new", |_| {});
    assert_eq!(ed.doc.get("c:2").unwrap().name, "offline edit");
    assert!(!ed.doc.contains("c:2/r"), "still deleted");
}

#[test]
fn reconnecting_again_while_pages_stream_in_keeps_them_too() {
    let server = sample();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    ed.set_page("p:2");
    ed.apply_local(vec![Op::update1("r:2", "name", json!("offline edit"))]);
    ed.set_page("0:1");
    // The socket drops again before page two arrives.
    let (head, _) = parts::stream(&server, "0:1");
    ed.load_first(&head, "new", true, "0:1").unwrap();
    open_streamed(&mut ed, &server, "newer", |_| {});
    assert_eq!(ed.doc.get("r:2").unwrap().name, "offline edit");
    assert!(ed.sync.has_outgoing());
}

#[test]
fn held_edits_replay_in_order_once_every_page_is_here() {
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
    let (head, rest) = parts::stream(&server, "0:1");
    ed.load_first(&head, "new", true, "0:1").unwrap();
    assert!(ed.doc.contains("source") && ed.take_outgoing().is_none(), "held until page two is here");
    for part in rest {
        ed.merge_page(&part).unwrap();
    }
    let after = deliver(&mut ed, &server);
    assert_eq!((ed.doc.parent_of("rect"), after.parent_of("rect")), (Some("destination"), Some("destination")));
    assert!(!ed.doc.contains("source") && !after.contains("source"));
}

#[test]
fn edits_made_while_pages_arrive_go_out_after_held_ones() {
    let mut server = sample();
    add(&mut server, "rect", NodeType::Rectangle, "0:1");
    add(&mut server, "destination", NodeType::Frame, "p:2");
    server.clear_fresh();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    // Sent before the socket dropped, never acknowledged.
    ed.apply_local(vec![Op::update("rect", json!({ "parent": "destination", "name": "older" }).as_object().unwrap().clone())]);
    ed.take_outgoing().unwrap();
    open_streamed(&mut ed, &server, "new", |ed| {
        ed.apply_local(vec![Op::update1("rect", "name", json!("newer"))]);
    });
    let after = deliver(&mut ed, &server);
    assert_eq!((ed.doc.get("rect").unwrap().name.as_str(), after.get("rect").unwrap().name.as_str()), ("newer", "newer"));
    assert_eq!(after.parent_of("rect"), Some("destination"));
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
        assert!(ed.take_outgoing().is_some());
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
    ed.load(&server.to_json(), "old", true).unwrap();
    ed.apply_local(vec![Op::Delete { id: "c:2/r".into() }]);
    ed.undo();
    open_streamed(&mut ed, &server, "new", |_| {});
    assert!(ed.doc.contains("c:2/r"));
    assert!(deliver(&mut ed, &server).contains("c:2/r"));
}

#[test]
fn a_page_added_offline_is_not_waited_for() {
    let server = painted();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    let offline = ed.add_page("Offline page");
    let state = json!({ "seq": 0, "layers": { "paint:2": { "tiles": [], "strokes": [] } } });
    open_streamed(&mut ed, &server, "new", |ed| ed.load_paint_state(state.clone()));
    assert!(ed.pending_pages.is_empty() && ed.waiting_paint.is_none(), "{:?}", ed.pending_pages);
    assert!(ed.doc.contains(&offline) && deliver(&mut ed, &server).contains(&offline));
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

fn painted() -> Document {
    let mut doc = sample();
    add(&mut doc, "paint:2", NodeType::Paint, "p:2");
    doc.apply(&Op::update("paint:2", json!({ "width": 256.0, "height": 256.0 }).as_object().unwrap().clone())).unwrap();
    doc
}

#[test]
fn paint_on_a_later_page_survives_arriving_first() {
    let server = painted();
    let stroke: flexma_doc::raster::Stroke = serde_json::from_value(json!({ "id": "saved-stroke", "layer": "paint:2", "brush": {}, "points": [[50.0, 50.0, 1.0]] })).unwrap();
    let state = json!({ "seq": 1, "layers": { "paint:2": { "tiles": [], "strokes": [{ "seq": 1, "stroke": stroke, "undone": false }] } } });
    let mut ed = Editor::new(None, None);
    // The paint state's request returns before page two's frame.
    open_streamed(&mut ed, &server, "client", |ed| ed.load_paint_state(state.clone()));
    assert!(ed.paint.has_layer("paint:2"));
    assert!(ed.paint.undo("saved-stroke", 0.0), "the saved stroke is there");
}

#[test]
fn unacknowledged_strokes_on_a_later_page_are_sent_again() {
    let server = painted();
    let state = json!({ "seq": 0, "layers": { "paint:2": { "tiles": [], "strokes": [] } } });
    let stroke: flexma_doc::raster::Stroke = serde_json::from_value(json!({ "id": "offline-stroke", "layer": "paint:2", "brush": {}, "points": [[50.0, 50.0, 1.0]] })).unwrap();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "old", true).unwrap();
    ed.load_paint_state(state.clone());
    ed.paint.begin(stroke, 0.0);
    assert_eq!(ed.paint.end().as_deref(), Some("offline-stroke"));
    ed.paint.take_outgoing(); // sent, never acknowledged
    open_streamed(&mut ed, &server, "new", |ed| ed.load_paint_state(state.clone()));
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
