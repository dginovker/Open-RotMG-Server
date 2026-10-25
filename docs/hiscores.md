# Hiscores and Discord account actions

`GET /api/hiscores` supplies permanent Discord top-ten messages and full website rankings. **Living fame** (`base_fame`) is the highest fame of a currently alive character per account, or zero with none alive; **collection** counts unique enemy-drop pickups; **total fame** is spendable account fame, matching the Play screen. Death records credit base fame, not historical bonuses. `account_fame` is the authoritative wallet: death and credit commit together, historical deaths initialize it once, and restarting never overwrites spending. Future purchases must debit this wallet atomically; purchase systems are not implemented here. Registered native accounts qualify after connecting; linking is optional and shore bots/QA are excluded.

Collection and inventory commit atomically. Starter/map gear, trades, Vault moves and player-dropped items never earn credit. The loot tables define the collectible set; original sprites/silhouettes reject catalogue mismatches. There is no inventory backfill.

## Linking and self-disconnection

Discord `/link` provides a private five-minute browser button. Existing game login plus cookie-bound Discord OAuth `identify` proof automatically enables linking and recovery without application confirmation. Signed-out users use the normal game tab. Discord identity must match the caller; conflicts never replace associations.

Discord `/kick` resolves only the actual caller's linked, registered account. It signals that connection and confirms only after normal departure saves finish and ownership is released. Offline accounts are reported honestly. Failed saves retain quarantine ownership and return an error; neither ownership flags nor other players are cleared.

## Password recovery

After linking, Discord `/recover` returns a private 15-minute, one-use password-reset link. Enter the new password only on the website. In-game `/discord norecovery` revokes recovery and outstanding grants; `/link` can enable it again. Protect the linked Discord account: it can reset the game password.

Reset links use a 256-bit fragment token, erased from browser history on load; only its SHA-256 hash is stored. No token is sent in request URLs or logged. Only the latest link is valid, with one issuance/minute/account. Password validation and Argon2 hashing match native registration. Reset fences authentication through session ownership claims, saves/disconnects the active session, then atomically replaces the verifier and consumes reset secrets. Failed save, expired grant or revoked permission cannot change the password. No email service, Flash/SWF changes, unlinking, moderation or guest/name recovery is provided.

## API and backups

`POST /internal/discord/{link/start,link,kick,recover}` requires direct loopback and the dedicated bearer credential. Origin, forwarding headers and unknown fields are rejected. Identity always comes from the bot's actual interaction user. Caddy denies public `/internal/*`. The bot has no game DB access and never accepts passwords. Configure both services with `DISCORD_LINK_TOKEN_FILE`, using systemd `LoadCredential`, and the bot's explicit loopback `GAME_API_URL`.

Configure game-only `DISCORD_OAUTH_APPLICATION_ID` and `DISCORD_OAUTH_SECRET_FILE` through `/etc/openrotmg/discord-oauth.env`; protect the secret outside Git. Register the full HTTPS game-origin URL ending `/api/account/link/callback` exactly. Unconfigured linking fails; partial configuration prevents startup.

Cookie-bound OAuth prevents forwarded-link account takeover; a live, unambiguous game login is required. Linking state is memory-only: restart invalidates it; backups exclude it. Success requires a server-confirmed receipt.

`POST /api/account/recover` accepts only `{token,password}`, requires the exact browser Origin and has no-store responses. The static `/recover/` page has strict same-origin CSP, no referrers, third-party resources or browser storage.

Install/test schema-5 backup support before migration. Wallets and recovery permissions survive; GUIDs, credentials and all link/consent/reset hash tables do not. Restored links/permissions grant no login or reset authority without existing registered credentials. Disaster identity recovery remains separate. Rust/Clippy, exact backup roundtrips, encrypted two-account packet and website/Discord tests are not rendered production gameplay acceptance.
