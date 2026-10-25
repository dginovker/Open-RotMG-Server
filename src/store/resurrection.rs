use super::*;

impl Store {
    pub async fn resurrect(
        &self,
        mut ch: Character,
        slot: usize,
        content: std::sync::Arc<Content>,
    ) -> Result<Character> {
        ensure!(
            !ch.alive && ch.hp == 0 && ch.items.len() == 12 && slot < 4,
            "resurrection requires a fatal snapshot and equipped item"
        );
        let item = content.get(u16::try_from(ch.items[slot])?)?;
        ensure!(item.resurrects, "item does not grant resurrection");
        ensure!(
            ch.revision >= 0 && ch.revision < i32::MAX,
            "invalid resurrection revision"
        );
        let previous_items = ch.items.clone();
        ch.items[slot] = -1;
        let stats = ch.effective(&content)?;
        ch.hp = stats[0];
        ch.mp = stats[1];
        ch.alive = true;
        ensure!(ch.hp > 0, "resurrection has no living health capacity");
        self.call(move |db| {
            let tx = db.transaction()?;
            let (raw, items): (String, String) = tx.query_row(
                "SELECT stats,items FROM characters WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1",
                params![ch.id, ch.account, ch.revision], |r| Ok((r.get(0)?, r.get(1)?)))
                .context("resurrection conflict: dead, foreign or stale character")?;
            let saved: Character = serde_json::from_str(&raw)?;
            ensure!(saved.schema == ch.schema && saved.class == ch.class && saved.base == ch.base
                && saved.level == ch.level && saved.exp == ch.exp && saved.fame == ch.fame
                && saved.pirate_caves == ch.pirate_caves && saved.dungeons == ch.dungeons && saved.abysses == ch.abysses && saved.trenches == ch.trenches && saved.spider_dens == ch.spider_dens && saved.mad_labs == ch.mad_labs && saved.jungles == ch.jungles && saved.manors == ch.manors && saved.beaches == ch.beaches && saved.davy_lockers == ch.davy_lockers && saved.tombs == ch.tombs
                && serde_json::from_str::<Vec<i32>>(&items)? == previous_items,
                "resurrection reservation changed");
            ch.lifetime.follows(&saved.lifetime)?;
            ch.revision += 1;
            let n = tx.execute(
                "UPDATE characters SET stats=?4,items=?5,revision=revision+1 WHERE id=?1 AND account=?2 AND revision=?3 AND alive=1",
                params![ch.id, ch.account, ch.revision - 1, serde_json::to_string(&ch)?, serde_json::to_string(&ch.items)?])?;
            ensure!(n == 1, "resurrection commit lost its reservation");
            tx.commit()?;
            Ok(ch)
        }).await
    }
}
