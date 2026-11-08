import type { PageRect, Quad } from "@memex/types";
import { describe, expect, it } from "vitest";
import { dpiFor, isZoomLevel, overlayBox, overlayBoxes, pageSizeAt, quadAngle, quadPoints, wordPlacement, ZOOM_LEVELS, zoomIn, zoomOut } from "./overlay";

const A4 = { width: 59527, height: 84188 };
const RECT: PageRect = { x0: 10000, y0: 20000, x1: 15000, y1: 21000 };

describe("page size", () => {
  it.each(ZOOM_LEVELS)("is one pixel per point at 100%%, in proportion at %i%%", (zoom) => {
    const size = pageSizeAt(A4, zoom);
    expect(size.width).toBeCloseTo(595.27 * (zoom / 100));
    expect(size.height).toBeCloseTo(841.88 * (zoom / 100));
  });
});

describe("an anchor rect's position", () => {
  it.each(ZOOM_LEVELS)("scales with the page at %i%%", (zoom) => {
    const k = zoom / 100;
    const box = overlayBox(RECT, A4, zoom);
    expect(box?.left).toBeCloseTo(100 * k);
    expect(box?.top).toBeCloseTo(200 * k);
    expect(box?.width).toBeCloseTo(50 * k);
    expect(box?.height).toBeCloseTo(10 * k);
  });

  it.each(ZOOM_LEVELS.flatMap((zoom) => [0, 90, 180, 270].map((rotation) => [zoom, rotation] as const)))("at %i%% on a page at /Rotate %i is where it says, as rects are already displayed coordinates", (zoom, rotation) => {
    // A rotation that swapped the sides is the classic bug here: the image is already turned, so turning the rect too would move it.
    const page = rotation % 180 === 0 ? A4 : { width: A4.height, height: A4.width };
    const facts = { ...page, rotation };
    const box = overlayBoxes([{ ...RECT, page: 3 }], 3, facts, zoom)[0];
    expect(box).toEqual(overlayBox(RECT, page, zoom));
    expect(box?.left).toBeCloseTo((RECT.x0 / 100) * (zoom / 100));
    expect(box?.top).toBeCloseTo((RECT.y0 / 100) * (zoom / 100));
  });

  it("stays on the page, and a rect wholly off it draws nothing", () => {
    expect(overlayBox({ x0: -500, y0: -500, x1: 100, y1: 100 }, A4, 100)).toEqual({ left: 0, top: 0, width: 1, height: 1 });
    expect(overlayBox({ x0: 59000, y0: 84000, x1: 70000, y1: 90000 }, A4, 100)).toEqual({ left: 590, top: 840, width: 5.27, height: 1.88 });
    expect(overlayBox({ x0: 70000, y0: 0, x1: 80000, y1: 100 }, A4, 100)).toBeNull();
  });

  it("takes corners in either order, and a rect with no area draws nothing", () => {
    expect(overlayBox({ x0: 200, y0: 300, x1: 100, y1: 100 }, A4, 100)).toEqual({ left: 1, top: 1, width: 1, height: 2 });
    expect(overlayBox({ x0: 100, y0: 100, x1: 100, y1: 200 }, A4, 100)).toBeNull();
  });

  it("draws only the rects of the page asked about, and keeps a multi-line quote's lines apart", () => {
    const rects = [{ ...RECT, page: 0 }, { ...RECT, y0: 22000, y1: 23000, page: 0 }, { ...RECT, page: 1 }];
    expect(overlayBoxes(rects, 0, A4, 100)).toHaveLength(2);
    expect(overlayBoxes(rects, 1, A4, 100)).toHaveLength(1);
    expect(overlayBoxes(rects, 2, A4, 100)).toEqual([]);
  });

  it("is empty on a page with no size, so a bad answer can't draw across the screen", () => {
    expect(overlayBoxes([{ ...RECT, page: 0 }], 0, { width: 0, height: 0 }, 100)).toEqual([]);
  });
});

