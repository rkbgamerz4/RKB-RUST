//! End-to-end checks of the `rkb-world` binary against synthetic 1.21 saves.

mod common;

use std::fs;
use std::process::{Command, Output};

use common::*;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rkb-world"))
        .args(args)
        .output()
        .expect("run rkb-world")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn synthetic_save() -> TempDir {
    let dir = TempDir::new("basic");
    let sections = vec![
        section_tag(-4, &[block("minecraft:bedrock")], &cells(&[])),
        section_tag(
            0,
            &[block("minecraft:air"), block("minecraft:stone")],
            &cells(&[(0, 1)]),
        ),
    ];
    let chunks = vec![(0, 0, chunk_tag(3955, 0, 0, sections))];
    write_world(dir.path(), &level_tag(3955, "CLI World"), &chunks);
    dir
}

#[test]
fn info_reports_version_and_counts() {
    let dir = synthetic_save();
    let output = run(&["info", &dir.path().display().to_string()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("name: CLI World"), "{text}");
    assert!(
        text.contains("minecraft: 1.21.1 (data version 3955)"),
        "{text}"
    );
    assert!(text.contains("protocol: 767"), "{text}");
    assert!(text.contains("packs: resource 34, data 48"), "{text}");
    assert!(text.contains("spawn: 0, 64, 0"), "{text}");
    assert!(text.contains("regions: 1"), "{text}");
    assert!(text.contains("chunks: 1"), "{text}");
}

#[test]
fn block_reports_states_and_unloaded_sections() {
    let dir = synthetic_save();
    let save = dir.path().display().to_string();

    let output = run(&["block", &save, "0", "-64", "0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "(0, -64, 0) = minecraft:bedrock");

    let output = run(&["block", &save, "0", "0", "0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "(0, 0, 0) = minecraft:stone");

    let output = run(&["block", &save, "0", "500", "0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "(0, 500, 0) = unloaded");

    let output = run(&["block", &save, "1000", "0", "0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "(1000, 0, 0) = unloaded");
}

#[test]
fn chunk_lists_sections() {
    let dir = synthetic_save();
    let save = dir.path().display().to_string();
    let output = run(&["chunk", &save, "0", "0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("chunk (0, 0): data version 3955"), "{text}");
    assert!(text.contains("sections: 2"), "{text}");
    assert!(
        text.contains("section -4: 1 states, 4096 non-air"),
        "{text}"
    );
    assert!(text.contains("section 0: 2 states, 1 non-air"), "{text}");

    let output = run(&["chunk", &save, "5", "5"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "chunk (5, 5) = not present");
}

#[test]
fn registry_lists_states_and_one_block() {
    let dir = TempDir::new("registry");
    let report = dir.path().join("blocks.json");
    fs::write(
        &report,
        r#"{
          "minecraft:air": { "states": [{ "id": 0, "default": true }] },
          "minecraft:stone": { "states": [{ "id": 1, "default": true }] },
          "minecraft:oak_log": {
            "states": [
              { "id": 2, "properties": { "axis": "x" } },
              { "id": 3, "default": true, "properties": { "axis": "y" } }
            ]
          }
        }"#,
    )
    .expect("write report");
    let path = report.display().to_string();

    let output = run(&["registry", &path]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("states: 4"), "{text}");
    assert!(text.contains("blocks: 3"), "{text}");
    assert!(
        text.contains("default minecraft:stone: minecraft:stone"),
        "{text}"
    );

    let output = run(&["registry", &path, "minecraft:oak_log"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("minecraft:oak_log[axis=x]"), "{text}");
    assert!(text.contains("minecraft:oak_log[axis=y]"), "{text}");

    let output = run(&["registry", &path, "minecraft:missing"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("is not in the registry"));
}

#[test]
fn unsupported_versions_fail_with_an_explanation() {
    let dir = TempDir::new("future");
    write_world(dir.path(), &level_tag(4080, "Future"), &[]);
    let output = run(&["info", &dir.path().display().to_string()]);
    assert_eq!(output.status.code(), Some(2));
    let text = stderr(&output);
    assert!(
        text.contains("unsupported Minecraft data version 4080"),
        "{text}"
    );
    assert!(text.contains("3953"), "{text}");
}

#[test]
fn bad_arguments_explain_the_usage() {
    let output = run(&[]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage:"));
    assert!(stdout(&output).contains("rkb-world info"));

    let output = run(&["block", "/nonexistent"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("missing x"));

    let output = run(&["block", "/nonexistent", "x", "0", "0"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("x must be an integer"));

    let output = run(&["nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("unknown command"));
}
