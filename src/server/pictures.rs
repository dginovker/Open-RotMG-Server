use super::*;
use axum::extract::Multipart;

pub(super) async fn save(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    match admit(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => return limited(),
        Err(e) => return error(e),
    }
    let result: Result<i32> = async {
        let mut form = HashMap::new();
        let mut png = None;
        while let Some(field) = multipart.next_field().await? {
            let name = field
                .name()
                .context("picture field name required")?
                .to_owned();
            ensure!(
                [
                    "guid",
                    "password",
                    "secret",
                    "name",
                    "dataType",
                    "tags",
                    "overwrite",
                    "data",
                    "Filename",
                    "Upload"
                ]
                .contains(&name.as_str()),
                "unsupported picture field"
            );
            if name == "data" {
                ensure!(png.is_none(), "duplicate picture data");
                let data = field.bytes().await?;
                ensure!(
                    data.len() <= crate::store::pictures::MAX_PNG,
                    "picture PNG exceeds 16 KiB"
                );
                png = Some(data.to_vec());
            } else {
                ensure!(!form.contains_key(&name), "duplicate picture metadata");
                let value = field.text().await?;
                ensure!(value.len() <= 256, "picture metadata exceeds 256 bytes");
                match name.as_str() {
                    "Filename" => ensure!(value == "Foo.png", "invalid picture Filename"),
                    "Upload" => ensure!(value == "Submit Query", "invalid picture Upload"),
                    _ => {}
                }
                form.insert(name, value);
            }
        }
        let _authentication = a.authentication.read().await;
        let account = authenticate(&a, &form).await?;
        let overwrite = match form.get("overwrite").map(String::as_str) {
            Some("on") => true,
            None => false,
            _ => bail!("invalid picture overwrite permission"),
        };
        a.store
            .save_picture(
                account,
                required(&form, "name")?.into(),
                required(&form, "dataType")?
                    .parse()
                    .context("invalid picture dataType")?,
                required(&form, "tags")?.into(),
                png.context("picture data required")?,
                overwrite,
            )
            .await
    }
    .await;
    match result {
        Ok(id) => xml(format!("<Success><Id>{id}</Id></Success>")),
        Err(e) => error(e),
    }
}

fn required<'a>(form: &'a HashMap<String, String>, name: &str) -> Result<&'a str> {
    form.get(name)
        .map(String::as_str)
        .with_context(|| format!("picture {name} required"))
}

async fn authenticate(a: &App, form: &HashMap<String, String>) -> Result<i32> {
    ensure!(
        form.get("secret").is_none_or(|s| s.is_empty()),
        "external platform authentication is not implemented"
    );
    a.store
        .authenticate(
            required(form, "guid")?.into(),
            required(form, "password")?.into(),
        )
        .await
}

fn xml(body: String) -> Response {
    (
        [
            ("content-type", "application/xml"),
            ("cache-control", "no-store"),
        ],
        body,
    )
        .into_response()
}

pub(super) async fn get_picture(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    match admit_read(&a, &headers, peer) {
        Ok(true) => {}
        Ok(false) => return limited_read(),
        Err(e) => return error(e),
    }
    let result: Result<Vec<u8>> = async {
        let id: i32 = required(&form, "id")?
            .parse()
            .context("invalid picture id")?;
        a.store.picture(id).await
    }
    .await;
    match result {
        Ok(png) => (
            [
                ("content-type", "image/png"),
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
            ],
            png,
        )
            .into_response(),
        Err(e) => error(e),
    }
}

fn limited_read() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("content-type", "application/xml"), ("retry-after", "60")],
        "<Error>Too many picture reads; retry in one minute.</Error>",
    )
        .into_response()
}

fn rate_allow_read(
    requests: &mut HashMap<IpAddr, (std::time::Instant, u32)>,
    ip: IpAddr,
    now: std::time::Instant,
) -> bool {
    requests.retain(|_, (start, _)| now.duration_since(*start).as_secs() < 60);
    if requests.len() >= 4096 && !requests.contains_key(&ip) {
        return false;
    }
    let (_, count) = requests.entry(ip).or_insert((now, 0));
    *count = count.saturating_add(1);
    *count <= 120
}

fn admit_read(a: &App, headers: &HeaderMap, peer: SocketAddr) -> Result<bool> {
    let ip = request_ip(a, headers, peer)?;
    Ok(rate_allow_read(
        &mut a
            .picture_reads
            .lock()
            .expect("picture request registry poisoned"),
        ip,
        std::time::Instant::now(),
    ))
}

pub(super) async fn request(
    State(a): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::extract::Path(operation): axum::extract::Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let admission = if operation == "list" {
        admit_read(&a, &headers, peer)
    } else {
        admit(&a, &headers, peer)
    };
    match admission {
        Ok(true) => {}
        Ok(false) => {
            return if operation == "list" {
                limited_read()
            } else {
                limited()
            };
        }
        Err(e) => return error(e),
    }
    let result: Result<String> = async {
        match operation.as_str() {
            "list" => {
                let number = |name: &str, default| -> Result<i32> {
                    form.get(name)
                        .map(|s| s.parse().with_context(|| format!("invalid picture {name}")))
                        .unwrap_or(Ok(default))
                };
                a.store
                    .picture_list(crate::store::pictures::PictureQuery {
                        my_guid: required(&form, "myGUID")?.into(),
                        guid: form.get("guid").cloned(),
                        data_type: form
                            .get("dataType")
                            .map(|s| s.parse())
                            .transpose()
                            .context("invalid picture dataType")?,
                        tags: form.get("tags").cloned().unwrap_or_default(),
                        offset: number("offset", 0)?,
                        num: number("num", 24)?,
                    })
                    .await
            }
            "delete" => {
                let _authentication = a.authentication.read().await;
                let account = authenticate(&a, &form).await?;
                let id = required(&form, "id")?
                    .parse()
                    .context("invalid picture id")?;
                a.store.delete_picture(account, id).await?;
                Ok("<Success/>".into())
            }
            _ => bail!("unsupported picture operation"),
        }
    }
    .await;
    match result {
        Ok(body) => xml(body),
        Err(e) => error(e),
    }
}
