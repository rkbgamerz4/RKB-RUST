# Security Policy

## Supported versions

RKB RUST is pre-`1.0` and does not yet maintain long-term support branches. Fixes
are made on the active development branch and released as new commits/tags.

## Reporting a vulnerability

Please do **not** open a public issue for a security problem. Report it privately
to the repository owner through GitHub's private vulnerability reporting:

1. Open the repository's **Security** tab.
2. Choose **Report a vulnerability**.
3. Include the affected commit, a description, reproduction steps, and impact.

If private reporting is unavailable, contact the owner listed on the repository
profile. Please allow reasonable time for a fix before public disclosure.

## Scope

In scope:

- Unsafe code in `crates/rkb-android` (JNI handle handling, raw pointers,
  Android logging).
- Asset path handling, archive parsing, checksum validation, and extraction in
  `rkb-resource` / `rkb-pak`.
- Parsing of configuration and scene files.
- Frame-buffer and direct-buffer bounds handling.
- CI workflow permissions and dependency integrity.

Out of scope:

- The software renderer's visual output or performance.
- Missing GPU acceleration.
- Issues in upstream Rust, Android, Gradle, or Kotlin dependencies that are not
  exploitable through this project's code. Report those upstream.
- Any claim of compatibility with Minecraft or third-party games. The project
  does not provide such compatibility.

## Security properties

- No network, shell, scripting, or dynamic-code loading features.
- Asset paths are normalized and traversal-safe; extraction revalidates names.
- Archives validate magic, version, entry bounds, UTF-8, duplicates, and payload
  checksums before use.
- `FileSource` canonicalizes paths and rejects symlink escapes.
- Release builds use `panic = "abort"`; JNI entry points avoid unwinding across
  the FFI boundary.
- CI workflows use `permissions: contents: read` and pinned action commits.

## Known limitations

- JNI handles are raw pointers. Callers must pass valid handles from
  `nativeCreate` and must not use them after `nativeDestroy`.
- Native calls are not thread-safe by design; the Kotlin host confines them to a
  single render thread.
- The direct `ByteBuffer` capacity is checked, but callers must still provide a
  buffer matching the declared frame dimensions.
- The project has not undergone a formal third-party audit.
