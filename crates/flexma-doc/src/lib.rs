//! # flexma-doc
//!
//! The Flexma document model, shared by the server (native) and the editor
//! engine (WebAssembly):
//!
//! - [`node`]: the node type with JSON-addressable properties
//! - [`doc`]: the document tree
//! - [`ops`]: create/update/delete operations with LWW-per-property semantics
//! - [`fractional`]: fractional indexing for sibling order
//! - [`layout`]: deterministic auto layout
//! - [`svg`]: SVG export
//! - [`friendly`]: forgiving node specs for AI agents (MCP)

pub mod color;
pub mod components;
pub mod doc;
pub mod fractional;
pub mod friendly;
pub mod geom;
pub mod layout;
pub mod node;
pub mod ops;
pub mod paint;
pub mod path;
pub mod svg;
pub mod text;

pub use color::Color;
pub use doc::{DocSnapshot, Document};
pub use geom::{Affine, Rect, Vec2};
pub use node::{Node, NodeId, NodeType, ROOT_ID};
pub use ops::{Op, OpEffect};
pub use paint::*;
