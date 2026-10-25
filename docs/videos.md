# Gameplay demos (optional)

Use `scripts/record-demo.mjs` for capture, cursor timing, rendering and technical validation. Gameplay assertions remain in your scenario; recording success does not prove gameplay success. Capture the real before state before editing, then verify after through normal client gameplay. Keep state, scenarios and media outside Git.

Requires Node 22+, Python 3/Pillow, FFmpeg/ffprobe, Noto Sans fonts and the existing owned-tab managed-CDP helper. Use the private-display launch recipe in `examples/testing/README.md`. Prepare a fresh owned gameplay target in that managed Chrome; never use desktop input, activate host windows or use another owner's browser. The tool does not bring tabs forward or restore/fullscreen windows. A hidden target fails readiness instead of activating the desktop. Recording consumes and closes that target, including failed takes.

```sh
node scripts/record-demo.mjs record \
  --port "$CHROME_PORT" --target "$TARGET_ID" --url "$EXACT_GAME_URL" \
  --helper "$ABSOLUTE_MANAGED_CDP_HELPER" \
  --scenario "$SCENARIO_MODULE" --out "$NEW_TAKE_DIRECTORY" --seconds 17

node scripts/record-demo.mjs render \
  --take "$NEW_TAKE_DIRECTORY" --start 0 --date 2026-10-13
```

The scenario exports `async run(input)`. Its browser-scoped methods are `move([x,y])`, `gesture(from,to,ms,hz)`, `click([x,y])`, `key(name,ms)` and `wait(ms)`; points are normalized to the original canvas. Use `wait` for stationary holds. Keep the scenario bounded and verify real outcomes separately through read-only observations. No injected gameplay state, invented cursor interpolation, reload-as-repair or overlapping controllers.

The tool waits for observed non-minimized window state and a visible document (not simulated focus), fails if the page becomes hidden, retains failed takes, probes actual paint/input cadence, transfers Blobs in bounded chunks, and exports continuous original-canvas footage with faithfully held received cursor positions. Reports separate measured source FPS and moving-cursor cadence from export FPS. Current qualification requires average decoded source FPS and median moving-cursor Hz ≥30, with bounded gaps; it is not a visual-motion classifier or a guarantee that an occluded/slow browser can be repaired. Inspect the explicit failure instead of relaxing it silently.

Rendering produces `demo.mp4`, `contact-sheet.jpg`, `quality.json` and `export.json`: sparse Open RotMG/date opening, authentic 4:3 gameplay with blurred sides, silent 15-second/1080p/60 FPS H.264 under 10 MB. Optional `--before SECONDS --after SECONDS` labels must correspond to real footage. Watch framing/privacy and verify the gameplay outcome before publication; preserve genuine client UI, with no debug caption sidebar or pixel erasure.

Publication is a separate authorized action, requiring current `gh --attach` support, push access to the explicit destination, and a fresh owned target already on that PR:

```sh
node scripts/record-demo.mjs publish \
  --take "$NEW_TAKE_DIRECTORY" --repo OpenRotMG/Open-RotMG-Server --pr "$PR_NUMBER" \
  --port "$CHROME_PORT" --target "$PR_TARGET_ID" --helper "$ABSOLUTE_MANAGED_CDP_HELPER"
```

It checks downloaded attachment bytes and actual inline playback. No permission changes, releases or media branches. Fork-only contributors can use their own public fork's PR and link the verified asset upstream, or request a maintainer attachment.

Tests:

```sh
node --test scripts/test_record-demo.mjs
python3 -m unittest discover -s scripts -p test_demo_media.py
```
