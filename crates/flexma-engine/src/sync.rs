//! Client side of multiplayer: coalesces local ops into batches, tracks which
//! properties have unacknowledged local changes, and filters incoming remote
//! changes so they never clobber in-flight local edits (Figma's model).

use flexma_doc::{NodeId, Op};
use std::collections::{HashMap, VecDeque};

const ALL: &str = "*";

#[derive(Default)]
pub struct Sync {
    pub client_id: String,
    next_batch: u64,
    outbox: Vec<Op>,
    inflight: VecDeque<(u64, Vec<(NodeId, String)>, Vec<Op>)>,
    pending: HashMap<(NodeId, String), u32>,
}

impl Sync {
    pub fn new(client_id: &str) -> Sync {
        Sync { client_id: client_id.to_string(), next_batch: 1, ..Default::default() }
    }

    /// Queue a local op, merging consecutive updates of the same node so a 60 Hz
    /// drag sends one update per flush.
    pub fn push(&mut self, op: Op) {
        if let Op::Update { id, props } = &op {
            for prev in self.outbox.iter_mut().rev() {
                match prev {
                    Op::Update { id: pid, props: pprops } if pid == id => {
                        for (k, v) in props {
                            pprops.insert(k.clone(), v.clone());
                        }
                        return;
                    }
                    Op::Create { node } if &node.id == id => break,
                    Op::Delete { id: did } if did == id => break,
                    _ => {}
                }
            }
        }
        self.outbox.push(op);
    }

    pub fn has_outgoing(&self) -> bool {
        !self.outbox.is_empty()
    }

    pub fn inflight_count(&self) -> usize {
        self.inflight.len()
    }

    /// Take the outbox as a wire message, marking its properties pending.
    pub fn flush(&mut self) -> Option<String> {
        if self.outbox.is_empty() {
            return None;
        }
        let ops = std::mem::take(&mut self.outbox);
        let batch = self.next_batch;
        self.next_batch += 1;
        let mut keys = Vec::new();
        for op in &ops {
            match op {
                Op::Create { node } => keys.push((node.id.clone(), ALL.to_string())),
                Op::Update { id, props } => {
                    for k in props.keys() {
                        keys.push((id.clone(), k.clone()));
                    }
                }
                Op::Delete { id } => keys.push((id.clone(), ALL.to_string())),
            }
        }
        for k in &keys {
            *self.pending.entry(k.clone()).or_insert(0) += 1;
        }
        let msg = serde_json::json!({ "t": "ops", "batch": batch, "ops": ops }).to_string();
        self.inflight.push_back((batch, keys, ops));
        Some(msg)
    }

    /// Our batch came back from the server.
    pub fn ack(&mut self, batch: u64) {
        while let Some((b, keys, _)) = self.inflight.front() {
            if *b > batch {
                break;
            }
            for k in keys.clone() {
                if let Some(c) = self.pending.get_mut(&k) {
                    *c -= 1;
                    if *c == 0 {
                        self.pending.remove(&k);
                    }
                }
            }
            self.inflight.pop_front();
        }
    }

    fn is_pending(&self, id: &str, key: &str) -> bool {
        self.pending.contains_key(&(id.to_string(), key.to_string()))
            || self.pending.contains_key(&(id.to_string(), ALL.to_string()))
    }

    /// Drop the parts of a remote op that conflict with unacknowledged local edits.
    pub fn filter_remote(&self, op: Op) -> Option<Op> {
        match op {
            Op::Update { id, mut props } => {
                props.retain(|k, _| !self.is_pending(&id, k));
                if props.is_empty() {
                    None
                } else {
                    Some(Op::Update { id, props })
                }
            }
            Op::Create { node } => {
                if self.is_pending(&node.id, ALL) {
                    None
                } else {
                    Some(Op::Create { node })
                }
            }
            other => Some(other),
        }
    }

    /// Every local op not yet acknowledged, oldest first.
    pub fn unacked(&self) -> impl Iterator<Item = &Op> {
        self.inflight.iter().flat_map(|(_, _, ops)| ops).chain(&self.outbox)
    }

    /// After a reconnect: everything not yet acknowledged, to be re-applied on
    /// top of the fresh server snapshot and resent.
    pub fn take_unacked(&mut self) -> Vec<Op> {
        let mut ops: Vec<Op> = self.inflight.drain(..).flat_map(|(_, _, ops)| ops).collect();
        ops.append(&mut self.outbox);
        self.pending.clear();
        ops
    }

    pub fn reset(&mut self, client_id: &str) {
        self.client_id = client_id.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn coalesces_and_filters() {
        let mut s = Sync::new("c1");
        s.push(Op::update1("n", "x", json!(1)));
        s.push(Op::update1("n", "x", json!(2)));
        s.push(Op::update1("n", "y", json!(3)));
        let msg = s.flush().unwrap();
        assert!(msg.contains("\"x\":2"));
        assert_eq!(s.inflight_count(), 1);
        let remote = Op::Update { id: "n".into(), props: json!({ "x": 10, "name": "a" }).as_object().unwrap().clone() };
        match s.filter_remote(remote).unwrap() {
            Op::Update { props, .. } => {
                assert!(props.get("x").is_none());
                assert!(props.get("name").is_some());
            }
            _ => panic!(),
        }
        s.ack(1);
        let remote = Op::update1("n", "x", json!(10));
        assert!(s.filter_remote(remote).is_some());
    }
}
