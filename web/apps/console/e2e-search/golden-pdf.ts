/** A PDF of whole pages of text in a Latin encoding, written directly, so the golden passages need no file in the repository. */
const WIDTH = 80;
const LINES_PER_PAGE = 50;

export const wrap = (text: string): readonly string[] => {
  const lines: string[] = [];
  let line = "";
  for (const word of text.split(/\s+/).filter((w) => w !== "")) {
    if (line !== "" && line.length + word.length + 1 > WIDTH) {
      lines.push(line);
      line = word;
    } else line = line === "" ? word : `${line} ${word}`;
  }
  if (line !== "") lines.push(line);
  return lines;
};

export const isLatin = (text: string): boolean => [...text].every((c) => {
  const code = c.codePointAt(0) ?? 0;
  return (code >= 0x20 && code < 0x7f) || (code >= 0xa1 && code <= 0xff && code !== 0xad);
});

const escape = (text: string) => text.replace(/\\/g, "\\\\").replace(/\(/g, "\\(").replace(/\)/g, "\\)");

/** Each string is one page. */
export const makeTextPdf = (pages: readonly string[]): Buffer => {
  const objects: string[] = [];
  const pageId = (n: number) => 4 + n * 2;
  const contentId = (n: number) => 5 + n * 2;
  objects[1] = "<< /Type /Catalog /Pages 2 0 R >>";
  objects[2] = `<< /Type /Pages /Count ${String(pages.length)} /Kids [${pages.map((_, n) => `${String(pageId(n))} 0 R`).join(" ")}] >>`;
  objects[3] = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";
  pages.forEach((text, n) => {
    const lines = wrap(text).slice(0, LINES_PER_PAGE).map((line, i) => `${i === 0 ? "" : "0 -14 Td "}(${escape(line)}) Tj`).join("\n");
    const stream = `BT /F1 11 Tf 60 770 Td\n${lines}\nET`;
    objects[pageId(n)] = `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents ${String(contentId(n))} 0 R >>`;
    objects[contentId(n)] = `<< /Length ${String(Buffer.byteLength(stream, "latin1"))} >>\nstream\n${stream}\nendstream`;
  });
  let out = "%PDF-1.4\n";
  const offsets: number[] = [];
  for (let id = 1; id < objects.length; id += 1) {
    offsets[id] = Buffer.byteLength(out, "latin1");
    out += `${String(id)} 0 obj\n${objects[id] ?? ""}\nendobj\n`;
  }
  const xref = Buffer.byteLength(out, "latin1");
  out += `xref\n0 ${String(objects.length)}\n0000000000 65535 f \n`;
  for (let id = 1; id < objects.length; id += 1) out += `${String(offsets[id] ?? 0).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${String(objects.length)} /Root 1 0 R >>\nstartxref\n${String(xref)}\n%%EOF\n`;
  return Buffer.from(out, "latin1");
};
