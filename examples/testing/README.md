# Four testing recipes

Copy and adapt these into scratch work. They are examples, not a scenario framework; adding shared helpers or more examples requires explicit approval. Prepared state never proves earned progression. Keep formatting, focused tests and long native journeys separate.

- **Cover:** `cargo test --locked example_cover` constructs a hostile shot behind cover using real collision code.
- **Timing:** `cargo test --locked example_timing` sends SHOOT before MOVE. For sustained-fire and skipped-clock variants, see the existing native attack regressions.
- **Prepared state:** default to a new owned `./run.sh PORT --test-state NEW_DIRECTORY --no-shore-bots --client CLIENT_DIRECTORY` session. It starts paused; `node scripts/game.mjs --help` documents authenticated explicit spawning, readable observations, bounded actions, actual gameplay-bot goals and coordinated native pause. Every spawn specifies class, level, seed, all equipment/backpack slots, world, coordinates and controller; no hidden gear or immunity. Keep earned journeys separate. Never use a production identity/profile.
- **UI transition:** import `exampleNexusReturn` from `ui-transition.mjs`; pass your owned tab's CDP call function, its server log path and character ID. Start outside Nexus after the game HUD has rendered. The recipe uses page-local focus emulation and releases it afterward; it never activates a desktop window. It sends the selected client's Insert binding and requires a NEW same-character Nexus entry. Key-release failure cannot hide the original error. This demonstrates observation, not the boss portal's UI selection.

On KDE/Wayland, start a private GPU-backed virtual compositor in its own session bus; never attach it to host KWin. Use a fresh absolute `QA_DISPLAY` directory and distinct port/profile:

```sh
mkdir -m 700 "$QA_DISPLAY" "$QA_DISPLAY/config" "$QA_DISPLAY/data"
env -u DISPLAY -u WAYLAND_DISPLAY XDG_RUNTIME_DIR="$QA_DISPLAY" \
  XDG_CONFIG_HOME="$QA_DISPLAY/config" XDG_DATA_HOME="$QA_DISPLAY/data" \
  QT_QPA_PLATFORM=offscreen dbus-run-session -- kwin_wayland --virtual \
  --socket rotmg-qa --width 1400 --height 1000 --no-global-shortcuts \
  --no-kactivities --no-lockscreen
```

In another terminal, retain the normal user bus/runtime for the managed launcher and Chrome's keyring, but point Chrome exclusively at that private display:

```sh
env -u DISPLAY WAYLAND_DISPLAY="$QA_DISPLAY/rotmg-qa" node --input-type=module - <<'JS'
import {launchBrowser} from '/home/wacket/.local/share/pi-browser-tools/launch-browser.mjs';
console.log(await launchBrowser({owner:process.env.PI_BROWSER_OWNER,args:[
  `--remote-debugging-port=${process.env.CHROME_PORT}`,
  `--user-data-dir=${process.env.CHROME_PROFILE}`, '--ozone-platform=wayland',
  '--window-size=1280,960', '--no-first-run', '--no-default-browser-check', 'about:blank',
]}));
JS
```

Export the port/profile variables; jobs outside Pi must supply an explicit `PI_BROWSER_OWNER`. Use managed target creation/activity helpers. Close owned targets/Chrome before stopping the owned compositor; preserve profiles. Do not use host `Page.bringToFront`, `Target.activateTarget` or MCP `bringToFront:true`. Focus emulation alone does not guarantee render cadence: verify actual frames and native outcomes, without host activation as repair. Xvfb alone failed native rendering here (WebGL blocklisting/AVM2 errors); do not silently substitute it or relax GPU/security flags.

Rust recipes compile with the existing tests. No browser dependency is bundled: use your existing browser-scoped connection, never desktop input. Keep generated state, media and exploratory scripts outside Git.

```sh
# Run in the qualified build environment; retain outside the shared cache.
python3 scripts/cargo.py --output "${RETAINED_BINARY:?Set an output path outside the cache}" \
  --output-bin openrotmg-build100 build --locked -p openrotmg-build100 --bin openrotmg-build100
```
