//! This is a helper script, not really an example.
//! We keep it in the examples/ subdirectory rather than
//! src/bin/ because we don't want it to be installed by
//! 'cargo install'.

use std::{
    env,
    fs::{self, File},
    io::{self, BufRead, BufReader},
    path::Path,
};

use anyhow::{bail, Context, Result as AResult};

use itertools::Itertools;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
";

fn main() -> AResult<()> {
    let mut args = env::args().skip(1);
    let Some(cmd) = args.next() else {
        bail!("{USAGE}");
    };

    match cmd.as_str() {
        "version" => version(args),
        "checktag" => checktag(args),
        "relnotes" => relnotes(args),
        "checkreadme" => checkreadme(args),
        _ => bail!("Unknown command: {cmd}\n{USAGE}"),
    }
}

fn version(mut args: impl Iterator<Item = String>) -> AResult<()> {
    let template = args.next().unwrap_or("".into());
    let (prefix, suffix) = if let Some(idx) = template.find('@') {
        (&template[..idx], &template[idx + 1..])
    } else {
        (template.as_str(), "")
    };
    println!("{prefix}{VERSION}{suffix}");
    Ok(())
}

fn checktag(mut args: impl Iterator<Item = String>) -> AResult<()> {
    let Some(tag) = args.next() else {
        bail!("Please pass a tag name to check against the current version\n{USAGE}");
    };

    let expected = format!("v{VERSION}");

    if tag == expected {
        println!("Tag {tag} matches version number {VERSION}");
        Ok(())
    } else {
        bail!("Tag {tag:?} does not match version number {VERSION:?}");
    }
}

fn relnotes(mut args: impl Iterator<Item = String>) -> AResult<()> {
    if let Some(unexpected) = args.next() {
        bail!("Unexpected argument: {unexpected:?}");
    }

    let changelog_file = "CHANGELOG.md";
    let lines = read_file(changelog_file)?;

    let parsed_version = semver::Version::parse(VERSION)
        .with_context(|| "Could not parse version number {VERSION:?}")?;
    let is_prerelease = parsed_version.pre.is_empty();
    let look_for = if is_prerelease {
        VERSION
    } else {
        "NEXTVERSION"
    };
    let header = format!("## mapiproxy {look_for} - ");

    let Some(start) = lines.iter().position(|line| line.starts_with(&header)) else {
        bail!("Could not find header {header:?} in {changelog_file}");
    };

    let mut buffer = String::new();
    for line in &lines[start + 1..] {
        if line.starts_with('#') {
            break;
        }
        buffer.push('\n');
        buffer.push_str(line);
    }

    println!("{}", buffer.trim());
    Ok(())
}

fn read_file(path: impl AsRef<Path>) -> AResult<Vec<String>> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("Could not open {}", path.display()))?;
    let br = BufReader::new(file);
    let content: io::Result<Vec<_>> = br.lines().collect();
    Ok(content?)
}

fn read_filex(path: impl AsRef<Path>) -> AResult<String> {
    let path = path.as_ref();
    fs::read_to_string(path).with_context(|| format!("Could not read {}", path.display()))
}

fn checkreadme(mut args: impl Iterator<Item = String>) -> AResult<()> {
    const README_FILE: &str = "README.md";
    const START_MARKER: &str = "```plain";
    const END_MARKER: &str = "```";

    let do_fix = match args.next() {
        None => false,
        Some(a) if a == "--fix" => true,
        Some(other) => bail!("Unknown argument: {other}"),
    };

    // Parse the existing readme file
    let old_readme = read_filex(README_FILE)?;
    let old_lines = old_readme.split_inclusive('\n').collect_vec();
    let Some(start) = old_lines
        .iter()
        .position(|line| line.starts_with(START_MARKER))
    else {
        bail!("Could not find {START_MARKER:?} in {old_readme}");
    };
    let start = start + 1;
    let Some(rel_end) = old_lines[start..]
        .iter()
        .position(|line| line.starts_with(END_MARKER))
    else {
        bail!(
            "Could not find {END_MARKER:?} in {old_readme} after line {n}",
            n = start + 1
        );
    };
    let end = start + rel_end;

    let mut old_usage = String::with_capacity(old_readme.len());
    for line in &old_lines[start..end] {
        old_usage.push_str(line);
    }

    let new_usage = include_str!("../src/usage.txt");

    if old_usage == new_usage {
        return Ok(());
    }

    println!("{old_usage:?}");

    println!("Usage information has changed: ");
    for diff in diff::lines(&old_usage, new_usage) {
        match diff {
            diff::Result::Left(line) => println!("-{line}"),
            diff::Result::Both(line, _) => println!(" {line}"),
            diff::Result::Right(line) => println!("+{line}"),
        }
    }

    if !do_fix {
        bail!("Usage info in {README_FILE} does not match --help output");
    }

    write_file(
        README_FILE,
        &[&old_lines[..start], &[new_usage], &old_lines[end..]],
    )
}

fn write_file(path: impl AsRef<Path>, content: &[&[&str]]) -> AResult<()> {
    let path = path.as_ref();
    let mut text = String::new();
    for chunk in content {
        for line in *chunk {
            text.push_str(line);
        }
    }

    fs::write(path, text).with_context(|| format!("Could not write {}", path.display()))?;
    Ok(())
}