describe("a word's selectable text", () => {
  const WORD: PageRect = { x0: 1000, y0: 2000, x1: 1900, y1: 4000 };

  it.each(ZOOM_LEVELS)("runs along a page's turn at %i%%, starting at the corner the text starts from", (zoom) => {
    const k = zoom / 100;
    const [left, top, right, bottom] = [10 * k, 20 * k, 19 * k, 40 * k];
    const [wide, tall] = [right - left, bottom - top];
    const close = (actual: { left: number; top: number; length: number; thickness: number; angle: number }, expected: typeof actual) => {
      for (const key of ["left", "top", "length", "thickness", "angle"] as const) expect(actual[key]).toBeCloseTo(expected[key]);
    };
    close(wordPlacement(WORD, 0, null, zoom), { left, top, length: wide, thickness: tall, angle: 0 });
    close(wordPlacement(WORD, 90, null, zoom), { left: right, top, length: tall, thickness: wide, angle: 90 });
    close(wordPlacement(WORD, 180, null, zoom), { left: right, top: bottom, length: wide, thickness: tall, angle: 180 });
    close(wordPlacement(WORD, 270, null, zoom), { left, top: bottom, length: tall, thickness: wide, angle: 270 });
  });

  it("treats a rotation past a full turn, or below zero, as the same turn", () => {
    expect(wordPlacement(WORD, 450, null, 100).angle).toBe(90);
    expect(wordPlacement(WORD, -90, null, 100).angle).toBe(270);
  });

  // Turning a corner by `angle` about the centre has to land it back on the rect's own centre, whichever way the quad slopes.
  it.each([0, 5, -5, 30, 90])("keeps a quad's word centred on its rect, sloped at %i degrees", (degrees) => {
    const radians = (degrees * Math.PI) / 180;
    const [dx, dy] = [Math.cos(radians) * 1000, Math.sin(radians) * 1000];
    const quad: Quad = [[0, 0], [dx, dy], [dx - dy, dy + dx], [-dy, dx]];
    for (const zoom of ZOOM_LEVELS) {
      const placed = wordPlacement(WORD, 90, quad, zoom);
      const turned = (placed.angle * Math.PI) / 180;
      const centreX = placed.left + (placed.length / 2) * Math.cos(turned) - (placed.thickness / 2) * Math.sin(turned);
      const centreY = placed.top + (placed.length / 2) * Math.sin(turned) + (placed.thickness / 2) * Math.cos(turned);
      expect(centreX).toBeCloseTo(((WORD.x0 + WORD.x1) / 200) * (zoom / 100));
      expect(centreY).toBeCloseTo(((WORD.y0 + WORD.y1) / 200) * (zoom / 100));
      expect(placed.angle).toBeCloseTo((degrees + 360) % 360);
    }
  });
});

describe("a quad", () => {
  const skewed: Quad = [[100, 100], [1100, 200], [1090, 400], [90, 300]];

  it("gives its slope from the top edge", () => {
    expect(quadAngle(skewed)).toBeCloseTo(5.71, 1);
    expect(quadAngle([[0, 100], [0, 0], [100, 0], [100, 100]])).toBeCloseTo(270);
  });

  it("with no length has no slope", () => {
    expect(quadAngle([[5, 5], [5, 5], [5, 5], [5, 5]])).toBe(0);
  });

  it.each(ZOOM_LEVELS)("keeps its corners, in order, at %i%%", (zoom) => {
    const k = zoom / 100;
    const points = quadPoints(skewed, zoom);
    expect(points).toHaveLength(4);
    expect(points[0]?.[0]).toBeCloseTo(1 * k);
    expect(points[1]?.[0]).toBeCloseTo(11 * k);
    expect(points[2]?.[1]).toBeCloseTo(4 * k);
    expect(points[3]?.[0]).toBeCloseTo(0.9 * k);
  });
});

describe("the image's resolution", () => {
  const ALLOWED = [36, 72, 150, 300];

  it("is the smallest that's sharp", () => {
    expect(dpiFor(100, 1, ALLOWED)).toBe(72);
    expect(dpiFor(50, 1, ALLOWED)).toBe(36);
    expect(dpiFor(125, 1, ALLOWED)).toBe(150);
    expect(dpiFor(100, 2, ALLOWED)).toBe(150);
    expect(dpiFor(200, 2, ALLOWED)).toBe(300);
  });

  it("stops at the largest there is, whatever order it's told them in", () => {
    expect(dpiFor(300, 2, [300, 36, 150])).toBe(300);
    expect(dpiFor(300, 3, [72])).toBe(72);
  });

  it("is nothing when the server offers none, and a bad screen ratio counts as one", () => {
    expect(dpiFor(100, 1, [])).toBeNull();
    expect(dpiFor(100, 0, ALLOWED)).toBe(72);
  });
});

describe("zoom", () => {
  it("steps through the levels and stops at each end", () => {
    expect(zoomIn(100)).toBe(125);
    expect(zoomOut(100)).toBe(75);
    expect(zoomIn(300)).toBe(300);
    expect(zoomOut(50)).toBe(50);
  });

  it("moves to the next level from anywhere between them", () => {
    expect(zoomIn(110)).toBe(125);
    expect(zoomOut(110)).toBe(100);
  });

  it("knows its levels, so a URL can't ask for another", () => {
    expect(isZoomLevel(150)).toBe(true);
    expect(isZoomLevel(151)).toBe(false);
  });
});
