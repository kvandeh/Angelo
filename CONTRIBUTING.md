# Contributing to Angelo

Fork the repository, make your change, and open a pull request against `main`. The Rust
crate lives in `Angelo/`, so run `cargo fmt`, `cargo clippy --all-targets` and `cargo test`
from there before you push, because continuous integration runs all three and fails on any
warning. If your change touches how mutants are run, also run `bash
scripts/verdict-matrix.sh` **from the repository root**, which checks that sixteen different
configurations still agree on the same score; a speedup that changes a verdict is a bug, not
a result. A change under `integrations/` is built by its own workflow and needs no cargo run
at all. New logic wants a test next to it in the same file, and anything
that changes documented behaviour wants the matching page in `docs/` updated in the same
pull request. That is the whole process.
