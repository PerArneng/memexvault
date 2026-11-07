# Labelled set for spec 024

Fifty short documents and their labels, for verification item 1 of `specs/024-extraction-agent/spec.md`.

## Sources and licenses

The text in `documents.jsonl` comes from Wikipedia, and each line records its own title, source, license and attribution. It isn't under this repository's Apache-2.0 license:

- `license: CC BY-SA 4.0`: 10 English documents from the Wikipedia API on 2026-11-07. `source_url` names the exact revision.
- `license: CC BY-SA 3.0`: the rest, from the published dataset `wikimedia/wikipedia`, config `20231101.<language>`, revision `b04c8d1ceb2f5cd4588862100d08de323dccfbaa`, whose card states `cc-by-sa-3.0` and `gfdl`. `source_url` is the article as the dataset gives it, and the text is the 2023-11-01 dump.

Each text is cut to its opening, at the last sentence end within 900 characters, or at 900 characters when none falls in that range. Nothing else was changed, so section headings the dump leaves in the text stay.

`labels.jsonl` is derived from those texts, and shares their license.

## Labels

An agent wrote the labels by reading each text, without any model output to compare with. **No person has checked them.** A review before the live run is worthwhile, as a labelling mistake counts against a model.

- `entities`: each named person, organization, product, project or decision the text states, with field values as lists of equally good spellings. `description` and `rationale` are running text, and aren't labelled or scored. Dates in `released`, `decided_on` and `started` are ISO, and are only labelled when the text gives a full date.
- `claims`: a few central facts per document, not all of them, with lists for the subject and value.

## Scoring

`crates/memexvault/tests/extraction_eval.rs` scores each field value and free claim. A prediction is `correct` if the labels hold it, `wrong` if they state that field differently, and otherwise `unlabelled`. Precision counts unlabelled as wrong until a person gives a verdict in the file named by `MEMEX_EVAL_ADJUDICATIONS`, because the claim labels aren't exhaustive. So the number before adjudication is a lower bound, and the pass is decided on the adjudicated one.

Each line of that file is `{"doc": "en-01", "prediction": "<the key from to_adjudicate>", "correct": true}`.

## Approval

`all-documents.txt` lists every id. Passing it as `MEMEX_EVAL_APPROVED` is the decision to send those documents to the provider, and no run does that by default.
