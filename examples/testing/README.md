# Native scenarios

Set `PI_BROWSER_TOOLS` to the absolute managed-browser helper directory and `PI_NATIVE_TEST_CHROME` to the absolute Chrome executable. Linux needs Node 22.15+, Xvfb, xauth, Openbox and the normal user session bus. `PI_NATIVE_TEST_CHROME_ARGS` optionally supplies a JSON array of rendering arguments.

```sh
node scripts/native-test.mjs examples/testing/item-use.mjs --run-dir /absolute/new-run
```

Copy the small item-use case (`fixture` and `run(game)`), not setup code. The runner owns a private display/browser, fresh guest, bounded actions, captures and cleanup. Existing run directories are refused; keep outputs outside Git.

Prepared fixtures do not prove earned progression; keep normal journeys separate. Never use host input, `Page.bringToFront` or target activation; use page-local focus only.

The protected `persona` starting-context operation takes an actual catalog `profile`, `world` and walkable `position`, requires a paused fresh test registration, and uses ordinary persona provisioning/replacement. Existing `game_bot` spawns remain pinned and stop on death. Privileged `world_info.damaging_ground` exposes up to 16 real walkable hazard positions; it does not create damage or death.

`result.json` records timings and assertions. Failures exit nonzero and retain state/logs. Discover controller operations and limits with `node scripts/game.mjs --help`.
