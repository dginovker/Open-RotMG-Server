use super::*;
#[cfg(not(windows))]
use std::process::Command;
use std::{fs, io, path::PathBuf};

const UUID: &str = "01234567-89ab-cdef-0123-456789abcdef";
fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!("metadata-policy-{}", rand::random::<u64>()));
    fs::create_dir(&path).unwrap();
    path
}
fn args(root: &Path, operation: &str, source: Option<&str>, destination: &str) -> Vec<String> {
    let mut args = vec![
        "--desktop-metadata".into(),
        operation.into(),
        root.to_str().unwrap().into(),
    ];
    if let Some(source) = source {
        args.push(source.into());
    }
    args.push(destination.into());
    args
}
#[test]
fn desktop_metadata_exact_roles_and_strict_wire() {
    let root = root();
    let hash = "a".repeat(64);
    for destination in [
        "runtime-profile.json".into(),
        "runtime-selection.json".into(),
        "runtime-pending.json".into(),
        "runtime-recovery.json".into(),
        "runtime-activity.json".into(),
        "runtime-check-refusal.json".into(),
        "shutdown-failed.json".into(),
        format!("runtime-recovery-history/{hash}.json"),
        format!("runtime-manifests/1-{hash}.json"),
        format!("runtime-manifests/1-{hash}.sig"),
        format!("diagnostics/failure-history/{hash}.json"),
        "runtime-staging/generation-Ab123/generation.json".into(),
        "runtime-staging/generation-Ab123/manifest.sig".into(),
    ] {
        let source = format!("{destination}.partial-{UUID}");
        for operation in ["publish-replace", "publish-distinct"] {
            let request = parse(&args(&root, operation, Some(&source), &destination)).unwrap();
            let bytes = acknowledgement(&request).unwrap();
            assert!(bytes.len() <= MAX_ACK && bytes.ends_with(b"\n"));
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value.as_object().unwrap().len(), 8);
            assert_eq!(value["root"], root.to_str().unwrap());
            assert_eq!(value["process"], std::process::id());
            assert_eq!(value["operation"], operation);
            assert_eq!(value["source"], source);
            assert_eq!(value["destination"], destination);
            assert_eq!(value["kind"], "file");
            assert_eq!(value["published"], true);
        }
    }
    for (source, suffix) in [
        ("runtime-pending.json", "pending"),
        ("runtime-activity.json", "activity"),
        ("runtime-check-refusal.json", "refusal"),
    ] {
        let destination = format!("runtime-recovery-history/{hash}.{suffix}.json");
        assert!(parse(&args(&root, "move-distinct", Some(source), &destination)).is_ok());
        assert!(
            parse(&args(
                &root,
                "move-distinct",
                Some("runtime-selection.json"),
                &destination
            ))
            .is_err()
        );
    }
    assert!(
        parse(&args(
            &root,
            "move-distinct",
            Some("runtime-staging/generation-A123"),
            &format!("runtimes/source-{hash}")
        ))
        .is_ok()
    );
    assert!(
        parse(&args(
            &root,
            "move-distinct",
            Some("runtime-staging/generation-A123"),
            &format!("runtimes/release-1-{hash}")
        ))
        .is_ok()
    );
    for container in [
        "runtimes",
        "runtime-staging",
        "runtime-staging/generation-A1",
        "runtime-manifests",
        "runtime-recovery-history",
        "diagnostics",
        "diagnostics/failure-history",
    ] {
        let request = parse(&args(&root, "create-container", None, container)).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&acknowledgement(&request).unwrap()).unwrap();
        assert!(value["source"].is_null());
        assert_eq!(value["kind"], "directory");
    }
    for invalid in [
        "state/game.db",
        "backups/snapshot.db",
        "game.db",
        "state.lock",
        "server.pem",
        "runtime-staging/generation-A/input.sqlite",
        "runtimes/source-a/generation.json",
        "runtime-recovery-history/foreign.json",
        "runtime-manifests/01-foreign.json",
        "runtime-staging/generation-A/CON",
        "../runtime-profile.json",
        "/runtime-profile.json",
        "runtime-profile.json:stream",
        "runtime-profile.json.",
        "runtime-profile.json/..",
        "runtime-staging\\generation-A\\manifest.json",
    ] {
        assert!(
            parse(&args(
                &root,
                "publish-replace",
                Some(&format!("{invalid}.partial-{UUID}")),
                invalid
            ))
            .is_err(),
            "{invalid}"
        );
    }
    assert!(
        parse(&args(
            &root,
            "publish-distinct",
            Some("runtime-profile.json"),
            "runtime-profile.json"
        ))
        .is_err()
    );
    assert!(
        parse(&args(
            &root,
            "publish-distinct",
            Some("runtime-profile.json.partial-not-uuid"),
            "runtime-profile.json"
        ))
        .is_err()
    );
    assert!(parse(&args(&root, "create-container", None, "backups")).is_err());
    assert!(
        parse(&args(
            Path::new("relative"),
            "create-container",
            None,
            "runtimes"
        ))
        .is_err()
    );
    let mut extra = args(&root, "create-container", None, "runtimes");
    extra.push("extra".into());
    assert!(parse(&extra).is_err());
    fs::remove_dir_all(root).unwrap();
}
struct Broken {
    bytes: Vec<u8>,
    limit: usize,
    flush_fails: bool,
}
impl Write for Broken {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let count = data.len().min(self.limit.saturating_sub(self.bytes.len()));
        if count == 0 {
            return Err(io::Error::other("injected write failure"));
        }
        self.bytes.extend_from_slice(&data[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.flush_fails {
            Err(io::Error::other("injected flush failure"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn desktop_metadata_checked_acknowledgement_write_and_flush() {
    for (limit, flush_fails) in [(7, false), (usize::MAX, true)] {
        let mut writer = Broken {
            bytes: vec![],
            limit,
            flush_fails,
        };
        assert!(write_ack(&mut writer, b"{\"protocol\":1}\n").is_err());
        assert!(writer.bytes.len() <= 15);
    }
}

#[cfg(not(windows))]
#[test]
fn desktop_metadata_non_windows_cli_refuses_before_service_lock_or_save_access() {
    let root = root();
    let source = format!("runtime-activity.json.partial-{UUID}");
    fs::write(root.join(&source), b"PRIVATE_METADATA_PAYLOAD_CANARY").unwrap();
    fs::write(root.join("game.db"), b"GAME_PROGRESS_CANARY").unwrap();
    for (operation, source, destination) in [
        (
            "publish-replace",
            Some(source.as_str()),
            "runtime-activity.json",
        ),
        ("create-container", None, "runtimes"),
        ("unknown", None, "runtime-profile.json"),
    ] {
        let arguments = args(&root, operation, source, destination);
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "desktop_metadata::tests::desktop_metadata_cli_child",
                "--nocapture",
            ])
            .env(
                "METADATA_TEST_ARGS",
                serde_json::to_string(&arguments).unwrap(),
            )
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let line = stdout.lines().find(|line| line.starts_with('{')).unwrap();
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(value["protocol"], 1);
        assert_eq!(value["outcome"], "unknown");
        assert_eq!(
            value["operation"],
            if operation == "unknown" {
                "invalid"
            } else {
                operation
            }
        );
        let all = stdout + &String::from_utf8_lossy(&output.stderr);
        for private in [
            root.to_str().unwrap(),
            "PRIVATE_METADATA_PAYLOAD_CANARY",
            "GAME_PROGRESS_CANARY",
        ] {
            assert!(!all.contains(private));
        }
        assert!(
            !root.join("state.lock").exists()
                && !root.join("server.pem").exists()
                && !root.join("runtimes").exists()
                && !root.join("runtime-activity.json").exists()
        );
    }
    assert_eq!(
        fs::read(root.join("game.db")).unwrap(),
        b"GAME_PROGRESS_CANARY"
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn desktop_metadata_cli_child() {
    if let Ok(args) = std::env::var("METADATA_TEST_ARGS") {
        let args: Vec<String> = serde_json::from_str(&args).unwrap();
        if crate::desktop_metadata::run(&args).is_err() {
            std::process::exit(19);
        }
        panic!("expected metadata refusal");
    }
}

#[cfg(windows)]
#[test]
fn desktop_metadata_windows_file_container_generation_and_failures() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = root();
    let call = |op: &str, src: Option<&str>, dst: &str| parse(&args(&root, op, src, dst)).unwrap();
    let execute = |request: &Request| super::windows::execute(request, fs::File::sync_all);
    fs::write(root.join("game.db"), b"PROGRESS_CANARY").unwrap();
    for name in ["runtimes", "runtime-staging", "runtime-recovery-history"] {
        execute(&call("create-container", None, name)).unwrap();
        assert!(execute(&call("create-container", None, name)).is_err());
    }
    let fresh = "runtime-profile.json";
    let temporary = format!("{fresh}.partial-{UUID}");
    fs::write(root.join(&temporary), b"new profile").unwrap();
    execute(&call("publish-distinct", Some(&temporary), fresh)).unwrap();
    assert_eq!(fs::read(root.join(fresh)).unwrap(), b"new profile");
    assert!(!root.join(&temporary).exists());
    let destination = "runtime-activity.json";
    let source = format!("{destination}.partial-{UUID}");
    fs::write(root.join(destination), b"old").unwrap();
    fs::write(root.join(&source), b"ACTIVE").unwrap();
    let request = call("publish-replace", Some(&source), destination);
    assert!(
        super::windows::execute(&request, |_| Err(io::Error::other(
            "injected native flush failure"
        )))
        .is_err()
    );
    assert_eq!(fs::read(root.join(destination)).unwrap(), b"old");
    assert!(root.join(&source).exists());
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(root.join(destination))
        .unwrap();
    assert!(execute(&request).is_err());
    assert!(root.join(&source).exists());
    drop(locked);
    execute(&request).unwrap();
    assert!(!root.join(&source).exists());
    assert_eq!(fs::read(root.join(destination)).unwrap(), b"ACTIVE");
    let mut broken = Broken {
        bytes: vec![],
        limit: 5,
        flush_fails: false,
    };
    assert!(write_ack(&mut broken, &acknowledgement(&request).unwrap()).is_err());
    assert_eq!(fs::read(root.join(destination)).unwrap(), b"ACTIVE");
    fs::write(root.join(&source), b"must not replace").unwrap();
    assert!(execute(&call("publish-distinct", Some(&source), destination)).is_err());
    assert_eq!(fs::read(root.join(&source)).unwrap(), b"must not replace");
    fs::remove_file(root.join(&source)).unwrap();
    fs::create_dir(root.join("runtime-pending.json")).unwrap();
    let bad_source = format!("runtime-pending.json.partial-{UUID}");
    fs::write(root.join(&bad_source), b"must not replace directory").unwrap();
    assert!(
        execute(&call(
            "publish-replace",
            Some(&bad_source),
            "runtime-pending.json"
        ))
        .is_err()
    );
    assert!(root.join("runtime-pending.json").is_dir());
    let retired = format!("runtime-recovery-history/{}.activity.json", "a".repeat(64));
    execute(&call("move-distinct", Some(destination), &retired)).unwrap();
    assert_eq!(fs::read(root.join(retired)).unwrap(), b"ACTIVE");
    let stage = "runtime-staging/generation-Test1";
    execute(&call("create-container", None, stage)).unwrap();
    fs::create_dir(root.join(stage).join("runtime")).unwrap();
    fs::write(
        root.join(stage).join("generation.json"),
        b"synthetic descriptor",
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(root.join(stage).join("generation.json"))
        .unwrap()
        .sync_all()
        .unwrap();
    let generation = format!("runtimes/source-{}", "b".repeat(64));
    execute(&call("move-distinct", Some(stage), &generation)).unwrap();
    assert!(!root.join(stage).exists());
    assert!(root.join(generation).join("generation.json").exists());
    assert_eq!(fs::read(root.join("game.db")).unwrap(), b"PROGRESS_CANARY");
    assert!(!root.join("state.lock").exists() && !root.join("server.pem").exists());
    fs::remove_dir_all(root).unwrap();
}
