# Server development and hosting

The launcher needs Git, Rust/Cargo and Python 3. Keep development separate from other worktrees and saves:

```sh
git worktree add -b feat/my-change ../my-change
cd ../my-change
./run.sh 8081 --state "$PWD/.local/qa" --client /absolute/path/to/client
```

`--client` (also `--local-client`) selects a prepared client checkout; omit it to download the pinned client into the selected state directory. The launcher prints its revision/dirty status and environment, retains logs in that state's `server.log`, and rejects an occupied port rather than choosing another. The binary also accepts `--state /absolute/path` with `--config`.

Add `--dev-control` for authenticated loopback gameplay checks using `python3 scripts/play.py --url http://127.0.0.1:8081 --state "$PWD/.local/qa"`. It is not needed to play normally. See the four recipes in `examples/testing/` for focused experiments; prepared state is not ordinary progression evidence. On Windows, use the sibling client's `run.bat`.

For hosting, build with `cargo build --release --locked` and run `openrotmg --config /etc/openrotmg/config.json`. The examples in `scripts/deploy/` keep TLS/static website serving at Caddy and the backend on loopback. Set explicit absolute data/client/state paths, the exact HTTPS origin, player limit and `realm_seed`; public configuration rejects development control. Local launches choose a random seed and log it. Keep state private and back up the persistent disk, including SQLite WAL, before upgrading.

`GET /health` reports monotonic process `uptime_seconds` and distinct native human `players`, autonomous `bots` and `qa_actors`. Population is enabled by default; explicit `"shore_bots": false` disables it. See the bot documentation for crew capacity and permanent-death rebuilding rules. Never enable production QA control. Private disk/SQLite backups retain the registry, identity and inventory. Credential-free exports preserve account/character bindings but omit login secrets; restoring those exports requires operator-reviewed identity recovery before bot startup.

Native registration converts a guest without changing account ownership; subsequent login uses email/password rather than the guest GUID. SQLite stores Argon2id verifiers. Registered accounts can choose one free, unique 3–10-letter name through the original client; name changes after that and email verification are not implemented.

Hiscores, drop-collection provenance, browser account linking, self-only `/kick` and Discord `/recover` are described in `docs/hiscores.md`. Native email-based password recovery is not implemented.

Independent credential-free backup and offline restore instructions are in `scripts/backups/OPERATIONS.md`.
