# Spec 003 corpus

100 pages, not committed (`corpus/` is git-ignored). Rebuild with `tools/gen_synth.py` and `tools/fetch_real.py`.

- **Synthetic, 70 pages (`tools/gen_synth.py`):** 18 English, 18 Swedish, 17 German, 17 Spanish, rendered at 150 DPI in four fonts, with words from the real pages below. Degradations: clean, blur, noise, skew 3° and 5°. Exact line text and boxes.
- **Real, 30 pages (`tools/fetch_real.py`):** random Wikisource pages with quality 3 or 4 (proofread or validated), 8 English, 8 Swedish, 7 German, 7 Spanish. Public-domain scans. Their sources are in each `corpus/real/<lang>/*.json`.

**Change from the spec:** the real text is the Wikisource community's proofreading, not my hand labels, and it's page text with no line boxes. So real pages give CER only, and line IoU comes from the synthetic pages. Languages total 26 English, 26 Swedish, 24 German and 24 Spanish pages.
