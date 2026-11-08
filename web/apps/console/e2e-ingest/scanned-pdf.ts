import { deflateSync } from "node:zlib";
import type { Browser } from "@playwright/test";
import { decodePng } from "../e2e-documents/png";

/** Text drawn by Chromium and kept only as pixels, in a PDF with no text layer, so the server has to OCR it. No file in the repository, and no library. */
export const makeScannedPdf = async (browser: Browser, pages: readonly string[]): Promise<Buffer> => {
  const page = await browser.newPage({ viewport: { width: 800, height: 1000 }, deviceScaleFactor: 1, colorScheme: "light" });
  const images: { width: number; height: number; data: Buffer }[] = [];
  for (const text of pages) {
    await page.setContent(`<body style="margin:60px;background:white;color:black;font:28px/1.6 Georgia,serif">${text}</body>`);
    const shot = decodePng(await page.screenshot());
    const gray = Buffer.alloc(shot.width * shot.height);
    for (let i = 0; i < gray.length; i += 1) gray[i] = shot.data[i * shot.channels] ?? 255;
    images.push({ width: shot.width, height: shot.height, data: deflateSync(gray) });
  }
  await page.close();

  const parts: Buffer[] = [];
  const offsets: number[] = [];
  let length = 0;
  const write = (chunk: Buffer | string) => {
    const b = typeof chunk === "string" ? Buffer.from(chunk, "latin1") : chunk;
    parts.push(b);
    length += b.length;
  };
  const object = (id: number, body: string, stream?: Buffer) => {
    offsets[id] = length;
    write(`${String(id)} 0 obj\n${body}\n`);
    if (stream !== undefined) {
      write("stream\n");
      write(stream);
      write("\nendstream\n");
    }
    write("endobj\n");
  };
  write("%PDF-1.4\n");
  const pageId = (n: number) => 3 + n * 3;
  object(1, "<< /Type /Catalog /Pages 2 0 R >>");
  object(2, `<< /Type /Pages /Count ${String(images.length)} /Kids [${images.map((_, n) => `${String(pageId(n))} 0 R`).join(" ")}] >>`);
  images.forEach((image, n) => {
    const content = Buffer.from(`q 595 0 0 744 0 0 cm /Im0 Do Q`);
    object(pageId(n), `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 744] /Resources << /XObject << /Im0 ${String(pageId(n) + 1)} 0 R >> >> /Contents ${String(pageId(n) + 2)} 0 R >>`);
    object(pageId(n) + 1, `<< /Type /XObject /Subtype /Image /Width ${String(image.width)} /Height ${String(image.height)} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length ${String(image.data.length)} >>`, image.data);
    object(pageId(n) + 2, `<< /Length ${String(content.length)} >>`, content);
  });
  const total = pageId(images.length);
  const xref = length;
  write(`xref\n0 ${String(total)}\n0000000000 65535 f \n`);
  for (let id = 1; id < total; id += 1) write(`${String(offsets[id] ?? 0).padStart(10, "0")} 00000 n \n`);
  write(`trailer\n<< /Size ${String(total)} /Root 1 0 R >>\nstartxref\n${String(xref)}\n%%EOF\n`);
  return Buffer.concat(parts);
};
