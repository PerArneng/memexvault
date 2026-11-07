# Golden sets for search

`golden.jsonl` (spec 020) and `golden_relational.jsonl` (spec 022) are the test questions and answers for search and for k-hop expansion.

## Source and license

These two files aren't under this repository's Apache-2.0 license. Treat them as [CC BY-SA](https://creativecommons.org/licenses/by-sa/3.0/):

- Each `answer` is short text taken from a Wikipedia paragraph, and each question was written by an agent from such a paragraph.
- The paragraphs are from the English, Swedish, German and Spanish Wikipedias, by Wikipedia contributors, through the published dataset [`wikimedia/wikipedia`](https://huggingface.co/datasets/wikimedia/wikipedia), config `20231101.<language>`. The dataset's metadata states `cc-by-sa-3.0` and `gfdl`.
- The corpus isn't committed. Each entry points to its paragraph by line and hash in the file `specs/004-spike-vector/tools/prep.py` builds, so credit is to the dataset, not to each article, because that corpus has no titles.

The extraction set's [README](../extraction/README.md) has a note on the license version, which applies here too.
