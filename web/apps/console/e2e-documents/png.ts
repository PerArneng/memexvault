import { inflateSync } from "node:zlib";

/** Reads the 8-bit, non-interlaced PNGs a browser's screenshot makes, so a pixel test needs no dependency. */
export interface Pixels {
  readonly width: number;
  readonly height: number;
  readonly channels: number;
  readonly data: Uint8Array;
}

const paeth = (a: number, b: number, c: number): number => {
  const p = a + b - c;
  const [pa, pb, pc] = [Math.abs(p - a), Math.abs(p - b), Math.abs(p - c)];
  if (pa <= pb && pa <= pc) return a;
  return pb <= pc ? b : c;
};

export const decodePng = (file: Buffer): Pixels => {
  let at = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const parts: Buffer[] = [];
  while (at < file.length) {
    const length = file.readUInt32BE(at);
    const kind = file.toString("latin1", at + 4, at + 8);
    const body = file.subarray(at + 8, at + 8 + length);
    if (kind === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      const [depth, color, interlace] = [body[8], body[9], body[12]];
      if (depth !== 8 || interlace !== 0 || (color !== 2 && color !== 6)) throw new Error(`can't read this PNG: depth ${String(depth)}, colour ${String(color)}, interlace ${String(interlace)}`);
      channels = color === 6 ? 4 : 3;
    }
    if (kind === "IDAT") parts.push(body);
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(parts));
  const stride = width * channels;
  const data = new Uint8Array(height * stride);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    for (let x = 0; x < stride; x += 1) {
      const value = raw[y * (stride + 1) + 1 + x] ?? 0;
      const left = x >= channels ? (data[y * stride + x - channels] ?? 0) : 0;
      const up = y > 0 ? (data[(y - 1) * stride + x] ?? 0) : 0;
      const corner = y > 0 && x >= channels ? (data[(y - 1) * stride + x - channels] ?? 0) : 0;
      const predicted = filter === 1 ? left : filter === 2 ? up : filter === 3 ? (left + up) >> 1 : filter === 4 ? paeth(left, up, corner) : 0;
      data[y * stride + x] = (value + predicted) & 0xff;
    }
  }
  return { width, height, channels, data };
};

export interface Bounds {
  readonly left: number;
  readonly top: number;
  /** One past the last changed pixel, so it compares with a box's edge. */
  readonly right: number;
  readonly bottom: number;
}

/** The box around every pixel that differs between two images of the same size. */
export const changedBounds = (a: Pixels, b: Pixels, threshold: number): Bounds | null => {
  let found: { left: number; top: number; right: number; bottom: number } | null = null;
  for (let y = 0; y < a.height; y += 1) {
    for (let x = 0; x < a.width; x += 1) {
      const offset = (y * a.width + x) * a.channels;
      let most = 0;
      for (let c = 0; c < 3; c += 1) most = Math.max(most, Math.abs((a.data[offset + c] ?? 0) - (b.data[offset + c] ?? 0)));
      if (most > threshold) {
        found ??= { left: x, top: y, right: x + 1, bottom: y + 1 };
        found.left = Math.min(found.left, x);
        found.top = Math.min(found.top, y);
        found.right = Math.max(found.right, x + 1);
        found.bottom = Math.max(found.bottom, y + 1);
      }
    }
  }
  return found;
};

/** Share of a box's pixels that are ink, which here is anything well away from a white page. */
export const inkIn = (image: Pixels, box: Bounds, darkerThan: number): { readonly ink: number; readonly total: number } => {
  let ink = 0;
  let total = 0;
  for (let y = Math.max(0, Math.floor(box.top)); y < Math.min(image.height, Math.ceil(box.bottom)); y += 1) {
    for (let x = Math.max(0, Math.floor(box.left)); x < Math.min(image.width, Math.ceil(box.right)); x += 1) {
      total += 1;
      const offset = (y * image.width + x) * image.channels;
      if ((image.data[offset] ?? 255) < darkerThan) ink += 1;
    }
  }
  return { ink, total };
};
