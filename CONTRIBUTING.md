# Contributing

Thanks for helping with RKB RUST.

## Development setup

- Stable Rust (MSRV is declared in `Cargo.toml`).
- `rustfmt` and `clippy`.
- For Android work: Android SDK 35, NDK r26/r27, JDK 17+.

## Before opening a pull request

```bash
./scripts/verify.sh
```

This runs formatting, clippy with warnings denied, all tests, a release build,
and the headless demo. Android changes should also run:

```bash
./scripts/build-android.sh
cd android && ./gradlew :app:assembleDebug
```

## Guidelines

- Keep the core crates free of Android dependencies; platform-specific code
  belongs in `rkb-android`.
- Do not add network, shell, dynamic-code loading, or filesystem writes outside
  explicit tools.
- Preserve the "no third-party game compatibility" disclaimer. Do not claim
  Minecraft compatibility.
- Add tests for parsing, bounds checks, input state, rendering primitives, and
  archive handling.
- Keep public APIs documented and avoid `unwrap` in production paths.
- Follow the existing naming and error-handling style.
- Keep workflow permissions read-only and pin third-party actions.

## Branching

Work happens on feature branches (for example
`feature/rkb-rust-foundation`). Do not push directly to `main`, and do not
force-push shared branches.
