//! Static contract check between the Kotlin declarations and JNI exports.
//!
//! A JNI name or signature mismatch only fails at runtime on a device, so this
//! test compares the two source files directly. It runs on every host CI job.

use std::fs;
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
struct KotlinFunction {
    name: String,
    params: Vec<String>,
    returns: String,
}

#[derive(Debug, PartialEq)]
struct RustExport {
    name: String,
    params: Vec<String>,
    returns: String,
}

fn source_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn extract_args(src: &str, open: usize) -> (Vec<String>, usize) {
    let mut depth = 0_i32;
    let mut current_start = open + 1;
    let mut args = Vec::new();
    let mut index = open;
    while index < src.len() {
        let character = src[index..].chars().next().expect("valid char");
        match character {
            '(' => {
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    push_arg(&mut args, &src[current_start..index]);
                    return (args, index);
                }
            }
            ',' if depth == 1 => {
                push_arg(&mut args, &src[current_start..index]);
                current_start = index + 1;
            }
            _ => {}
        }
        index += character.len_utf8();
    }
    panic!("unterminated argument list");
}

fn push_arg(args: &mut Vec<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        args.push(text.to_owned());
    }
}

fn kotlin_declarations(src: &str) -> Vec<KotlinFunction> {
    let mut functions = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = src[cursor..].find("external fun ") {
        let start = cursor + offset + "external fun ".len();
        let name_end = src[start..]
            .find(|character: char| !character.is_alphanumeric() && character != '_')
            .map_or(src.len(), |offset| start + offset);
        let name = src[start..name_end].to_owned();
        let open = src[name_end..].find('(').expect("function has parameters") + name_end;
        let (raw_params, close) = extract_args(src, open);

        let params = raw_params
            .iter()
            .map(|param| {
                let (_, kind) = param.split_once(':').expect("parameter has a type");
                kind.trim().to_owned()
            })
            .collect();

        let tail = src[close + 1..].trim_start();
        let returns = tail.strip_prefix(':').map_or_else(
            || "Unit".to_owned(),
            |rest| rest.lines().next().unwrap_or("Unit").trim().to_owned(),
        );

        functions.push(KotlinFunction {
            name,
            params,
            returns,
        });
        cursor = close + 1;
    }
    functions
}

fn rust_exports(src: &str) -> Vec<RustExport> {
    let marker = "fn Java_com_rkb_rust_RkbNative_";
    let mut exports = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = src[cursor..].find(marker) {
        let start = cursor + offset + marker.len();
        let name_end = src[start..].find('(').expect("export has parameters") + start;
        let name = src[start..name_end].trim().to_owned();
        let open = name_end;
        let (raw_params, close) = extract_args(src, open);

        let params = raw_params
            .iter()
            .map(|param| {
                let (_, kind) = param.split_once(':').expect("parameter has a type");
                kind.trim()
                    .trim_start_matches('&')
                    .trim_start_matches("mut ")
                    .trim()
                    .to_owned()
            })
            .collect();

        let tail = src[close + 1..].trim_start();
        let returns = tail.strip_prefix("->").map_or_else(
            || "()".to_owned(),
            |rest| rest.split('{').next().unwrap_or("()").trim().to_owned(),
        );

        exports.push(RustExport {
            name,
            params,
            returns,
        });
        cursor = close + 1;
    }
    exports
}

fn kotlin_type(jni: &str) -> &'static str {
    match jni {
        "jlong" => "Long",
        "jint" => "Int",
        "jboolean" => "Boolean",
        "f32" => "Float",
        "JByteBuffer" => "java.nio.ByteBuffer",
        "JObject" => "android.view.Surface",
        "jstring" => "String",
        "()" => "Unit",
        other => panic!("unmapped JNI type '{other}'"),
    }
}

/// Kotlin may mark a reference type nullable (`String?`); the JNI signature is
/// identical, so nullability is not part of the contract.
fn nullable_free(value: &str) -> &str {
    value.trim().trim_end_matches('?')
}

#[test]
fn kotlin_declarations_match_jni_exports() {
    let kotlin = fs::read_to_string(source_path(
        "android/app/src/main/java/com/rkb/rust/RkbNative.kt",
    ))
    .expect("read RkbNative.kt");
    let rust = fs::read_to_string(source_path("crates/rkb-android/src/lib.rs"))
        .expect("read rkb-android/src/lib.rs");

    let declarations = kotlin_declarations(&kotlin);
    assert!(
        declarations.len() >= 8,
        "expected the full JNI surface, found {} declarations",
        declarations.len()
    );

    for declaration in &declarations {
        let export = rust_exports(&rust)
            .into_iter()
            .find(|export| export.name == declaration.name)
            .unwrap_or_else(|| panic!("missing JNI export for native{}", declaration.name));

        // The first two parameters are always the JNI environment and class.
        assert!(
            export.params.len() >= 2,
            "export {} is missing JNI parameters",
            declaration.name
        );
        let mapped: Vec<&str> = export.params[2..]
            .iter()
            .map(|kind| kotlin_type(kind))
            .collect();
        let expected: Vec<&str> = declaration
            .params
            .iter()
            .map(|param| nullable_free(param))
            .collect();
        assert_eq!(
            mapped, expected,
            "parameter mismatch for native{}",
            declaration.name
        );
        assert_eq!(
            kotlin_type(&export.returns),
            nullable_free(&declaration.returns),
            "return type mismatch for native{}",
            declaration.name
        );
    }
}

#[test]
fn every_export_has_a_kotlin_declaration() {
    let kotlin = fs::read_to_string(source_path(
        "android/app/src/main/java/com/rkb/rust/RkbNative.kt",
    ))
    .expect("read RkbNative.kt");
    let rust = fs::read_to_string(source_path("crates/rkb-android/src/lib.rs"))
        .expect("read rkb-android/src/lib.rs");

    let declarations = kotlin_declarations(&kotlin);
    for export in rust_exports(&rust) {
        assert!(
            declarations
                .iter()
                .any(|declaration| declaration.name == export.name),
            "orphan JNI export: {}",
            export.name
        );
    }
}
