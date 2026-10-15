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
  | 'INSTANCE'
  | 'PAINT';

export type Tool = 'select' | 'hand' | 'frame' | 'rectangle' | 'ellipse' | 'line' | 'text' | 'pen' | 'polygon' | 'star' | 'brush' | 'eraser';

/** Brush or eraser settings (the engine's `raster::Brush`). */
export interface BrushSettings {
  /** Diameter in layer px at full pressure. */
  size: number;
  /** 0 soft … 1 hard edge. */
  hardness: number;
  /** Alpha of each dab; overlapping dabs build up. */
  flow: number;
  /** Distance between dabs as a fraction of the diameter. */
  spacing: number;
  /** Straight sRGB color and alpha, 0…1. */
  color: [number, number, number, number];
  erase: boolean;
  pressureSize: boolean;
  pressureFlow: boolean;
  /** Stabilizer, 0…0.95. */
  smoothing: number;
  /** Built-in tip image id (null: round). */
  tip: string | null;
  /** Stamp the tip's own colors (photo stamps). */
  tipColor: boolean;
  /** Tip rotation in degrees, and how far it turns with the stroke (0…1). */
  angle: number;
  follow: number;
  /** Random variation per dab. */
  sizeJitter: number;
  angleJitter: number;
  flowJitter: number;
  scatter: number;
  /** Paper grain fixed to the layer. */
  grain: string | null;
  grainScale: number;
  grainStrength: number;
  /** Color variation fixed to the layer. */
  tint: string | null;
  tintScale: number;
  tintStrength: number;
  /** Above 0: picks up and drags the paint under it; this much survives each dab. */
  smudge: number;
  /** Brush color mixed in while smudging. */
  mix: number;
  /** Pen tilt widens (up to 3× when flat) and fades the dab. */
  tiltSize: number;
  tiltFlow: number;
  /** The tip turns to the pen's direction. */
  tiltAngle: boolean;
}

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
      fit?: 'FILL' | 'FIT' | 'STRETCH' | 'TILE';
      /** TILE: layer px per image px. */
      tileScale?: number;
      /** A region of the image in image px (a sprite in an atlas). */
      crop?: { x: number; y: number; width: number; height: number } | null;
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
  /** Paint layers: brush strokes only touch pixels that are already painted. */
  alphaLock: boolean;
  /** Stable id from an export (e.g. Unity), so re-uploads update in place. */
  key?: string | null;
  /** Free-form JSON from tools and agents. */
  meta?: Record<string, unknown>;
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
  layoutMode: 'NONE' | 'HORIZONTAL' | 'VERTICAL' | 'GRID';
  layoutWrap?: boolean;
  counterAxisSpacing?: number | null;
  gridColumns?: number | null;
  gridRows?: number | null;
  gridCellSize?: [number, number] | null;
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
  parentLayout: 'NONE' | 'HORIZONTAL' | 'VERTICAL' | 'GRID';
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
