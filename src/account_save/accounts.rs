use super::*;
use crate::store::local_accounts::{
    LocalAccountEdit, edit_local_account, local_accounts, validate_local_edit,
};

pub(crate) fn forget_passwords(state: &Path) -> Result<()> {
    for entry in std::fs::read_dir(state)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let staged = name
            .strip_prefix("local-passwords-")
            .and_then(|n| n.strip_suffix(".tmp"))
            .is_some_and(|n| n.len() == 16 && n.bytes().all(|b| b.is_ascii_hexdigit()));
        if name == "local-passwords.json" || staged {
            ensure!(
                entry.file_type()?.is_file(),
                "legacy password cache must be a regular file"
            );
            std::fs::remove_file(entry.path()).context("remove legacy readable password cache")?;
            #[cfg(unix)]
            File::open(state)?.sync_all()?;
        }
    }
    Ok(())
}

pub(super) fn run(
    state: &Path,
    data: &Path,
    backup: Option<&Path>,
    phase: &mut Phase,
) -> Result<()> {
    phase.stage("Account management requires a stopped game and a valid schema23 local save.");
    let _lock = StateLock::acquire(state)?;
    forget_passwords(state)?;
    let content = Content::load(data)?;
    let mut db = Connection::open_with_flags(
        state.join("game.db"),
        if backup.is_some() {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        },
    )?;
    verify(&db, &content)?;
    let schema: i32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        schema == 23,
        "account management requires schema23 without implicit migration"
    );
    if let Some(backup) = backup {
        phase.stage("Account edit input is invalid. Use an email-shaped username and a password of5–128 bytes.");
        ensure!(
            !std::io::stdin().is_terminal(),
            "account edit requires private stdin"
        );
        let mut bytes = Vec::new();
        std::io::stdin().take(4097).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 4096, "account edit input too large");
        let edit: LocalAccountEdit = serde_json::from_slice(&bytes)?;
        validate_local_edit(&edit)?;
        phase.stage(
            "Account changed or username is taken. Refresh the account list before trying again.",
        );
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        edit_local_account(&tx, &edit)?;
        phase.stage("Account pre-backup failed; local credentials were not changed. Inspect the backup folder before retrying.");
        snapshot(&state.join("game.db"), backup, &content)?;
        phase.mutating("Account edit completion is unknown. Keep the save and backup; do not repeat the edit automatically.");
        tx.commit()?;
    }
    let accounts = local_accounts(&db)?;
    phase.emit(&serde_json::json!({"accounts":accounts}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_readable_password_caches_are_removed_without_touching_saves_or_locks() {
        let path =
            std::env::temp_dir().join(format!("password-purge-{:016x}", rand::random::<u64>()));
        std::fs::create_dir(&path).unwrap();
        for name in [
            "local-passwords.json",
            "local-passwords-0123456789abcdef.tmp",
            "game.db",
            "state.lock",
            "local-passwords-not-a-cache.tmp",
        ] {
            std::fs::write(path.join(name), "private-fixture").unwrap();
        }
        forget_passwords(&path).unwrap();
        assert!(!path.join("local-passwords.json").exists());
        assert!(!path.join("local-passwords-0123456789abcdef.tmp").exists());
        for name in ["game.db", "state.lock", "local-passwords-not-a-cache.tmp"] {
            assert!(path.join(name).is_file());
        }
        std::fs::remove_dir_all(path).unwrap();
    }
}
