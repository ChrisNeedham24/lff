# Contributions

For those wishing to contribute, welcome and good luck.

### Links

- [Issues](https://github.com/ChrisNeedham24/lff/issues)
- [Discussions](https://github.com/ChrisNeedham24/lff/discussions)

### Testing

Tests in this repository use the built-in Rust testing framework as well as the [tarpaulin](https://github.com/xd009642/tarpaulin) crate for coverage.

To run the tests, the following command can be run from the project's root directory: `cargo test`

Alternatively, to run the tests with coverage enabled, you can run the following command from the project's root directory: `cargo tarpaulin`.
This will also generate a coverage report.

All business logic and functional code requires unit testing, with the exception of obvious things like writing to standard out.

100% coverage of functional code is required to pass the coverage CI check.

If you create a new function that writes to standard out or does some other uncoverable operation, you can add `#[cfg(not(tarpaulin_include))]` above it to exempt it from coverage checks.

### Environment details

To contribute to Microcosm, first create a fork of the repository into your own GitHub account.
For development, Rust 2021 as well as the crates defined in `Cargo.toml` are needed.

Please note that all changes should be made on a branch *other* than main.

### Submitting pull requests

When you're satisfied that you have completed an issue, or have made another valuable contribution, put up a pull request for review.
You should receive a response in a day or two, and a full review by the weekend at the latest.

### If you find a bug

Hey, none of us are perfect. So, if you find a bug in the tool, add a new issue [here](https://github.com/ChrisNeedham24/lff/issues/new).
Any submitted issues of this kind should have the bug label, so be sure to mention in the issue description that the label should be applied.

If you're not sure whether something classifies as a bug, just suggest the 'almost a bug' label instead.

### What to try first

Any issue with either of the 'hacktoberfest' or 'good first issue' labels will be a good start.

### Additional features

If you have an idea for the tool that isn't adequately captured in existing issues, add a new issue [here](https://github.com/ChrisNeedham24/lff/issues/new).
If you think the feature may require some significant work, be sure to mention that in the issue description as well.

### Style guide

lff follows standard Rust styling.
[clippy](https://github.com/rust-lang/rust-clippy) and [rustfmt](https://github.com/rust-lang/rustfmt) are also used to guarantee conformity.
If you're ever unsure whether you've formatted something correctly, run `cargo clippy` and/or `rustfmt src/main.rs --check`.

### Code of Conduct

See [here](/CODE-OF-CONDUCT.md) for the repository's Code of Conduct.