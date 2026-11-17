//! Regression test against a chunk captured from the official 1.21.1 server.
//!
//! The fixture is the body of one `level_chunk_with_light` packet (chunk 0,0)
//! from a vanilla 1.21.1 flat world, captured with `rkb-play` and
//! `RKB_DUMP_CHUNK`. It pinned down one wire detail: a single-valued paletted
//! container still carries its (zero-length) long array, so it is three bytes,
//! not two. Without that byte the whole column was misaligned by two bytes per
//! air section.
//!
//! The data is world data from a local test server, not a Mojang asset.

use rkb_proto::packets::play::{ChunkColumn, ChunkData};

const CHUNK: &[u8] = include_bytes!("fixtures/chunk_0_0_1_21_1.bin");

#[test]
fn vanilla_flat_chunk_decodes_exactly() {
    let packet = ChunkData::decode(CHUNK).expect("chunk packet");
    assert_eq!((packet.x, packet.z), (0, 0));
    assert_eq!(packet.data.len(), 2245);
    assert!(packet.block_entities.is_empty());
    assert_eq!(packet.light.sky_light.len(), 2);
    assert_eq!(packet.light.sky_light[0].len(), 2048);
    assert!(packet.light.block_light.is_empty());

    let column =
        ChunkColumn::decode(packet.x, packet.z, -64, &packet.data, 24).expect("chunk sections");
    assert_eq!(column.sections.len(), 24);
    assert_eq!(column.non_air_blocks(), 1024);

    // The default 1.21.1 flat preset: bedrock at -64, dirt at -63 and -62,
    // grass block at -61, then air.
    assert_eq!(column.block_state(0, -64, 0), Some(79));
    assert_eq!(column.block_state(0, -63, 0), Some(10));
    assert_eq!(column.block_state(0, -62, 0), Some(10));
    assert_eq!(column.block_state(0, -61, 0), Some(9));
    assert_eq!(column.block_state(0, -60, 0), Some(0));
    assert_eq!(column.block_state(15, -61, 15), Some(9));

    // Outside the sent column or the dimension, nothing is inferred.
    assert_eq!(column.block_state(0, -80, 0), None);
    assert_eq!(column.block_state(0, 320, 0), None);
    assert_eq!(column.block_state(16, 0, 0), None);
}
