#!/usr/bin/env python3
"""Builds the self-authored resource pack used for M6 verification.

Every pixel and every JSON file here is written by this script: the pack is
synthetic on purpose, so no Minecraft/Mojang asset appears anywhere in RKB's
verification evidence.
"""
import json
import os
import struct
import sys
import zlib

# Output directory: first argument, or "mcpack" in the working directory.
ROOT = sys.argv[1] if len(sys.argv) > 1 else "mcpack"
SIZE = 16


def png(path, pixels):
    """pixels: list of rows, each a list of (r, g, b, a)."""
    raw = b""
    for row in pixels:
        raw += b"\x00" + b"".join(struct.pack("4B", *px) for px in row)

    def chunk(kind, data):
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    out += chunk(b"IDAT", zlib.compress(raw, 9))
    out += chunk(b"IEND", b"")
    with open(path, "wb") as fh:
        fh.write(out)


def speckle(base, dark, seed, density=0.35):
    """Deterministic two-tone 16x16 pattern; no randomness, no game art."""
    base = (base[0], base[1], base[2], 255)
    dark = (dark[0], dark[1], dark[2], 255)
    rows = []
    for y in range(SIZE):
        row = []
        for x in range(SIZE):
            h = (x * 73856093) ^ (y * 19349663) ^ (seed * 83492791)
            row.append(dark if (h % 100) < int(density * 100) else base)
        rows.append(row)
    return rows


def border(rows, color):
    for i in range(SIZE):
        rows[0][i] = color
        rows[SIZE - 1][i] = color
        rows[i][0] = color
        rows[i][SIZE - 1] = color
    return rows


def grass_side():
    rows = speckle((134, 96, 67), (110, 76, 50), 5, 0.30)
    for y in range(4):
        for x in range(SIZE):
            rows[y][x] = (
                (66, 168, 50, 255) if (x + y) % 3 else (48, 132, 38, 255)
            )
    return rows


def transparent(pattern_fn):
    rows = pattern_fn()
    for y in range(SIZE):
        for x in range(SIZE):
            if 2 <= x <= SIZE - 3 and 2 <= y <= SIZE - 3:
                r, g, b, _ = rows[y][x]
                rows[y][x] = (r, g, b, 0)
    return border(rows, (220, 240, 255, 255))


def write(rel, pixels):
    path = os.path.join(ROOT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    png(path, pixels)


def write_json(rel, value):
    path = os.path.join(ROOT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        json.dump(value, fh, separators=(",", ":"))


def cube_all(model, texture):
    # Vanilla's block/cube.json puts "cullface" on every face: a full cube's
    # six faces are all boundary faces, so a touching neighbour hides them.
    write_json(
        f"assets/minecraft/models/block/{model}.json",
        {
            "elements": [
                {
                    "from": [0, 0, 0],
                    "to": [16, 16, 16],
                    "faces": {
                        face: {"texture": "#all", "cullface": face}
                        for face in ("down", "up", "north", "south", "east", "west")
                    },
                }
            ],
            "textures": {"all": texture},
        },
    )


def main():
    os.makedirs(ROOT, exist_ok=True)
    with open(os.path.join(ROOT, "pack.mcmeta"), "w") as fh:
        json.dump(
            {
                "pack": {
                    "pack_format": 34,
                    "description": "RKB M6 self-authored verification pack",
                }
            },
            fh,
        )

    write("assets/minecraft/textures/block/grass_block_top.png", speckle((96, 168, 60), (72, 138, 44), 1))
    write("assets/minecraft/textures/block/grass_block_side.png", grass_side())
    write("assets/minecraft/textures/block/dirt.png", speckle((134, 96, 67), (110, 76, 50), 2))
    write("assets/minecraft/textures/block/stone.png", speckle((128, 128, 128), (104, 104, 104), 3))
    write("assets/minecraft/textures/block/bedrock.png", speckle((70, 70, 70), (38, 38, 38), 4, 0.5))
    write(
        "assets/minecraft/textures/block/glowstone.png",
        border(speckle((255, 214, 92), (226, 170, 60), 6, 0.4), (160, 120, 40, 255)),
    )
    write("assets/minecraft/textures/block/glass.png", transparent(lambda: speckle((200, 230, 245), (170, 205, 230), 7, 0.1)))
    write("assets/minecraft/textures/block/ice.png", transparent(lambda: speckle((150, 200, 255), (120, 175, 240), 8, 0.15)))

    for name, texture in (
        ("stone", "block/stone"),
        ("bedrock", "block/bedrock"),
        ("glowstone", "block/glowstone"),
        ("glass", "block/glass"),
        ("ice", "block/ice"),
    ):
        write_json(
            f"assets/minecraft/blockstates/{name}.json",
            {"variants": {"": {"model": f"minecraft:block/{name}"}}},
        )
        cube_all(name, texture)

    write_json(
        "assets/minecraft/blockstates/dirt.json",
        {"variants": {"": {"model": "minecraft:block/dirt"}}},
    )
    cube_all("dirt", "block/dirt")

    write_json(
        "assets/minecraft/blockstates/grass_block.json",
        {"variants": {"": {"model": "minecraft:block/grass_block"}}},
    )
    write_json(
        "assets/minecraft/models/block/grass_block.json",
        {
            "textures": {
                "top": "block/grass_block_top",
                "side": "block/grass_block_side",
                "bottom": "block/dirt",
            },
            "elements": [
                {
                    "from": [0, 0, 0],
                    "to": [16, 16, 16],
                    "faces": {
                        face: {"texture": tex, "cullface": face}
                        for face, tex in (
                            ("down", "#bottom"),
                            ("up", "#top"),
                            ("north", "#side"),
                            ("south", "#side"),
                            ("east", "#side"),
                            ("west", "#side"),
                        )
                    },
                }
            ],
        },
    )

    # A non-cube shape from pack data: a bottom slab, 8 model units tall.
    # Vanilla's block/slab.json culls every face except "up": the top of a
    # bottom slab is never a boundary face against the block above it.
    write_json(
        "assets/minecraft/blockstates/stone_slab.json",
        {"variants": {"": {"model": "minecraft:block/stone_slab"}}},
    )
    write_json(
        "assets/minecraft/models/block/stone_slab.json",
        {
            "textures": {"all": "block/stone"},
            "elements": [
                {
                    "from": [0, 0, 0],
                    "to": [16, 8, 16],
                    "faces": {
                        face: {"texture": "#all", **({"cullface": face} if face != "up" else {})}
                        for face in ("down", "up", "north", "south", "east", "west")
                    },
                }
            ],
        },
    )

    print("wrote pack to", ROOT)


if __name__ == "__main__":
    main()
