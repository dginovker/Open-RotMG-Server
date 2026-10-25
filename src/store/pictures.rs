use super::*;
use std::io::Read;

pub const MAX_PNG: usize = 16 * 1024;
pub const MAX_PER_ACCOUNT: usize = 128;
pub const MAX_TOTAL: usize = 1024;

pub fn validate_capacity(max_account: usize, total: usize) -> Result<()> {
    ensure!(
        max_account <= MAX_PER_ACCOUNT,
        "picture account capacity exceeded: {max_account} rows, limit {MAX_PER_ACCOUNT}; delete an owned picture before saving a new name"
    );
    ensure!(
        total <= MAX_TOTAL,
        "picture global capacity exceeded: {total} rows, limit {MAX_TOTAL}; an owner must delete a picture before a new name can be saved"
    );
    Ok(())
}

pub const TABLE: &str = "CREATE TABLE pictures(id INTEGER PRIMARY KEY CHECK(id BETWEEN 1 AND 2147483647),account INTEGER NOT NULL REFERENCES accounts(id),name TEXT NOT NULL,data_type INTEGER NOT NULL CHECK(data_type BETWEEN 1 AND 7),tags TEXT NOT NULL,png BLOB NOT NULL CHECK(typeof(png)='blob' AND length(png)<=16384),UNIQUE(account,name))";
pub const SEQUENCE: &str = "CREATE TABLE picture_sequence(id INTEGER PRIMARY KEY CHECK(id=1),next_id INTEGER NOT NULL CHECK(typeof(next_id)='integer' AND next_id BETWEEN 1 AND 2147483648))";

pub fn metadata(name: &str, data_type: i32, tags: &str) -> Result<()> {
    ensure!(
        (1..=32).contains(&name.len())
            && name.bytes().any(|b| b != b' ')
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b' '),
        "picture name must be 1–32 ASCII letters, digits or spaces"
    );
    ensure!((1..=7).contains(&data_type), "picture dataType must be 1–7");
    tag_filter(tags)?;
    Ok(())
}

