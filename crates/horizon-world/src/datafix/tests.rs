use super::*;
use crate::nbt::parse_snbt;

fn snbt(s: &str) -> Tag {
    parse_snbt(s).unwrap().0
}

#[test]
fn current_data_is_left_alone() {
    let mut tag = snbt("{DataVersion: 5023, Status: \"minecraft:full\"}");
    let before = tag.clone();
    update_chunk(&mut tag, "minecraft:overworld", Some("minecraft:noise"));
    assert_eq!(tag, before);
}

#[test]
fn item_stacks_of_1_20_take_components() {
    let mut player = snbt(
        "{DataVersion: 3465, Inventory: [{Slot: 0b, id: \"minecraft:diamond_sword\", Count: 1b, \
         tag: {Damage: 5, display: {Name: '{\"text\":\"Blade\"}'}}}]}",
    );
    update_player(&mut player);
    let stack = &dynamic::stream(player.get("Inventory"))[0];
    assert_eq!(
        dynamic::as_str(stack.get("id")).as_deref(),
        Some("minecraft:diamond_sword")
    );
    assert_eq!(stack.get("count"), Some(&Tag::Int(1)));
    let components = stack.get("components").expect("components");
    assert_eq!(components.get("minecraft:damage"), Some(&Tag::Int(5)));
    assert_eq!(
        components.get("minecraft:custom_name"),
        Some(&Tag::compound([("text", dynamic::string("Blade"))]))
    );
}

#[test]
fn level_data_of_1_20_moves_to_the_new_fields() {
    let mut level = snbt(
        "{DataVersion: 3465, Difficulty: 3b, DifficultyLocked: 1b, hardcore: 0b, SpawnX: 1, SpawnY: 64, SpawnZ: -3, \
         SpawnAngle: 90.0f, GameRules: {doFireTick: \"false\", keepInventory: \"true\", randomTickSpeed: \"5\"}}",
    );
    update(Ty::Level, &mut level, 3465, CURRENT);
    let settings = level
        .get("difficulty_settings")
        .expect("difficulty settings");
    assert_eq!(
        dynamic::as_str(settings.get("difficulty")).as_deref(),
        Some("hard")
    );
    assert_eq!(settings.get("locked"), Some(&Tag::Byte(1)));
    let rules = level.get("game_rules").expect("game rules");
    assert_eq!(rules.get("minecraft:keep_inventory"), Some(&Tag::Byte(1)));
    assert_eq!(rules.get("minecraft:random_tick_speed"), Some(&Tag::Int(5)));
    assert_eq!(
        rules.get("minecraft:fire_spread_radius_around_player"),
        Some(&Tag::Int(0))
    );
    assert_eq!(
        level.get("spawn").and_then(|s| s.get("pos")),
        Some(&Tag::IntArray(vec![1, 64, -3]))
    );
}

#[test]
fn block_states_take_their_new_field_names() {
    let mut chunk = snbt(
        "{DataVersion: 4903, Status: \"minecraft:full\", sections: [{Y: 0b, block_states: \
         {palette: [{Name: \"minecraft:chain\", Properties: {axis: \"y\"}}]}}]}",
    );
    update_chunk(&mut chunk, "minecraft:the_nether", Some("minecraft:noise"));
    let section = &dynamic::stream(chunk.get("sections"))[0];
    let state = &dynamic::stream(section.get("block_states").and_then(|b| b.get("palette")))[0];
    assert_eq!(
        dynamic::as_str(state.get("id")).as_deref(),
        Some("minecraft:chain")
    );
    assert!(state.get("properties").is_some());
    assert_eq!(chunk.get("DataVersion"), Some(&Tag::Int(CURRENT)));
}

/// A conversion that stops on a bad chunk leaves the old DataVersion in
/// level.dat, so the next open retries instead of accepting a mix of a
/// current level.dat and old-format chunks (#3882); a successful one stamps
/// it only once every file is converted.
#[test]
fn a_failed_conversion_leaves_the_old_data_version() {
    use crate::compression::Compression;
    use crate::nbt::{Limits, NamedTag};
    use crate::region::{ChunkRecord, RegionFile, RegionWriter};
    use horizon_core::position::ChunkPos;
    use std::io::{Read, Seek, SeekFrom, Write};
    const VERSION: i32 = 4903;
    let version_of = |world: &std::path::Path| {
        files::read_dat(&world.join("level.dat"))
            .unwrap()
            .and_then(|root| root.get("Data").cloned())
            .and_then(|data| data.get("DataVersion").cloned())
            .map(|tag| match tag {
                Tag::Int(v) => v,
                _ => panic!("DataVersion is not an int"),
            })
    };
    let root = std::env::temp_dir().join(format!("horizon-upgrade-stamp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for good in [true, false] {
        let world = root.join(if good { "good" } else { "bad" });
        std::fs::create_dir_all(&world).unwrap();
        files::write_dat(
            &world.join("level.dat"),
            &Tag::compound([("Data", snbt(&format!("{{DataVersion: {VERSION}}}")))]),
        )
        .unwrap();
        let folder = world.join("dimensions/minecraft/overworld/region");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("r.0.0.mca");
        let record = ChunkRecord::from_nbt(
            &NamedTag {
                name: "".into(),
                tag: snbt("{DataVersion: 4903, Status: \"minecraft:full\"}"),
            },
            Compression::Zlib,
            1,
            Limits::default(),
        )
        .unwrap();
        let mut writer = RegionWriter::create(&path).unwrap();
        writer.write(ChunkPos::new(0, 0), &record).unwrap();
        writer.finish().unwrap();
        if !good {
            // A length the pointed sectors cannot hold: reading the chunk fails.
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut entry = [0; 4];
            file.seek(SeekFrom::Start(0)).unwrap();
            file.read_exact(&mut entry).unwrap();
            let start = u64::from(u32::from_be_bytes(entry) >> 8) * 4096;
            file.seek(SeekFrom::Start(start)).unwrap();
            file.write_all(&100_000u32.to_be_bytes()).unwrap();
            file.write_all(&[2]).unwrap();
        }
        let result = files::upgrade_world(&world);
        if good {
            assert_eq!(result.unwrap(), VERSION);
            assert_eq!(version_of(&world), Some(CURRENT));
            let mut region = RegionFile::open(&path).unwrap();
            let chunk = region
                .read(ChunkPos::new(0, 0))
                .unwrap()
                .expect("the chunk survives")
                .nbt(Limits::default())
                .unwrap();
            assert_eq!(chunk.tag.get("DataVersion"), Some(&Tag::Int(CURRENT)));
        } else {
            assert!(result.is_err(), "the bad chunk fails the upgrade");
            assert_eq!(
                version_of(&world),
                Some(VERSION),
                "level.dat keeps the old version for the retry"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}
