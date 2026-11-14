//! `rkb-pak` packs, verifies, and unpacks `RKBPAK1` asset archives.
//!
//! The archive format is defined in `rkb-resource::archive`. Paths inside an
//! archive are normalized and traversal-safe, and every payload is checksummed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rkb_resource::{AssetSource, FileSource, PakArchive, PakWriter};

const USAGE: &str = "\
RKB RUST asset archive tool

Usage:
  rkb-pak pack <input-dir> <output.pak> [--force]
  rkb-pak list <archive.pak>
  rkb-pak verify <archive.pak>
  rkb-pak unpack <archive.pak> <output-dir> [--force]
  rkb-pak --help

Commands:
  pack      Build an archive from every regular file under <input-dir>
  list      List archive entries and their sizes
  verify    Parse an archive and validate every entry checksum
  unpack    Extract archive entries into <output-dir>
";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Pack {
        input: PathBuf,
        output: PathBuf,
        force: bool,
    },
    List {
        archive: PathBuf,
    },
    Verify {
        archive: PathBuf,
    },
    Unpack {
        archive: PathBuf,
        output: PathBuf,
        force: bool,
    },
}

fn parse_args() -> Result<Option<Command>, String> {
    parse_command(std::env::args().skip(1))
}

fn parse_command(mut args: impl Iterator<Item = String>) -> Result<Option<Command>, String> {
    let Some(command) = args.next() else {
        return Ok(None);
    };
    match command.as_str() {
        "-h" | "--help" | "help" => Ok(None),
        "pack" => {
            let input = args
                .next()
                .ok_or_else(|| "pack requires <input-dir>".to_owned())?;
            let output = args
                .next()
                .ok_or_else(|| "pack requires <output.pak>".to_owned())?;
            let force = take_force(&mut args)?;
            reject_extra(args)?;
            Ok(Some(Command::Pack {
                input: PathBuf::from(input),
                output: PathBuf::from(output),
                force,
            }))
        }
        "list" => {
            let archive = args
                .next()
                .ok_or_else(|| "list requires <archive.pak>".to_owned())?;
            reject_extra(args)?;
            Ok(Some(Command::List {
                archive: PathBuf::from(archive),
            }))
        }
        "verify" => {
            let archive = args
                .next()
                .ok_or_else(|| "verify requires <archive.pak>".to_owned())?;
            reject_extra(args)?;
            Ok(Some(Command::Verify {
                archive: PathBuf::from(archive),
            }))
        }
        "unpack" => {
            let archive = args
                .next()
                .ok_or_else(|| "unpack requires <archive.pak>".to_owned())?;
            let output = args
                .next()
                .ok_or_else(|| "unpack requires <output-dir>".to_owned())?;
            let force = take_force(&mut args)?;
            reject_extra(args)?;
            Ok(Some(Command::Unpack {
                archive: PathBuf::from(archive),
                output: PathBuf::from(output),
                force,
            }))
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

fn take_force(args: &mut impl Iterator<Item = String>) -> Result<bool, String> {
    let mut force = false;
    for arg in args.by_ref() {
        match arg.as_str() {
            "--force" | "-f" => force = true,
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(force)
}

fn reject_extra(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    match args.next() {
        Some(arg) => Err(format!("unexpected argument '{arg}'")),
        None => Ok(()),
    }
}

fn load_archive(path: &Path) -> Result<PakArchive, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    PakArchive::parse(bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn pack(input: &Path, output: &Path, force: bool) -> Result<(), String> {
    ensure_writable_file(output, force)?;
    let source = FileSource::new(input).map_err(|error| error.to_string())?;
    if source.list().is_empty() {
        return Err(format!(
            "{} contains no regular files to pack",
            input.display()
        ));
    }
    let bytes = PakWriter::from_source(&source).map_err(|error| error.to_string())?;
    write_output(output, &bytes)?;
    let archive = PakArchive::parse(bytes).map_err(|error| error.to_string())?;
    println!(
        "packed {} entries into {} ({} bytes)",
        archive.len(),
        output.display(),
        fs::metadata(output).map_or(0, |metadata| metadata.len())
    );
    Ok(())
}

fn list(path: &Path) -> Result<(), String> {
    let archive = load_archive(path)?;
    let mut total = 0_u64;
    for asset in archive.list() {
        let bytes = archive
            .read(&asset)
            .map_err(|error| format!("{}: {error}", asset))?;
        total += bytes.len() as u64;
        println!("{:>10}  {}", bytes.len(), asset);
    }
    println!("{} entries, {} bytes payload", archive.len(), total);
    Ok(())
}

fn verify(path: &Path) -> Result<(), String> {
    let archive = load_archive(path)?;
    let mut total = 0_u64;
    for asset in archive.list() {
        let bytes = archive
            .read(&asset)
            .map_err(|error| format!("{}: {error}", asset))?;
        total += bytes.len() as u64;
    }
    println!(
        "ok: {} entries, {} bytes payload, all checksums valid",
        archive.len(),
        total
    );
    Ok(())
}

fn unpack(archive_path: &Path, output: &Path, force: bool) -> Result<(), String> {
    let archive = load_archive(archive_path)?;
    fs::create_dir_all(output)
        .map_err(|error| format!("could not create {}: {error}", output.display()))?;

    let mut written = BTreeSet::new();
    for asset in archive.list() {
        let relative = asset.as_str();
        if !written.insert(relative.to_owned()) {
            return Err(format!("duplicate entry '{relative}'"));
        }
        let target = output.join(relative);
        ensure_writable_file(&target, force)?;
        let bytes = archive
            .read(&asset)
            .map_err(|error| format!("{}: {error}", asset))?;
        write_output(&target, &bytes)?;
    }
    println!(
        "unpacked {} entries into {}",
        written.len(),
        output.display()
    );
    Ok(())
}

fn ensure_writable_file(path: &Path, force: bool) -> Result<(), String> {
    if path.exists() && !force {
        return Err(format!(
            "{} already exists (pass --force to overwrite)",
            path.display()
        ));
    }
    Ok(())
}

fn write_output(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
    }
    fs::write(path, bytes).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Pack {
            input,
            output,
            force,
        } => pack(&input, &output, force),
        Command::List { archive } => list(&archive),
        Command::Verify { archive } => verify(&archive),
        Command::Unpack {
            archive,
            output,
            force,
        } => unpack(&archive, &output, force),
    }
}

fn main() -> ExitCode {
    match parse_args() {
        Ok(Some(command)) => match run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(None) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rkb-pak-{name}-{}-{stamp}", std::process::id()))
    }

    #[test]
    fn parses_commands_and_rejects_bad_options() {
        let parse = |args: &[&str]| parse_command(args.iter().map(|arg| (*arg).to_owned()));
        assert_eq!(
            parse(&["pack", "in", "out.pak", "--force"]).unwrap(),
            Some(Command::Pack {
                input: PathBuf::from("in"),
                output: PathBuf::from("out.pak"),
                force: true,
            })
        );
        assert!(parse(&["pack", "in"]).is_err());
        assert!(parse(&["pack", "in", "out", "--bogus"]).is_err());
        assert_eq!(parse(&["--help"]).unwrap(), None);
    }

    #[test]
    fn pack_list_verify_and_unpack_round_trip() {
        let root = scratch("round-trip");
        let input = root.join("input");
        let output = root.join("out.pak");
        let unpacked = root.join("unpacked");
        fs::create_dir_all(input.join("nested")).unwrap();
        fs::write(input.join("alpha.txt"), b"alpha").unwrap();
        fs::write(input.join("nested/beta.bin"), [0_u8, 1, 2, 3, 255]).unwrap();

        pack(&input, &output, false).unwrap();
        assert!(pack(&input, &output, false).is_err());
        pack(&input, &output, true).unwrap();
        list(&output).unwrap();
        verify(&output).unwrap();
        unpack(&output, &unpacked, false).unwrap();
        assert_eq!(fs::read(unpacked.join("alpha.txt")).unwrap(), b"alpha");
        assert_eq!(
            fs::read(unpacked.join("nested/beta.bin")).unwrap(),
            [0_u8, 1, 2, 3, 255]
        );
        assert!(unpack(&output, &unpacked, false).is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_corrupt_archives() {
        let root = scratch("corrupt");
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("bad.pak");
        fs::write(&archive, b"not an archive").unwrap();
        assert!(load_archive(&archive)
            .unwrap_err()
            .contains("invalid PAK archive"));
        let _ = fs::remove_dir_all(root);
    }
}
