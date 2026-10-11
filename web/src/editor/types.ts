// Shapes of the JSON snapshots the engine hands to the UI.

export type NodeType =
  | 'DOCUMENT'
  | 'PAGE'
  | 'FRAME'
  | 'GROUP'
  | 'RECTANGLE'
  | 'ELLIPSE'
  | 'LINE'
  | 'TEXT'
  | 'VECTOR'
  | 'POLYGON'
  | 'STAR';

export type Tool = 'select' | 'hand' | 'frame' | 'rectangle' | 'ellipse' | 'line' | 'text' | 'pen' | 'polygon' | 'star';

export interface LayerNode {
  id: string;
  name: string;
  type: NodeType;
  visible: boolean;
  locked: boolean;
  layout: boolean;
  children: LayerNode[];
}

export interface PageInfo {
  id: string;
  name: string;
}

export interface GradientStop {
  position: number;
  color: string;
}

export type Paint =
  | { type: 'SOLID'; color: string; opacity?: number; visible?: boolean }
  | { type: 'LINEAR_GRADIENT'; stops: GradientStop[]; angle?: number; opacity?: number; visible?: boolean }
  | { type: 'IMAGE'; asset: string; fit?: 'FILL' | 'FIT' | 'STRETCH'; opacity?: number; visible?: boolean };

export type EffectValue =
  | { type: 'DROP_SHADOW'; color: string; x: number; y: number; blur: number; spread: number; visible?: boolean }
  | { type: 'LAYER_BLUR'; radius: number; visible?: boolean };

export interface Padding {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/** Full node JSON (every property present) as returned by `selection()`. */
export interface NodeData {
  id: string;
  type: NodeType;
  name: string;
  parent: string | null;
  x: number;
  y: number;
  width: number;
  height: number;
  rotation: number;
  flipX: boolean;
  flipY: boolean;
  visible: boolean;
  locked: boolean;
  opacity: number;
  fills: Paint[];
  strokes: Paint[];
  strokeWeight: number;
  strokeAlign: 'CENTER' | 'INSIDE' | 'OUTSIDE';
  cornerRadius: number;
  cornerRadii: [number, number, number, number] | null;
  effects: EffectValue[];
  clipsContent: boolean;
  characters: string;
  fontFamily: string;
  fontSize: number;
  fontWeight: number;
  italic: boolean;
  textAlign: 'LEFT' | 'CENTER' | 'RIGHT';
  textAlignVertical: 'TOP' | 'CENTER' | 'BOTTOM';
  lineHeight: number | null;
  letterSpacing: number;
  textAutoResize: 'WIDTH_AND_HEIGHT' | 'HEIGHT' | 'NONE';
  textDecoration: string | null;
  pathData: string;
  pointCount: number;
  innerRadius: number;
  layoutMode: 'NONE' | 'HORIZONTAL' | 'VERTICAL';
  itemSpacing: number;
  padding: Padding;
  primaryAxisAlign: 'START' | 'CENTER' | 'END' | 'SPACE_BETWEEN';
  counterAxisAlign: 'START' | 'CENTER' | 'END' | 'SPACE_BETWEEN';
  layoutSizingHorizontal: 'FIXED' | 'HUG' | 'FILL';
  layoutSizingVertical: 'FIXED' | 'HUG' | 'FILL';
  layoutAbsolute: boolean;
  description: string;
  parentType: NodeType | null;
  parentLayout: 'NONE' | 'HORIZONTAL' | 'VERTICAL';
}

export interface Peer {
  clientId: string;
  userId: string;
  name: string;
  color: string;
  cursor: [number, number] | null;
  selection: string[];
  page: string | null;
  isAgent: boolean;
}

export interface EditingText {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  rotation: number;
  zoom: number;
  characters: string;
  fontFamily: string;
  fontSize: number;
  fontWeight: number;
  italic: boolean;
  lineHeight: number;
  letterSpacing: number;
  textAlign: 'LEFT' | 'CENTER' | 'RIGHT';
  autoResize: 'WIDTH_AND_HEIGHT' | 'HEIGHT' | 'NONE';
  color: string;
}

export type ConnectionStatus = 'connecting' | 'online' | 'offline';