pub fn tag_filter(tags: &str) -> Result<Vec<String>> {
    ensure!(
        tags.len() <= 256
            && tags
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b" ,".contains(&b)),
        "picture tags must be at most 256 lowercase ASCII letters, digits, spaces or commas"
    );
    Ok(tags
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

pub fn validate_png(data_type: i32, png: &[u8]) -> Result<()> {
    ensure!(
        png.len() <= MAX_PNG && png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "picture must be a PNG of at most 16 KiB"
    );
    let mut remaining = &png[8..];
    let mut chunks = Vec::new();
    // The selected encoder emits only these three chunks: no compressed metadata or APNG allocations.
    for kind in [b"IHDR", b"IDAT", b"IEND"] {
        ensure!(remaining.len() >= 12, "truncated picture PNG chunk");
        let len = u32::from_be_bytes(remaining[..4].try_into()?) as usize;
        ensure!(
            len <= remaining.len() - 12 && &remaining[4..8] == kind,
            "unsupported picture PNG chunk layout"
        );
        let crc = u32::from_be_bytes(remaining[8 + len..12 + len].try_into()?);
        ensure!(
            crc32fast::hash(&remaining[4..8 + len]) == crc,
            "picture PNG CRC mismatch"
        );
        chunks.push(&remaining[8..8 + len]);
        remaining = &remaining[12 + len..];
    }
    ensure!(
        remaining.is_empty() && chunks[0].len() == 13 && chunks[2].is_empty(),
        "invalid picture PNG header or trailing bytes"
    );
    let header = chunks[0];
    let width = u32::from_be_bytes(header[..4].try_into()?);
    let height = u32::from_be_bytes(header[4..8].try_into()?);
    let simple = [(8, 8), (16, 8), (16, 16)];
    let valid = match data_type {
        1 => {
            simple.contains(&(width, height))
                || [(56, 8), (112, 8), (112, 16)].contains(&(width, height))
        }
        5 => width == height && [4, 5, 9, 10].contains(&width),
        2..=4 | 6..=7 => simple.contains(&(width, height)),
        _ => false,
    };
    ensure!(
        valid && header[8..] == [8, 6, 0, 0, 0],
        "unsupported native picture dimensions or format"
    );
    let expected = (width as usize * 4 + 1) * height as usize;
    let mut decoder = flate2::bufread::ZlibDecoder::new(chunks[1]);
    let mut pixels = Vec::with_capacity(expected + 1);
    decoder
        .by_ref()
        .take((expected + 1) as u64)
        .read_to_end(&mut pixels)?;
    ensure!(
        pixels.len() == expected && decoder.total_in() == chunks[1].len() as u64,
        "picture PNG decompression length mismatch"
    );
    ensure!(
        pixels
            .chunks_exact(width as usize * 4 + 1)
            .all(|row| row[0] <= 4),
        "invalid picture PNG row filter"
    );
    Ok(())
}

pub(super) fn migrate(db: &Connection, version: i32) -> Result<()> {
    for (name, definition) in [("pictures", TABLE), ("picture_sequence", SEQUENCE)] {
        let sql: Option<String> = db
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        if version < 15 {
            ensure!(
                sql.is_none(),
                "unexpected {name} before schema 15; restore reviewed state"
            );
            db.execute_batch(definition)?;
        } else {
            ensure!(
                sql.as_deref() == Some(definition),
                "unsupported canonical {name} schema; restore reviewed state"
            );
        }
    }
    if version < 15 {
        db.execute("INSERT INTO picture_sequence(id,next_id) VALUES(1,1)", [])?;
    }
    let references: Vec<(String, String, String)> = {
        let mut q = db
            .prepare("SELECT \"table\",\"from\",\"to\" FROM pragma_foreign_key_list('pictures')")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    ensure!(
        references == vec![("accounts".into(), "account".into(), "id".into())],
        "invalid picture account foreign key"
    );
    let (max_account, total) = db.query_row(
        "SELECT coalesce(max(n),0),coalesce(sum(n),0) FROM (SELECT count(*) n FROM pictures GROUP BY account)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    validate_capacity(max_account, total)?;
    let oversized: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pictures WHERE typeof(png)<>'blob' OR length(png)>16384 OR length(name)>32 OR length(tags)>256)", [], |r| r.get(0))?;
    ensure!(
        !oversized,
        "oversized or malformed stored picture; restore reviewed state"
    );
    let mut q = db.prepare("SELECT p.id,p.account,p.name,p.data_type,p.tags,p.png,a.id FROM pictures p LEFT JOIN accounts a ON a.id=p.account")?;
    let mut max = 0;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, i32>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i32>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Vec<u8>>(5)?,
            r.get::<_, Option<i32>>(6)?,
        ))
    })? {
        let (id, account, name, kind, tags, png, owner) = row?;
        ensure!(
            id > 0 && account > 0 && owner == Some(account),
            "invalid picture identity or ownership"
        );
        metadata(&name, kind, &tags)?;
        validate_png(kind, &png)
            .with_context(|| format!("invalid stored picture {id}; restore reviewed state"))?;
        max = max.max(id);
    }
    let (singleton, next): (bool, i64) = db.query_row(
        "SELECT count(*)=1 AND min(id)=1,coalesce(max(next_id),0) FROM picture_sequence",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(
        singleton && next > i64::from(max) && next <= i64::from(i32::MAX) + 1,
        "invalid picture identity high-water mark; restore reviewed state"
    );
    Ok(())
}

pub struct PictureQuery {
    pub my_guid: String,
    pub guid: Option<String>,
    pub data_type: Option<i32>,
    pub tags: String,
    pub offset: i32,
    pub num: i32,
}

impl Store {
    pub async fn save_picture(
        &self,
        account: i32,
        name: String,
        data_type: i32,
        tags: String,
        png: Vec<u8>,
        overwrite: bool,
    ) -> Result<i32> {
        metadata(&name, data_type, &tags)?;
        validate_png(data_type, &png)?;
        self.call(move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let existing: Option<i32> = tx.query_row("SELECT id FROM pictures WHERE account=?1 AND name=?2", params![account, name], |r| r.get(0)).optional()?;
            let id = if let Some(id) = existing {
                ensure!(overwrite, "picture name already exists; overwrite permission required");
                tx.execute("UPDATE pictures SET data_type=?2,tags=?3,png=?4 WHERE id=?1 AND account=?5", params![id,data_type,tags,png,account])?;
                id
            } else {
                let (owned, total): (usize, usize) = tx.query_row(
                    "SELECT (SELECT count(*) FROM pictures WHERE account=?1),count(*) FROM pictures",
                    [account], |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                validate_capacity(owned + 1, total + 1)?;
                let next: i64 = tx.query_row("SELECT next_id FROM picture_sequence WHERE id=1", [], |r| r.get(0))?;
                let id = i32::try_from(next).context("picture IDs exhausted")?;
                ensure!(id > 0, "invalid picture sequence");
                tx.execute("INSERT INTO pictures(id,account,name,data_type,tags,png) VALUES(?1,?2,?3,?4,?5,?6)", params![id,account,name,data_type,tags,png])?;
                tx.execute("UPDATE picture_sequence SET next_id=next_id+1 WHERE id=1", [])?;
                id
            };
            tx.commit()?;
            tracing::info!(account, picture=id, "picture save committed");
            Ok(id)
        }).await
    }

