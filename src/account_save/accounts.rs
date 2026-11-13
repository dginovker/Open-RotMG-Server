use super::*;
use crate::store::local_accounts::{
    LocalAccountEdit, edit_local_account, local_accounts, password_key, validate_local_edit,
};
use std::collections::BTreeMap;

fn passwords(state: &Path) -> Result<BTreeMap<String, String>> {
    let path = state.join("local-passwords.json");
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        info => {
            ensure!(
                info?.is_file(),
                "local password file must be a regular file"
            );
            let mut bytes = Vec::new();
            File::open(path)?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 1024 * 1024, "local password file too large");
            Ok(serde_json::from_slice(&bytes)?)
        }
    }
}
fn remember(state: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    let destination = state.join("local-passwords.json");
    let temporary = state.join(format!(
        "local-passwords-{:016x}.tmp",
        rand::random::<u64>()
    ));
    let mut file = private_create(&temporary)?;
    let bytes = serde_json::to_vec(values)?;
    ensure!(bytes.len() <= 1024 * 1024, "local password file too large");
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(windows)]
    crate::windows_publication::publish(&temporary, &destination, true)?;
    #[cfg(not(windows))]
    {
        std::fs::rename(&temporary, &destination)?;
        sync_parent(&destination)?;
    }
    Ok(())
}
pub(super) fn remember_import(state: &Path, account: i32, password: String) -> Result<()> {
    let db = readonly(&state.join("game.db"))?;
    let verifier: String = db.query_row(
        "SELECT password_hash FROM credentials WHERE account=?1",
        [account],
        |r| r.get(0),
    )?;
    let mut saved = passwords(state)?;
    saved.insert(password_key(&verifier), password);
    remember(state, &saved)
}
pub(super) fn run(
    state: &Path,
    data: &Path,
    backup: Option<&Path>,
    phase: &mut Phase,
) -> Result<()> {
    phase.stage("Account management requires a stopped game and a valid schema23 local save.");
    let _lock = StateLock::acquire(state)?;
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
    let mut saved = passwords(state)?;
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
        // Verification against the committed hash hides a staged password if commit fails or a different backup is restored.
        let verifier: String = tx.query_row(
            "SELECT password_hash FROM credentials WHERE account=?1",
            [edit.account],
            |r| r.get(0),
        )?;
        if let Some(password) = edit.password {
            saved.insert(password_key(&verifier), password);
            remember(state, &saved)?;
        }
        phase.mutating("Account edit completion is unknown. Keep the save and backup; do not repeat the edit automatically.");
        tx.commit()?;
    }
    let accounts = local_accounts(&db, &saved)?;
    phase.emit(&serde_json::json!({"accounts":accounts}))
}
