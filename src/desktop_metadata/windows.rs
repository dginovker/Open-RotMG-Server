use super::*;
use anyhow::Context;
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
};

fn checked(metadata: Metadata) -> Result<Metadata> {
    ensure!(
        metadata.file_attributes() & 0x400 == 0,
        "reparse points are forbidden in metadata paths"
    );
    Ok(metadata)
}
fn root(request: &Request) -> Result<PathBuf> {
    ensure!(
        checked(fs::symlink_metadata(&request.root).context("inspect existing metadata root")?)?
            .is_dir(),
        "metadata root must be an existing directory"
    );
    Ok(Path::new(&request.root)
        .canonicalize()
        .context("resolve existing metadata root")?)
}
fn resolve(root: &Path, relative: &str, missing_leaf: bool) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    let parts: Vec<_> = relative.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        path.push(part);
        let last = i + 1 == parts.len();
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                let metadata = checked(metadata)?;
                ensure!(
                    last || metadata.is_dir(),
                    "metadata ancestor is not a directory"
                );
                ensure!(
                    path.canonicalize()?.starts_with(root),
                    "metadata path escapes resolved root"
                );
            }
            Err(error) if last && missing_leaf && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect metadata path"),
        }
    }
    Ok(path)
}
fn require_new(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("inspect new metadata destination"),
        Ok(_) => bail!("metadata destination already exists"),
    }
}
fn flush_close(file: File, flush: fn(&File) -> io::Result<()>) -> Result<()> {
    let flushed = flush(&file);
    let closed = crate::windows_publication::close(file);
    match (flushed, closed) {
        (Err(flush), Err(close)) => bail!(
            "control-file flush failed (OS {:?}); close failed (OS {:?})",
            flush.raw_os_error(),
            close.raw_os_error()
        ),
        (Err(error), _) => Err(error).context("flush control file"),
        (_, Err(error)) => Err(error).context("close control file"),
        _ => Ok(()),
    }
}
fn complete_stage(source: &Path) -> Result<()> {
    let mut runtime = false;
    let mut generation = false;
    let mut manifest = false;
    let mut signature = false;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("non-UTF8 generation entry")?;
        let meta = checked(fs::symlink_metadata(entry.path())?)?;
        match name {
            "runtime" => {
                ensure!(meta.is_dir(), "generation runtime is not a directory");
                runtime = true;
            }
            "generation.json" => {
                ensure!(meta.is_file(), "generation descriptor is not a file");
                generation = true;
            }
            "manifest.json" => {
                ensure!(meta.is_file(), "generation manifest is not a file");
                manifest = true;
            }
            "manifest.sig" => {
                ensure!(meta.is_file(), "generation signature is not a file");
                signature = true;
            }
            _ => bail!("unexpected entry in complete generation stage"),
        }
    }
    ensure!(
        runtime && generation && manifest == signature,
        "incomplete generation stage"
    );
    Ok(())
}
pub(super) fn execute(request: &Request, flush: fn(&File) -> io::Result<()>) -> Result<()> {
    let root = root(request)?;
    let destination = resolve(&root, &request.destination, true)?;
    if request.operation == Operation::CreateContainer {
        require_new(&destination)?;
        let parent = destination
            .parent()
            .context("missing metadata container parent")?;
        let temporary = parent.join(format!(
            ".desktop-metadata-container-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir(&temporary).context("create private metadata container staging")?;
        let result = crate::windows_publication::publish(&temporary, &destination, false)
            .context("publish distinct control container");
        if let Err(primary) = result {
            return match fs::remove_dir(&temporary) {
                Ok(()) => Err(primary),
                Err(cleanup) => Err(primary).context(format!(
                    "container staging cleanup failed (OS {:?})",
                    cleanup.raw_os_error()
                )),
            };
        }
        return Ok(());
    }
    let source = resolve(
        &root,
        request
            .source
            .as_deref()
            .context("missing metadata source")?,
        false,
    )?;
    ensure!(
        source != destination,
        "metadata source and destination must be distinct"
    );
    if request.operation == Operation::PublishReplace {
        match fs::symlink_metadata(&destination) {
            Ok(metadata) => ensure!(
                checked(metadata)?.is_file(),
                "replacement is restricted to regular control files"
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect replacement control file"),
        }
    } else {
        require_new(&destination)?;
    }
    if request.kind == "file" {
        ensure!(
            checked(fs::symlink_metadata(&source)?)?.is_file(),
            "control source must be a regular file"
        );
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0x00200000)
            .open(&source)
            .context("open control file without following leaf reparse points")?;
        let metadata = checked(file.metadata()?)?;
        ensure!(
            metadata.is_file(),
            "opened control source is not a regular file"
        );
        flush_close(file, flush)?;
    } else {
        ensure!(
            checked(fs::symlink_metadata(&source)?)?.is_dir(),
            "generation source is not a directory"
        );
        complete_stage(&source)?;
        // Caller verified and closed/fsynced every staged file; this move is not a recursive tree flush.
    }
    crate::windows_publication::publish(
        &source,
        &destination,
        request.operation == Operation::PublishReplace,
    )
    .context("publish desktop control namespace")
}
