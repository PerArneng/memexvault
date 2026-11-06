# /// script
# requires-python = ">=3.12"
# dependencies = ["reportlab==4.4.*", "pypdf==6.*", "cryptography"]
# ///
"""Generate the synthetic part of the spec 002 corpus into ../corpus/gen. Usage: uv run gen_corpus.py"""
import pathlib, random, subprocess
from reportlab.lib.pagesizes import A4
from reportlab.pdfgen import canvas
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from pypdf import PdfReader, PdfWriter
from pypdf.generic import RectangleObject

out = pathlib.Path(__file__).parent.parent / "corpus/gen"
out.mkdir(parents=True, exist_ok=True)
pdfmetrics.registerFont(TTFont("Arial", "/System/Library/Fonts/Supplemental/Arial.ttf"))
WORDS = {
 "en": "the system stores original documents and every claim links back to the exact region where evidence appears after review office fifty affluent".split(),
 "sv": "systemet lagrar dokument på disk och varje påstående går att spåra till källan nämnda områden för söktjänst räksmörgås göteborg åtgärd".split(),
 "de": "das system speichert größere dokumente und jede aussage führt zurück zur quelle übergröße straße müller prüfung schließlich äußerst".split(),
 "es": "el sistema guarda documentos y cada afirmación enlaza con la región exacta de la fuente mañana señor niño año corazón está según".split(),
}
def para(rng, lang, n): return [rng.choice(WORDS[lang]) for _ in range(n)]
def flow(c, words, x, y, width, bottom, font, size, lead):
    line, top = [], y
    for w in words:
        if pdfmetrics.stringWidth(" ".join(line + [w]), font, size) > width:
            c.setFont(font, size); c.drawString(x, y, " ".join(line)); y -= lead; line = []
            if y < bottom: return
        line.append(w)
    if line: c.setFont(font, size); c.drawString(x, y, " ".join(line))
def columns(name, lang, cols, pages=2, font="Arial"):
    rng = random.Random(name); c = canvas.Canvas(str(out / name), pagesize=A4); W, H = A4
    for _ in range(pages):
        gap = 18; cw = (W - 80 - gap * (cols - 1)) / cols
        for i in range(cols):
            flow(c, para(rng, lang, 600), 40 + i * (cw + gap), H - 60, cw, 50, font, 10, 12)
        c.showPage()
    c.save()
def table(name, lang):
    rng = random.Random(name); c = canvas.Canvas(str(out / name), pagesize=A4); W, H = A4
    for r in range(30):
        y = H - 60 - r * 20
        c.line(40, y - 5, W - 40, y - 5)
        for col in range(5):
            c.setFont("Arial", 9); c.drawString(45 + col * 100, y, f"{rng.choice(WORDS[lang])} {rng.randint(10, 9999)}")
            c.line(40 + col * 100, y - 5, 40 + col * 100, y + 15)
    c.save()
def transformed(name, src, rotate=0, crop=None):
    r = PdfReader(out / src); w = PdfWriter()
    for p in r.pages:
        if crop:
            p.cropbox = RectangleObject(crop)
        if rotate: p.rotate(rotate)
        w.add_page(p)
    w.write(out / name)
def main():
    columns("two_column_en.pdf", "en", 2)
    columns("three_column_de.pdf", "de", 3)
    columns("one_column_es_helv.pdf", "es", 1, font="Helvetica")
    columns("one_column_sv.pdf", "sv", 1)
    table("table_sv.pdf", "sv"); table("table_es.pdf", "es")
    transformed("cropbox_offset.pdf", "two_column_en.pdf", crop=(70, 110, 500, 760))
    transformed("rotate90.pdf", "one_column_sv.pdf", rotate=90)
    transformed("rotate180_crop.pdf", "three_column_de.pdf", rotate=180, crop=(70, 110, 500, 760))
    transformed("rotate270_crop.pdf", "one_column_es_helv.pdf", rotate=270, crop=(30, 40, 560, 800))
    w = PdfWriter(clone_from=out / "one_column_sv.pdf")
    w.encrypt(user_password="", owner_password="owner", algorithm="AES-256"); w.write(out / "encrypted_empty_password.pdf")
    # Image-only page and a hidden OCR layer over it, both from a raster of the English file.
    tmp = out / "scan"
    subprocess.run(["pdftoppm", "-r", "200", "-png", "-f", "1", "-l", "1", "-singlefile", str(out / "two_column_en.pdf"), str(tmp)], check=True)
    c = canvas.Canvas(str(out / "image_only.pdf"), pagesize=A4); c.drawImage(f"{tmp}.png", 0, 0, *A4); c.save()
    subprocess.run(["tesseract", f"{tmp}.png", str(out / "ocr_layer"), "--dpi", "200", "pdf"], check=True, capture_output=True)
    (out / "scan.png").unlink()
    r = PdfWriter(); r.append(out / "one_column_sv.pdf", pages=(0, 1)); r.append(out / "image_only.pdf"); r.write(out / "mixed_text_image.pdf")
    # Delete intermediates? All stay in the corpus.
main()
