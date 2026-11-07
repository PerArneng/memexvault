import type { PageRect, Quad } from "@memex/types";

/**
 * Turns anchor rects and page sizes into positions on screen. A rect is in displayed coordinates, after CropBox and /Rotate, in 1/100 pt, so a
 * page's rotation never moves it: the server's image is already turned. 100% zoom is one CSS pixel per point. Nothing here knows about a screen.
 */
export interface Size {
  readonly width: number;
  readonly height: number;
}

export interface Box extends Size {
  readonly left: number;
  readonly top: number;
}

/** A word's invisible text: start at this corner, then run `length` along `angle`, clockwise in degrees. */
export interface WordPlacement {
  readonly left: number;
  readonly top: number;
  readonly length: number;
  readonly thickness: number;
  readonly angle: number;
}

export const ZOOM_LEVELS = [50, 75, 100, 125, 150, 200, 300] as const;
export const DEFAULT_ZOOM = 100;
const UNITS_PER_POINT = 100;
const POINTS_PER_INCH = 72;
const PERCENT = 100;

const toPixels = (units: number, zoom: number): number => (units / UNITS_PER_POINT) * (zoom / PERCENT);

export const pageSizeAt = (page: Size, zoom: number): Size => ({ width: toPixels(page.width, zoom), height: toPixels(page.height, zoom) });

/** A rect drawn on its page. It's held to the page, and one with nothing left is null, as a broken one mustn't draw somewhere misleading. */
export const overlayBox = (rect: PageRect, page: Size, zoom: number): Box | null => {
  const x0 = Math.max(0, Math.min(rect.x0, rect.x1));
  const y0 = Math.max(0, Math.min(rect.y0, rect.y1));
  const x1 = Math.min(page.width, Math.max(rect.x0, rect.x1));
  const y1 = Math.min(page.height, Math.max(rect.y0, rect.y1));
  if (x1 <= x0 || y1 <= y0) return null;
  return { left: toPixels(x0, zoom), top: toPixels(y0, zoom), width: toPixels(x1 - x0, zoom), height: toPixels(y1 - y0, zoom) };
};

/** The rects that fall on one page, as boxes on it. */
export const overlayBoxes = (rects: readonly (PageRect & { readonly page: number })[], pageIndex: number, page: Size, zoom: number): readonly Box[] =>
  rects.flatMap((rect) => {
    if (rect.page !== pageIndex) return [];
    const box = overlayBox(rect, page, zoom);
    return box === null ? [] : [box];
  });

const normalAngle = (degrees: number): number => ((degrees % 360) + 360) % 360;

/** A skewed OCR line's slope, from its top edge. A quad with no length has none. */
export const quadAngle = (quad: Quad): number => {
  const [[ax, ay], [bx, by]] = quad;
  if (ax === bx && ay === by) return 0;
  return normalAngle((Math.atan2(by - ay, bx - ax) * 180) / Math.PI);
};

/** A quad's corners on screen, in order. */
export const quadPoints = (quad: Quad, zoom: number): readonly (readonly [number, number])[] => quad.map(([x, y]) => [toPixels(x, zoom), toPixels(y, zoom)] as const);

/**
 * Where a word's selectable text goes. On a page turned by /Rotate, text runs along the turn, and so does the word. `pageAngle` is the page's
 * rotation for a text layer, and 0 for OCR, which reads a page as it's displayed. A quad's slope replaces it, around the word's own centre.
 */
export const wordPlacement = (rect: PageRect, pageAngle: number, quad: Quad | null, zoom: number): WordPlacement => {
  const left = toPixels(rect.x0, zoom);
  const top = toPixels(rect.y0, zoom);
  const right = toPixels(rect.x1, zoom);
  const bottom = toPixels(rect.y1, zoom);
  const across = right - left;
  const down = bottom - top;
  if (quad !== null) {
    const slope = quadAngle(quad);
    const radians = (slope * Math.PI) / 180;
    const centreX = (left + right) / 2;
    const centreY = (top + bottom) / 2;
    return {
      left: centreX - (across / 2) * Math.cos(radians) + (down / 2) * Math.sin(radians),
      top: centreY - (across / 2) * Math.sin(radians) - (down / 2) * Math.cos(radians),
      length: across,
      thickness: down,
      angle: slope,
    };
  }
  switch (normalAngle(pageAngle)) {
    case 90:
      return { left: right, top, length: down, thickness: across, angle: 90 };
    case 180:
      return { left: right, top: bottom, length: across, thickness: down, angle: 180 };
    case 270:
      return { left, top: bottom, length: down, thickness: across, angle: 270 };
    default:
      return { left, top, length: across, thickness: down, angle: 0 };
  }
};

/** The smallest resolution the server offers that's sharp at this zoom on this screen, or else its largest. Null when it offers none. */
export const dpiFor = (zoom: number, pixelRatio: number, allowed: readonly number[]): number | null => {
  if (allowed.length === 0) return null;
  const sorted = [...allowed].sort((a, b) => a - b);
  const needed = POINTS_PER_INCH * (zoom / PERCENT) * (pixelRatio > 0 ? pixelRatio : 1);
  return sorted.find((dpi) => dpi >= needed) ?? sorted[sorted.length - 1] ?? null;
};

export const zoomIn = (zoom: number): number => ZOOM_LEVELS.find((z) => z > zoom) ?? zoom;
export const zoomOut = (zoom: number): number => [...ZOOM_LEVELS].reverse().find((z) => z < zoom) ?? zoom;
export const isZoomLevel = (zoom: number): boolean => (ZOOM_LEVELS as readonly number[]).includes(zoom);
