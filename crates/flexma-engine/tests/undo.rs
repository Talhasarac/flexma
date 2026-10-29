//! Undo and redo of local edits, as the server ends up with them too.

use flexma_doc::{Document, Node, NodeType, Op};
use flexma_engine::editor::Editor;

fn add(doc: &mut Document, id: &str, kind: NodeType, parent: &str) {
    let mut node = Node::new(id, kind);
    node.parent = Some(parent.into());
    node.index = doc.index_at_end(parent);
    doc.apply(&Op::Create { node }).unwrap();
}

/// `server` after every edit we haven't sent yet reaches it.
fn deliver(ed: &mut Editor, server: &mut Document) {
    while let Some(batch) = ed.take_outgoing() {
        let batch: serde_json::Value = serde_json::from_str(&batch).unwrap();
        for op in serde_json::from_value::<Vec<Op>>(batch["ops"].clone()).unwrap() {
            server.apply(&op).unwrap();
        }
    }
}

#[test]
fn undoing_a_delete_brings_back_everything_inside() {
    let mut server = Document::new_empty();
    add(&mut server, "frame", NodeType::Frame, "0:1");
    add(&mut server, "group", NodeType::Group, "frame");
    add(&mut server, "rect", NodeType::Rectangle, "group");
    add(&mut server, "text", NodeType::Text, "frame");
    server.clear_fresh();
    let mut ed = Editor::new(None, None);
    ed.load(&server.to_json(), "client", true).unwrap();
    let ids = ["frame", "group", "rect", "text"];
    let present = |doc: &Document| ids.map(|id| doc.contains(id));
    ed.set_selection(vec!["frame".into()]);
    ed.delete_selection();
    deliver(&mut ed, &mut server);
    assert_eq!(present(&server), [false; 4]);
    ed.undo();
    deliver(&mut ed, &mut server);
    assert_eq!((present(&ed.doc), present(&server)), ([true; 4], [true; 4]));
    assert_eq!(ed.doc.parent_of("rect"), Some("group"));
    ed.redo();
    ed.undo();
    deliver(&mut ed, &mut server);
    assert_eq!((present(&ed.doc), present(&server)), ([true; 4], [true; 4]));
}
