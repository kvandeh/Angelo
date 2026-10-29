# Contributing to Angelo

1. Fork the repository and make your change on a branch.
2. Open a pull request against `develop`. A push to `develop` publishes a test build to
   TestPyPI. A merge from `develop` into `main` makes a release and publishes it to PyPI.
3. Before you push, run these commands in `Angelo/`, where the Rust crate is. CI runs all
   of them and fails on any warning:

   ```bash
   cargo fmt
   cargo clippy --all-targets
   cargo test
   ```

4. If your change affects how Angelo runs mutants, also run this command from the
   **repository root**:

   ```bash
   bash scripts/verdict-matrix.sh
   ```

   It makes sure that all configurations give the same score: 16 on Linux and macOS, 8 on
   Windows. A speed-up that changes a verdict is a bug.

A change in `integrations/` has its own workflow and needs no cargo run.

Put the test for new logic in the same file as the logic. If your change affects behaviour
that the documentation describes, update the page in `docs/` in the same pull request.
