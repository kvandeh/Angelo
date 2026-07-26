# Contributing to angelo

Fork the repository, make your change, and open a pull request against `main`. Run
`cargo fmt`, `cargo clippy --all-targets` and `cargo test` before you push, because
continuous integration runs all three and fails on any warning. If your change touches how
mutants are run, also run `bash scripts/verdict-matrix.sh`, which checks that eight
different configurations still agree on the same score; a speedup that changes a verdict
is a bug, not a result. New logic wants a test next to it in the same file, and anything
that changes documented behaviour wants the matching page in `docs/` updated in the same
pull request. That is the whole process.
