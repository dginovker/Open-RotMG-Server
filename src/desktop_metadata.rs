use anyhow::{Result, bail, ensure};
use serde::Serialize;
use std::{io::Write, path::Path};

#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

const MAX_ACK: usize = 16 * 1024;
const FAILURE: &str =
    "Desktop metadata publication failed; outcome unknown. Do not replay automatically.";
#[derive(Clone, Copy, Debug, PartialEq)]
enum Operation {
    PublishReplace,
    PublishDistinct,
    MoveDistinct,
    CreateContainer,
}
impl Operation {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "publish-replace" => Ok(Self::PublishReplace),
            "publish-distinct" => Ok(Self::PublishDistinct),
            "move-distinct" => Ok(Self::MoveDistinct),
            "create-container" => Ok(Self::CreateContainer),
            _ => bail!("unknown desktop metadata operation"),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::PublishReplace => "publish-replace",
            Self::PublishDistinct => "publish-distinct",
            Self::MoveDistinct => "move-distinct",
            Self::CreateContainer => "create-container",
        }
    }
}
struct Request {
    operation: Operation,
    root: String,
    source: Option<String>,
    destination: String,
    kind: &'static str,
}
fn hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| matches!(b,b'0'..=b'9'|b'a'..=b'f'))
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                matches!(b,b'0'..=b'9'|b'a'..=b'f')
            }
        })
}
fn stage(value: &str) -> bool {
    value.strip_prefix("generation-").is_some_and(|v| {
        !v.is_empty() && v.len() <= 96 && v.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}
fn generation(value: &str) -> bool {
    value.strip_prefix("source-").is_some_and(hex)
        || value
            .strip_prefix("release-")
            .and_then(|v| v.split_once('-'))
            .is_some_and(|(n, h)| {
                !n.is_empty()
                    && n.len() <= 16
                    && !n.starts_with('0')
                    && n.bytes().all(|b| b.is_ascii_digit())
                    && hex(h)
            })
}
fn portable(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value.split('/').all(|c| {
            !c.is_empty()
                && c.len() <= 180
                && c != "."
                && c != ".."
                && !c.ends_with('.')
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        })
}
fn control_file(value: &str) -> bool {
    let parts: Vec<_> = value.split('/').collect();
    match parts.as_slice() {
        [name] => [
            "runtime-profile.json",
            "runtime-selection.json",
            "runtime-pending.json",
            "runtime-recovery.json",
            "runtime-activity.json",
            "runtime-check-refusal.json",
            "shutdown-failed.json",
        ]
        .contains(name),
        ["runtime-recovery-history", name] => name
            .strip_suffix(".pending.json")
            .or_else(|| name.strip_suffix(".activity.json"))
            .or_else(|| name.strip_suffix(".refusal.json"))
            .or_else(|| name.strip_suffix(".json"))
            .is_some_and(hex),
        ["diagnostics", "failure-history", name] => name.strip_suffix(".json").is_some_and(hex),
        ["runtime-manifests", name] => name
            .strip_suffix(".json")
            .or_else(|| name.strip_suffix(".sig"))
            .and_then(|v| v.split_once('-'))
            .is_some_and(|(n, h)| {
                !n.is_empty()
                    && n.len() <= 16
                    && !n.starts_with('0')
                    && n.bytes().all(|b| b.is_ascii_digit())
                    && hex(h)
            }),
        ["runtime-staging", directory, name] => {
            stage(directory) && ["manifest.json", "manifest.sig", "generation.json"].contains(name)
        }
        _ => false,
    }
}
fn container(value: &str) -> bool {
    matches!(
        value,
        "runtimes"
            | "runtime-staging"
            | "runtime-manifests"
            | "runtime-recovery-history"
            | "diagnostics"
            | "diagnostics/failure-history"
    ) || value.strip_prefix("runtime-staging/").is_some_and(stage)
}
fn parse(args: &[String]) -> Result<Request> {
    let operation = Operation::parse(args.get(1).map(String::as_str).unwrap_or(""))?;
    let creating = operation == Operation::CreateContainer;
    ensure!(
        args.len() == if creating { 4 } else { 5 },
        "invalid desktop metadata argument count"
    );
    let root = &args[2];
    ensure!(
        Path::new(root).is_absolute()
            && root.len() <= 4096
            && !root.chars().any(char::is_control)
            && Path::new(root).components().all(|c| !matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )),
        "invalid desktop metadata root"
    );
    let destination = &args[if creating { 3 } else { 4 }];
    ensure!(
        portable(destination),
        "invalid desktop metadata destination"
    );
    let source = if creating {
        None
    } else {
        ensure!(portable(&args[3]), "invalid desktop metadata source");
        Some(args[3].clone())
    };
    let kind = match operation {
        Operation::PublishReplace | Operation::PublishDistinct => {
            ensure!(
                control_file(destination),
                "destination is not a control record"
            );
            ensure!(
                source
                    .as_deref()
                    .and_then(|s| s.strip_prefix(destination))
                    .and_then(|s| s.strip_prefix(".partial-"))
                    .is_some_and(uuid),
                "source is not the matching private control-record temporary"
            );
            "file"
        }
        Operation::CreateContainer => {
            ensure!(
                container(destination),
                "destination is not a control container"
            );
            "directory"
        }
        Operation::MoveDistinct => {
            let retirement = match source.as_deref() {
                Some("runtime-pending.json") => Some(".pending.json"),
                Some("runtime-activity.json") => Some(".activity.json"),
                Some("runtime-check-refusal.json") => Some(".refusal.json"),
                _ => None,
            };
            if let Some(suffix) = retirement {
                ensure!(
                    destination
                        .strip_prefix("runtime-recovery-history/")
                        .and_then(|n| n.strip_suffix(suffix))
                        .is_some_and(hex),
                    "invalid control-history destination"
                );
                "file"
            } else {
                ensure!(
                    source
                        .as_deref()
                        .and_then(|s| s.strip_prefix("runtime-staging/"))
                        .is_some_and(stage)
                        && destination
                            .strip_prefix("runtimes/")
                            .is_some_and(generation),
                    "invalid complete-generation move"
                );
                "directory"
            }
        }
    };
    Ok(Request {
        operation,
        root: root.clone(),
        source,
        destination: destination.clone(),
        kind,
    })
}
#[derive(Serialize)]
struct Acknowledgement<'a> {
    protocol: u8,
    operation: &'static str,
    process: u32,
    root: &'a str,
    source: Option<&'a str>,
    destination: &'a str,
    kind: &'static str,
    published: bool,
}
fn acknowledgement(request: &Request) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&Acknowledgement {
        protocol: 1,
        operation: request.operation.name(),
        process: std::process::id(),
        root: &request.root,
        source: request.source.as_deref(),
        destination: &request.destination,
        kind: request.kind,
        published: true,
    })?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_ACK,
        "desktop metadata acknowledgement too large"
    );
    Ok(bytes)
}
fn write_ack(writer: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    writer.write_all(bytes)?;
    writer.flush()
}
#[cfg(not(windows))]
fn execute(_: &Request) -> Result<()> {
    bail!("desktop metadata publication is supported only on Windows")
}
#[cfg(windows)]
fn execute(request: &Request) -> Result<()> {
    windows::execute(request, std::fs::File::sync_all)
}

pub(crate) fn run(args: &[String]) -> Result<bool> {
    if args.first().map(String::as_str) != Some("--desktop-metadata") {
        return Ok(false);
    }
    let operation = args
        .get(1)
        .and_then(|v| Operation::parse(v).ok())
        .map(Operation::name)
        .unwrap_or("invalid");
    let mut started = false;
    let mut stdout = std::io::stdout().lock();
    let result = (|| -> Result<()> {
        let request = parse(args)?;
        let bytes = acknowledgement(&request)?;
        execute(&request)?;
        started = true;
        write_ack(&mut stdout, &bytes)?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Desktop metadata failure: {error:#}");
        if !started {
            let mut bytes = serde_json::to_vec(
                &serde_json::json!({"protocol":1,"operation":operation,"outcome":"unknown","error":FAILURE}),
            )?;
            bytes.push(b'\n');
            if let Err(ack_error) = write_ack(&mut stdout, &bytes) {
                eprintln!(
                    "Desktop metadata failure acknowledgement write/flush failed: {ack_error}"
                );
            }
        }
        bail!(FAILURE);
    }
    Ok(true)
}
