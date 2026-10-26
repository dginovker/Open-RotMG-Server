use super::*;
use std::collections::BTreeSet;

pub type AccountLists = [BTreeSet<i32>; 2];

fn lists(db: &Connection, account: i32) -> Result<AccountLists> {
    let mut lists = AccountLists::default();
    let mut query = db.prepare("SELECT list_id,target FROM account_lists WHERE account=?1")?;
    for row in query.query_map([account], |r| Ok((r.get::<_, usize>(0)?, r.get(1)?)))? {
        let (list, target) = row?;
        lists
            .get_mut(list)
            .context("invalid account list")?
            .insert(target);
    }
    ensure!(
        lists.iter().all(|l| l.len() <= i16::MAX as usize),
        "account list too long"
    );
    Ok(lists)
}

impl Store {
    pub async fn account_named(&self, name: String) -> Result<i32> {
        self.call(move |db| {
            accounts::named_account(db, &name)?
                .ok_or_else(|| anyhow::anyhow!(Rejected("Player not found.")))
        })
        .await
    }

    pub async fn account_lists(&self, account: i32) -> Result<AccountLists> {
        self.call(move |db| lists(db, account)).await
    }

    pub async fn edit_account_list(
        &self,
        account: i32,
        list: i32,
        add: bool,
        target: i32,
    ) -> Result<AccountLists> {
        ensure!(
            (0..=1).contains(&list) && account != target,
            Rejected("invalid account list edit")
        );
        self.call(move |db| {
            let tx = db.transaction()?;
            if add {
                tx.execute(
                    "INSERT OR IGNORE INTO account_lists(account,list_id,target) VALUES(?1,?2,?3)",
                    params![account, list, target],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM account_lists WHERE account=?1 AND list_id=?2 AND target=?3",
                    params![account, list, target],
                )?;
            }
            let lists = lists(&tx, account)?;
            tx.commit()?;
            Ok(lists)
        })
        .await
    }
}
