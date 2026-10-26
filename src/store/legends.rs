use super::*;

fn window(span: &str) -> Result<i64> {
    match span {
        "week" => Ok(7 * 86400),
        "month" => Ok(30 * 86400),
        "all" => Ok(i64::MAX),
        _ => anyhow::bail!("timespan must be week, month or all"),
    }
}
fn entry(
    tag: &str,
    character: &Character,
    receipt: &death_awards::DeathFameReceipt,
) -> Result<String> {
    ensure!(
        character.fame >= 0 && character.items.len() == 12 && !character.alive && character.hp == 0,
        "invalid Legends death snapshot"
    );
    Ok(format!(
        "<{tag} accountId=\"{}\" charId=\"{}\"><Name>{}</Name><ObjectType>{}</ObjectType><Tex1>{}</Tex1><Tex2>{}</Tex2><Equipment>{}</Equipment><TotalFame>{}</TotalFame></{tag}>",
        character.account,
        character.id,
        xml_escape(&character.name),
        character.class,
        character.tex1,
        character.tex2,
        character
            .items
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(","),
        receipt.total
    ))
}
impl Store {
    pub async fn legends(&self, span: String, account: i32, character: i32) -> Result<String> {
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        )?;
        self.legends_at(span, account, character, now).await
    }
    async fn legends_at(
        &self,
        span: String,
        account: i32,
        character: i32,
        now: i64,
    ) -> Result<String> {
        let cutoff = now.saturating_sub(window(&span)?).max(0);
        let legacy_base = self.legacy_base;
        self.call(move |db| {
            let tx = db.transaction()?;
            let mut xml = format!("<FameList timespan=\"{span}\">");
            let mut mine_listed = false;
            {
                let mut q = tx.prepare("SELECT snapshot,receipt FROM deaths WHERE died_at>?1 AND died_at<=?2 ORDER BY json_extract(receipt,'$.total') DESC,died_at ASC,character ASC LIMIT 20")?;
                for row in q.query_map(params![cutoff,now],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
                    let (raw, paid) = row?;
                    let receipt=death_awards::decode_receipt(&paid)?;
                    let c = receipt.character(&raw,legacy_base)?;
                    mine_listed |= c.account == account && c.id == character;
                    xml.push_str(&entry("FameListElem",&c,&receipt)?);
                }
            }
            if !mine_listed && account > 0 && character > 0 {
                let mut q = tx.prepare("SELECT snapshot,receipt FROM deaths JOIN characters ON characters.id=deaths.character WHERE characters.account=?1 AND deaths.character=?2 AND died_at>?3 AND died_at<=?4")?;
                for row in q.query_map(params![account,character,cutoff,now],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
                    let (raw, paid) = row?;
                    let receipt=death_awards::decode_receipt(&paid)?;
                    xml.push_str(&entry("MyFameListElem",&receipt.character(&raw,legacy_base)?,&receipt)?);
                }
            }
            xml.push_str("</FameList>");
            tx.commit()?;
            Ok(xml)
        }).await
    }
}
