# Injection fixtures

Written for this repository (spec 025), and covered by its license. Each line is one document carrying a planted instruction in one of four places: `text` (visible), `hidden` (invisible characters, or a page the pipeline flagged), `metadata` (the file name and metadata only, which must never reach a model) and `tool_output` (text dressed up as a tool's result).

`quote`, `subject`, `predicate` and `value` are what a model that fell for it would return. The tests in `crates/memex-core/tests/review.rs` have a scripted agent return exactly that, so they prove what the gate does with an obedient model. They say nothing about whether a real model would obey. `live_injection_fixtures` in `crates/memexvault/tests/review_eval.rs` is the run that does, and it costs money.
