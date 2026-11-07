/** A plain text PDF of as many pages as asked for, written directly, so a long document needs no library and no file in the repository. */
export const makePdf = (pages: number, lineOf: (page: number) => string): Buffer => {
  const escape = (text: string) => text.replace(/\\/g, "\\\\").replace(/\(/g, "\\(").replace(/\)/g, "\\)");
  const objects: string[] = [];
  const pageId = (n: number) => 4 + n * 2;
  const contentId = (n: number) => 5 + n * 2;
  objects[1] = "<< /Type /Catalog /Pages 2 0 R >>";
  objects[2] = `<< /Type /Pages /Count ${String(pages)} /Kids [${Array.from({ length: pages }, (_, n) => `${String(pageId(n))} 0 R`).join(" ")}] >>`;
  objects[3] = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
  for (let n = 0; n < pages; n += 1) {
    const lines = Array.from({ length: 30 }, (_, i) => `${i === 0 ? "" : "0 -22 Td "}(${escape(`${lineOf(n)} line ${String(i + 1)}`)}) Tj`).join("\n");
    const stream = `BT /F1 12 Tf 72 760 Td\n${lines}\nET`;
    objects[pageId(n)] = `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents ${String(contentId(n))} 0 R >>`;
    objects[contentId(n)] = `<< /Length ${String(Buffer.byteLength(stream, "latin1"))} >>\nstream\n${stream}\nendstream`;
  }
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
