export function fitScale(
  width: number,
  height: number,
  availableWidth: number,
  availableHeight: number,
) {
  if (width <= 0 || height <= 0 || availableWidth <= 0 || availableHeight <= 0)
    return 1;
  return Math.min(availableWidth / width, availableHeight / height);
}

/** Keep the image point underneath the cursor fixed when the pixel scale changes. */
export function zoomAround(
  oldScale: number,
  newScale: number,
  x: number,
  y: number,
  anchorX: number,
  anchorY: number,
) {
  const ratio = newScale / oldScale;
  return {
    x: anchorX - (anchorX - x) * ratio,
    y: anchorY - (anchorY - y) * ratio,
  };
}
