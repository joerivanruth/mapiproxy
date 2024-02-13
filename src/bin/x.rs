use std::{env, fs};

use anyhow::{bail, Context, Result as AResult};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
    Usage: cargo run --bin x -- CMD [ARGS]
    Cmd:
        x version [TEMPLATE]        template is prefix or has '@' to be replaced with version
        checktag TAG                check if tag (v1.2.3) matches version number 1.2.3
        relnotes                    extract current entry from CHANGELOG.md
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
    let changelog = fs::read_to_string(changelog_file)
        .with_context(|| format!("Could not read {changelog_file}"))?;
    let lines: Vec<_> = changelog.lines().collect();

    let header = format!("## mapiproxy {VERSION} - ");
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
