# The reviewer's labelled set

Made-up facts in English, Swedish, German and Spanish, written for this repository (spec 025) and covered by its license. Each line is a claim and a quote, with `correct` saying whether the quote states the claim.

**An agent wrote every label, so `labelled_by` is `agent` on every line, and these labels don't count as human labels.** They exist so the harness in `crates/memexvault/tests/review_eval.rs` has something to run. To use the set for spec 025's item 2, a person checks each line, corrects it if needed, and sets `labelled_by` to who they are. The harness says plainly in its report whether all its labels came from people, and it won't call its number a measurement if they didn't.