    pub async fn picture(&self, id: i32) -> Result<Vec<u8>> {
        ensure!(id > 0, "invalid picture id");
        self.call(move |db| {
            let (kind, png): (i32, Vec<u8>) = db
                .query_row(
                    "SELECT data_type,png FROM pictures WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .context("picture not found")?;
            validate_png(kind, &png).with_context(|| format!("invalid stored picture {id}"))?;
            Ok(png)
        })
        .await
    }

    pub async fn picture_list(&self, query: PictureQuery) -> Result<String> {
        ensure!(
            (0..=i32::MAX).contains(&query.offset) && (1..=24).contains(&query.num),
            "picture offset must be nonnegative and num must be 1–24"
        );
        ensure!(
            query.data_type.is_none_or(|kind| (1..=7).contains(&kind)),
            "picture dataType must be 1–7"
        );
        ensure!(
            query.my_guid.len() <= 245 && query.guid.as_ref().is_none_or(|g| g.len() <= 245),
            "picture identity filter exceeds 245 bytes"
        );
        let tags = serde_json::to_string(&tag_filter(&query.tags)?)?;
        self.call(move |db| {
            let mut xml = format!("<Pictures offset=\"{}\">", query.offset);
            if query.guid.as_ref().is_some_and(|g| g.eq_ignore_ascii_case("administrator@wildshadow.com")) {
                return Ok(format!("{xml}</Pictures>"));
            }
            let identity = |guid: &str| -> Result<Option<i32>> {
                Ok(db.query_row("SELECT a.id FROM accounts a LEFT JOIN credentials c ON c.account=a.id WHERE (c.account IS NOT NULL AND c.email=?1 COLLATE NOCASE) OR (c.account IS NULL AND a.guid=?1)", [guid], |r| r.get(0)).optional()?)
            };
            let mine = identity(&query.my_guid)?;
            let owner = query.guid.as_deref().map(identity).transpose()?.flatten();
            if query.guid.is_some() && owner.is_none() { return Ok(format!("{xml}</Pictures>")); }
            let mut q = db.prepare("SELECT p.id,p.account,p.name,p.data_type,p.tags FROM pictures p WHERE (?1 IS NULL OR p.account=?1) AND (?2 IS NULL OR p.data_type=?2) AND NOT EXISTS (SELECT 1 FROM json_each(?3) wanted WHERE NOT EXISTS (SELECT 1 FROM json_each('[\"'||replace(p.tags,',','\",\"')||'\"]') actual WHERE trim(actual.value)=wanted.value)) ORDER BY p.id DESC LIMIT ?4 OFFSET ?5")?;
            for row in q.query_map(params![owner,query.data_type,tags,query.num,query.offset], |r| Ok((r.get::<_,i32>(0)?,r.get::<_,i32>(1)?,r.get::<_,String>(2)?,r.get::<_,i32>(3)?,r.get::<_,String>(4)?)))? {
                let (id,account,name,kind,tags) = row?;
                metadata(&name,kind,&tags)?;
                xml.push_str(&format!("<Pic id=\"{id}\"><PicName>{}</PicName><DataType>{kind}</DataType><Tags>{}</Tags>{}</Pic>", xml_escape(&name),xml_escape(&tags),if mine == Some(account) { "<Mine/>" } else { "" }));
            }
            xml.push_str("</Pictures>");
            Ok(xml)
        }).await
    }

    pub async fn delete_picture(&self, account: i32, id: i32) -> Result<()> {
        ensure!(id > 0, "invalid picture id");
        self.call(move |db| {
            ensure!(
                db.execute(
                    "DELETE FROM pictures WHERE id=?1 AND account=?2",
                    params![id, account]
                )? == 1,
                "picture not found or not owned by account"
            );
            tracing::info!(account, picture = id, "picture deletion committed");
            Ok(())
        })
        .await
    }
}
