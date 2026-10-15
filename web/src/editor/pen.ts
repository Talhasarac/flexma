// Pen input: pressure curve and tilt from pointer events.
/** Pen pressure through the user's curve: `curve` −1 (light touch) … 1 (firm). */
export function applyCurve(p: number, curve: number): number {
  const v = Math.min(1, Math.max(0, p));
  return curve === 0 ? v : Math.pow(v, Math.pow(4, curve));
}

/**
 * A pen's tilt (0 upright … 1 flat) and the direction it leans (radians, 0 =
 * right, clockwise on screen), from altitude/azimuth or tiltX/tiltY.
 */
export function penTilt(e: PointerEvent): [number, number] {
  const p = e as PointerEvent & { altitudeAngle?: number; azimuthAngle?: number };
  let altitude = p.altitudeAngle;
  let azimuth = p.azimuthAngle;
  if (altitude === undefined || azimuth === undefined) {
    const a = Math.tan((e.tiltX * Math.PI) / 180);
    const b = Math.tan((e.tiltY * Math.PI) / 180);
    if (a === 0 && b === 0) return [0, 0];
    azimuth = Math.atan2(b, a);
    altitude = Math.atan(1 / Math.hypot(a, b));
  }
  const tilt = Math.min(1, Math.max(0, 1 - altitude / (Math.PI / 2)));
  return [tilt, azimuth < 0 ? azimuth + 2 * Math.PI : azimuth];
}
