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
  | 'STAR'
  | 'COMPONENT'
  | 'INSTANCE';

export type Tool = 'select' | 'hand' | 'frame' | 'rectangle' | 'ellipse' | 'line' | 'text' | 'pen' | 'polygon' | 'star';

export interface LayerNode {
  id: string;
  name: string;
  type: NodeType;
  visible: boolean;
  locked: boolean;
  layout: boolean;
  /** A layer inside an instance (follows its main component). */
  derived: boolean;
  children: LayerNode[];
}

export interface ComponentInfo {
  id: string;
  name: string;
  page: string | null;
  pageName: string | null;
  width: number;
  height: number;
  description: string;
  instances: number;
}

/** A shared color (FILL) or text style. */
export interface StyleInfo {
  id: string;
  name: string;
  type: 'FILL' | 'TEXT';
  description: string;
  fills: Paint[];
  fontFamily: string;
  fontSize: number;
  fontWeight: number;
  italic: boolean;
  lineHeight: number | null;
  letterSpacing: number;
  textDecoration: string | null;
  /** Layers linked to it. */
  uses: number;
}

export type StyleKind = 'fill' | 'stroke' | 'text';

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
  | {
      type: 'IMAGE';
      asset: string;
      fit?: 'FILL' | 'FIT' | 'STRETCH';
      /** Multiply color (Unity's Image.color). */
      tint?: string | null;
      slice?: NineSlice | null;
      /** Unity's Filled image type. */
      filled?: ImageFilled | null;
      opacity?: number;
      visible?: boolean;
    };

export type FillMethod = 'HORIZONTAL' | 'VERTICAL' | 'RADIAL_90' | 'RADIAL_180' | 'RADIAL_360';
export type FillOrigin = 'LEFT' | 'RIGHT' | 'BOTTOM' | 'TOP' | 'BOTTOM_LEFT' | 'TOP_LEFT' | 'TOP_RIGHT' | 'BOTTOM_RIGHT';

/** Show only part of an image: a progress bar or a cooldown wheel. */
export interface ImageFilled {
  method: FillMethod;
  /** 0..1 */
  amount: number;
  origin: FillOrigin;
  clockwise?: boolean;
}

/** The origins each fill method takes, in Unity's fillOrigin order. */
export const FILL_ORIGINS: Record<FillMethod, FillOrigin[]> = {
  HORIZONTAL: ['LEFT', 'RIGHT'],
  VERTICAL: ['BOTTOM', 'TOP'],
  RADIAL_90: ['BOTTOM_LEFT', 'TOP_LEFT', 'TOP_RIGHT', 'BOTTOM_RIGHT'],
  RADIAL_180: ['BOTTOM', 'LEFT', 'TOP', 'RIGHT'],
  RADIAL_360: ['BOTTOM', 'RIGHT', 'TOP', 'LEFT'],
};

/** 9-slice insets in image pixels; corners keep their size. */
export interface NineSlice {
  top: number;
  right: number;
  bottom: number;
  left: number;
  /** Layer px per image px for the border (default 1). */
  scale?: number;
  /** Draw the center patch (default true). */
  fillCenter?: boolean;
}

/** How a layer follows when its parent frame is resized (Unity's anchors). */
export type ConstraintType = 'MIN' | 'MAX' | 'CENTER' | 'STRETCH' | 'SCALE';

export interface Constraints {
  horizontal: ConstraintType;
  vertical: ConstraintType;
}

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
  clipAlpha: boolean;
  hideFill: boolean;
  textWrap: boolean;
  textOverflow: 'VISIBLE' | 'CLIP' | 'ELLIPSIS' | 'TRUNCATE';
  /** Fixed boxes: shrink the font within this range to fit. */
  autoFontSize?: { min: number; max: number } | null;
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
  constraints: Constraints;
  description: string;
  componentId: string | null;
  fillStyleId: string | null;
  strokeStyleId: string | null;
  textStyleId: string | null;
  overrides: Record<string, Record<string, unknown>>;
  derived: boolean;
  componentName?: string | null;
  overrideCount?: number;
  instanceCount?: number;
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
  /** Viewing through a public share link (not signed in). */
  isGuest?: boolean;
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
  wrap: boolean;
  color: string;
}

export type ConnectionStatus = 'connecting' | 'online' | 'offline';
