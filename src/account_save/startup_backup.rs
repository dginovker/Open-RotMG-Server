use super::*;
use std::io::ErrorKind;

#[derive(serde::Serialize)]
struct Acknowledgement<'a> {
    protocol: u8,
    operation: &'static str,
    process: u32,
    backup: &'a Path,
    sha256: Option<String>,
    source_schema: Option<i32>,
    target_schema: i32,
    outcome: &'static str,
}
fn require_new(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("inspect startup destination {}", path.display()))
        }
        Ok(_) => anyhow::bail!("startup destination already exists: {}", path.display()),
    }
}
#[cfg(unix)]
fn same_directory(a: &Path, b: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    // Directory identity also covers case-insensitive Unix volumes and mount aliases.
    let a = std::fs::metadata(a)?;
    let b = std::fs::metadata(b)?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}
#[cfg(not(any(unix, windows)))]
fn same_directory(_: &Path, _: &Path) -> Result<bool> {
    anyhow::bail!("startup backup boundary is unsupported on this platform")
}
#[cfg(windows)]
fn same_directory(a: &Path, b: &Path) -> Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CompareStringOrdinal(
            a: *const u16,
            a_len: i32,
            b: *const u16,
            b_len: i32,
            ignore_case: i32,
        ) -> i32;
    }
    let a: Vec<u16> = a.as_os_str().encode_wide().collect();
    let b: Vec<u16> = b.as_os_str().encode_wide().collect();
    // Windows canonical paths can preserve spelling; ordinal comparison also rejects case aliases.
    let result = unsafe {
        CompareStringOrdinal(
            a.as_ptr(),
            a.len().try_into()?,
            b.as_ptr(),
            b.len().try_into()?,
            1,
        )
    };
    ensure!(
        result != 0,
        "cannot compare resolved startup backup boundary"
    );
    Ok(result == 2)
}
#[cfg(target_os = "linux")]
fn mount_id(metadata: &libc::statx) -> Result<u64> {
    ensure!(
        metadata.stx_mask & libc::STATX_MNT_ID != 0,
        "STATX_MNT_ID unavailable; startup backup containment requires Linux 5.8+ mount identity"
    );
    Ok(metadata.stx_mnt_id)
}
#[cfg(target_os = "linux")]
fn query_mount_id(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut metadata: libc::statx = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::statx(
            libc::AT_FDCWD,
            path.as_ptr(),
            0,
            libc::STATX_MNT_ID,
            &mut metadata,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error())
            .context("query startup backup mount identity with statx (Linux 5.8+ required)");
    }
    mount_id(&metadata)
}
#[cfg(target_os = "linux")]
fn require_same_mount(state: u64, parent: u64) -> Result<()> {
    // Bind roots below state retain dev/inode but acquire a different mount ID.
    ensure!(
        state == parent,
        "startup backup crosses Linux mounts ({state} != {parent}); outside-state containment is ambiguous"
    );
    Ok(())
}
fn outside_state(state: &Path, backup: &Path) -> Result<()> {
    let state = state
        .canonicalize()
        .context("resolve startup state directory")?;
    let parent = backup
        .parent()
        .context("startup backup needs a parent")?
        .canonicalize()
        .context("resolve existing startup backup parent")?;
    ensure!(parent.is_dir(), "startup backup parent is not a directory");
    #[cfg(target_os = "linux")]
    require_same_mount(query_mount_id(&state)?, query_mount_id(&parent)?)?;
    for ancestor in parent.ancestors() {
        ensure!(
            !same_directory(ancestor, &state)?,
            "startup backup parent must be outside the state directory"
        );
    }
    Ok(())
}
fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 8192];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn publish_ack(path: &Path, bytes: &[u8]) -> Result<()> {
    let staging = path.with_extension(format!("partial-{:016x}", rand::random::<u64>()));
    let mut file = private_create(&staging)?;
    let mut consumed = false;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        sync_staging(&file, "acknowledgement")?;
        drop(file);
        consumed = publish_distinct(&staging, path)?;
        Ok(())
    })();
    if consumed {
        result
    } else {
        result.and(std::fs::remove_file(&staging).context("remove startup acknowledgement staging"))
    }
}
pub(crate) fn pre_migration_backup(
    state: &Path,
    backup: &Path,
    content: &Content,
    _lock: &StateLock,
) -> Result<()> {
    ensure!(
        backup.is_absolute() && backup.file_name().is_some(),
        "startup backup requires an absolute file path"
    );
    outside_state(state, backup)?;
    let ack = std::path::PathBuf::from(format!(
        "{}.ack.json",
        backup
            .to_str()
            .context("startup backup path must be UTF-8")?
    ));
    require_new(backup)?;
    require_new(&ack)?;
    let database = state.join("game.db");
    let (sha256, source_schema, outcome) = match std::fs::symlink_metadata(&database) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "startup game database must be a regular file, not a symlink"
            );
            snapshot(&database, backup, content).context("required startup backup failed")?;
            let db = readonly(backup)?;
            verify(&db, content)?;
            let schema: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            (Some(hash_file(backup)?), Some(schema), "verified-backup")
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            for name in ["game.db-wal", "game.db-shm", "game.db-journal"] {
                match std::fs::symlink_metadata(state.join(name)) {
                    Ok(meta) => ensure!(
                        meta.is_file() && !meta.file_type().is_symlink() && meta.len() == 0,
                        "orphan SQLite sidecar prevents no-game-db acknowledgement: {name}"
                    ),
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => return Err(error).context("inspect orphan SQLite sidecar"),
                }
            }
            (None, None, "no-game-db")
        }
        Err(error) => return Err(error).context("inspect startup game database"),
    };
    let bytes = serde_json::to_vec(&Acknowledgement {
        protocol: 1,
        operation: "pre-migration",
        process: std::process::id(),
        backup,
        sha256,
        source_schema,
        target_schema: 23,
        outcome,
    })?;
    ensure!(
        bytes.len() <= 16 * 1024,
        "startup acknowledgement exceeds 16 KiB"
    );
    #[cfg(test)]
    if std::env::var_os("STARTUP_BACKUP_FAIL_ACK").is_some() {
        std::fs::create_dir(&ack)?;
    }
    publish_ack(&ack, &bytes).context("required startup acknowledgement failed before migration")
}

#[cfg(test)]
mod tests;
