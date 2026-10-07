# Contributing to Replane Rust SDK

Thank you for your interest in contributing! This guide will help you get started.

## Getting Started

### Prerequisites

- **Rust**: latest stable toolchain (install via [rustup](https://rustup.rs/))

### Clone the Repository

```sh
git clone https://github.com/replane-dev/replane-rust.git
cd replane-rust
```

### Build

```sh
cargo build
```

## Development

The crate is organized as follows:

- `src/` - SDK library
- `tests/` - Integration tests against a mock SSE server
- `examples/` - Runnable examples

### Run Tests

```sh
cargo test
```

### Lint

```sh
cargo clippy --all-targets -- -D warnings
```

### Format

```sh
cargo fmt
```

### Build Docs

```sh
cargo doc --no-deps --open
```

### Run Example

```sh
REPLANE_BASE_URL=http://localhost:8080 REPLANE_SDK_KEY=rp_... REPLANE_CONFIG=my-config \
    cargo run --example basic
```

## Pull Requests

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/your-feature`
3. Make your changes
4. Ensure tests pass: `cargo test`
5. Ensure linting passes: `cargo clippy --all-targets -- -D warnings`
6. Ensure code is formatted: `cargo fmt --check`
7. Commit your changes with a descriptive message
8. Push to your fork and submit a pull request

## Releasing

Run `./scripts/bump-version.sh`. It bumps the version in `Cargo.toml`, commits, and pushes a `vX.Y.Z` tag. CI then publishes the crate to crates.io and creates a GitHub release.

## Reporting Issues

Found a bug or have a feature request? Please [open an issue](https://github.com/replane-dev/replane-rust/issues) on GitHub.

## Community

Have questions or want to discuss Replane? Join the conversation in [GitHub Discussions](https://github.com/orgs/replane-dev/discussions).

## License

By contributing to Replane Rust SDK, you agree that your contributions will be licensed under the MIT License.
