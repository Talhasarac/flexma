//! Keyboard shortcuts (Figma-compatible where it matters).

use crate::editor::Editor;
use crate::tools::{Gesture, Mods, Tool};
use flexma_doc::{NodeType, Vec2};

impl Editor {
    /// Returns true when the key was handled (the UI should preventDefault).
    pub fn key_down(&mut self, key: &str, mods: Mods) -> bool {
        if self.editing_text.is_some() {
            if key == "Escape" {
                self.end_text_edit();
                return true;
            }
            return false;
        }
        let k = key.to_ascii_lowercase();
        let m = mods.meta;
        match (k.as_str(), m, mods.shift, mods.alt) {
            ("shift" | "alt" | "meta" | "control", _, _, _) => {
                self.modifiers_changed(mods);
                return false;
            }
            (" ", false, _, _) => {
                self.space_down = true;
                self.ui_version += 1;
                return true;
            }
            ("escape", _, _, _) => {
                match self.gesture {
                    Gesture::Pen { .. } => self.finish_pen(),
                    Gesture::None => {
                        if self.tool != Tool::Select {
                            self.set_tool(Tool::Select);
                        } else {
                            self.select_parent();
                        }
                    }
                    _ => {
                        self.gesture = Gesture::None;
                        if self.history.is_open() {
                            self.end();
                        }
                    }
                }
                self.dirty = true;
                return true;
            }
            ("enter", false, shift, _) => {
                if matches!(self.gesture, Gesture::Pen { .. }) {
                    self.finish_pen();
                } else if self.selection.len() == 1 && self.doc.get(&self.selection[0]).map(|n| n.kind == NodeType::Text).unwrap_or(false) {
                    let id = self.selection[0].clone();
                    self.start_text_edit(&id);
                } else if shift {
                    self.select_parent();
                } else {
                    self.select_children();
                }
                return true;
            }
            ("backspace" | "delete", _, _, _) => {
                self.delete_selection();
                return true;
            }
            ("arrowleft" | "arrowright" | "arrowup" | "arrowdown", false, shift, _) => {
                let step = if shift { 10.0 } else { 1.0 };
                let (dx, dy) = match k.as_str() {
                    "arrowleft" => (-step, 0.0),
                    "arrowright" => (step, 0.0),
                    "arrowup" => (0.0, -step),
                    _ => (0.0, step),
                };
                self.nudge(dx, dy);
                return true;
            }
            ("z", true, false, _) => self.undo(),
            ("z", true, true, _) | ("y", true, _, _) => self.redo(),
            ("a", true, false, _) => self.select_all(),
            ("d", true, _, _) => self.duplicate(),
            ("g", true, false, false) => {
                self.wrap_selection(NodeType::Group, false);
            }
            ("g", true, true, _) => self.ungroup(),
            ("g", true, false, true) => {
                self.wrap_selection(NodeType::Frame, false);
            }
            ("]", true, _, true) | ("]", true, true, _) => self.reorder("front"),
            ("[", true, _, true) | ("[", true, true, _) => self.reorder("back"),
            ("]", true, _, _) => self.reorder("forward"),
            ("[", true, _, _) => self.reorder("backward"),
            ("h", true, true, _) => {
                let ids = self.top_selection();
                self.toggle_prop(&ids, "visible");
            }
            ("l", true, true, _) => {
                let ids = self.top_selection();
                self.toggle_prop(&ids, "locked");
            }
            ("h", false, true, _) => self.flip(true),
            ("v", false, true, _) => self.flip(false),
            ("a", false, true, _) => self.add_auto_layout(),
            ("!" | "1", false, true, _) => self.zoom_to_fit(false),
            ("@" | "2", false, true, _) => self.zoom_to_fit(true),
            (")" | "0", false, true, _) | ("0", true, false, _) => {
                let c = Vec2::new(self.css_w / 2.0, self.css_h / 2.0);
                self.cam.set_zoom_at(1.0, c);
                self.camera_changed();
            }
            ("=" | "+", _, _, _) => {
                let c = Vec2::new(self.css_w / 2.0, self.css_h / 2.0);
                let z = self.cam.step(true);
                self.cam.set_zoom_at(z, c);
                self.camera_changed();
            }
            ("-" | "_", _, _, _) => {
                let c = Vec2::new(self.css_w / 2.0, self.css_h / 2.0);
                let z = self.cam.step(false);
                self.cam.set_zoom_at(z, c);
                self.camera_changed();
            }
            (_, false, false, false) => {
                let tool = match k.as_str() {
                    "v" => Some(Tool::Select),
                    "h" => Some(Tool::Hand),
                    "f" | "a" => Some(Tool::Frame),
                    "r" => Some(Tool::Rectangle),
                    "o" => Some(Tool::Ellipse),
                    "l" => Some(Tool::Line),
                    "t" => Some(Tool::Text),
                    "p" => Some(Tool::Pen),
                    _ => None,
                };
                match tool {
                    Some(t) if self.can_edit || t == Tool::Select || t == Tool::Hand => {
                        self.set_tool(t);
                        return true;
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }

    pub fn key_up(&mut self, key: &str, mods: Mods) {
        match key {
            " " => {
                self.space_down = false;
                if let Gesture::Pan { .. } = self.gesture {
                    self.gesture = Gesture::None;
                }
                self.ui_version += 1;
            }
            "Shift" | "Alt" | "Meta" | "Control" => self.modifiers_changed(mods),
            _ => {}
        }
    }
}
