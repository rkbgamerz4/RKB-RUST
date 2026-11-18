# Third-party notices

RKB RUST is licensed under MIT OR Apache-2.0. This file records the projects
whose code or ideas influenced RKB RUST and the crates distributed with it.

## Minecraft

Minecraft, Minecraft: Java Edition, and Mojang are trademarks of Mojang
Studios. RKB RUST contains no Minecraft code, jar, asset, texture, sound, data
file, or world. The `rkb-mc` crate reads files the user supplies from their own
installation. Do not distribute Mojang files with RKB RUST.

## Horizon (reference project)

Horizon is a Rust client for Minecraft: Java Edition, used as an architectural
reference for this project.

- Upstream project: <https://github.com/horizon-rust/horizon>
- Inspected archive: the Android port, <https://github.com/MocLG/horizon-android>
- License: MIT
- Copyright (c) 2026 Ludovic Marie and the Horizon contributors

No Horizon source code was copied into RKB RUST. The analysis in
[`docs/horizon-analysis.md`](docs/horizon-analysis.md) discusses its architecture
and records the license findings. The MIT notice is reproduced here for
completeness and attribution:

```text
MIT License

Copyright (c) 2026 Ludovic Marie and the Horizon contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Horizon's own `THIRD_PARTY_NOTICES.md` lists components that RKB RUST does **not**
bundle or copy:

- wgpu-hal 30.0.1 — MIT or Apache-2.0;
- Pixelify Sans — SIL OFL 1.1;
- Pixelarticons — MIT;
- Zalith Launcher 2 default control layout — GPL-3.0;
- OpenJDK 25 Android runtime — GPL-2.0 with Classpath Exception.

RKB RUST does not copy the Zalith layout, does not bundle a Java runtime, and
does not include those fonts or icons.

## Rust crates used by `rkb-mc` and `rkb-minecraft`

These versions are the ones recorded in `Cargo.lock` for the current build.
Licenses are taken from each crate's metadata.

| Crate | Version | License |
| --- | ---: | --- |
| `flate2` | 1.1.11 | MIT OR Apache-2.0 |
| `miniz_oxide` | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| `miniz_oxide` (inflate for `rkb-minecraft` PNG/ZIP reading) | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| `crc32fast` | 1.5.5 | MIT OR Apache-2.0 |
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| `simd-adler32` | 0.3.10 | MIT |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 |
| `itoa` | 1.0.19 | MIT OR Apache-2.0 |
| `memchr` | 2.8.3 | Unlicense OR MIT |
| `zmij` | 1.0.23 | MIT |

`rkb-minecraft` reads user-supplied resource packs with its own PNG and ZIP
parsers; `miniz_oxide` only performs the DEFLATE/zlib decompression and
compression those parsers need. No `png`, `zip`, or `image` crate is used, and
no Minecraft asset is generated, bundled, or redistributed by RKB RUST: every
texture a pack supplies comes from the pack the user points at, and every
texture RKB draws itself is drawn by RKB code.

`serde_json` may resolve additional `serde`/`syn` build or optional
dependencies in `Cargo.lock`; those are MIT OR Apache-2.0 (and
`unicode-ident` is MIT OR Apache-2.0 AND Unicode-3.0) and are not modified by
RKB RUST.

The rest of the RKB RUST workspace has its own dependency list; see
`Cargo.lock` and the individual crate manifests. No third-party source has been
patched or vendored in this change.
