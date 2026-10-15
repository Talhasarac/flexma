//! Undo/redo of local changes. Each transaction stores the inverse ops captured
//! just before its ops were applied; a whole drag is one transaction, so it is
//! one undo step. Undo applies inverses as new ops (multiplayer-safe).

use flexma_doc::Op;

#[derive(Default, Clone)]
pub struct Txn {
    pub label: String,
    /// Inverse ops in application order (apply them reversed to undo).
    pub inverse: Vec<Op>,
    /// Selection before the transaction, restored on undo.
    pub selection: Vec<String>,
    /// A brush stroke (by id) instead of document ops.
    pub stroke: Option<String>,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Txn>,
    redo: Vec<Txn>,
    open: Option<Txn>,
    depth: u32,
}

const MAX_STEPS: usize = 200;

impl History {
    pub fn begin(&mut self, label: &str, selection: &[String]) {
        self.depth += 1;
        if self.open.is_none() {
            self.open = Some(Txn { label: label.to_string(), inverse: vec![], selection: selection.to_vec(), stroke: None });
        }
    }

    pub fn end(&mut self) {
        if self.depth == 0 {
            return;
        }
        self.depth -= 1;
        if self.depth == 0 {
            if let Some(t) = self.open.take() {
                if !t.inverse.is_empty() {
                    self.undo.push(t);
                    if self.undo.len() > MAX_STEPS {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Record inverse ops for an op about to be applied.
    pub fn record(&mut self, inverse: Vec<Op>) {
        if let Some(t) = self.open.as_mut() {
            t.inverse.extend(inverse);
        }
    }

    pub fn pop_undo(&mut self) -> Option<Txn> {
        self.undo.pop()
    }

    pub fn pop_redo(&mut self) -> Option<Txn> {
        self.redo.pop()
    }

    pub fn push_redo(&mut self, t: Txn) {
        self.redo.push(t);
    }

    /// A finished brush stroke is one undo step.
    pub fn push_stroke(&mut self, id: String, selection: &[String]) {
        self.undo.push(Txn { label: "Paint".into(), inverse: vec![], selection: selection.to_vec(), stroke: Some(id) });
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn push_undo_keep_redo(&mut self, t: Txn) {
        self.undo.push(t);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
