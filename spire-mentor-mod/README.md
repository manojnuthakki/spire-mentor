# Spire Mentor

(Called *Spire Coach* in 0.3.0 and *SimBridge* before that, and kept in
`sts2-bridge/` — older logs, experiment records and installs under
`mods/SpireCoach/` or `mods/SimBridge/` use those names.)

A C# mod for **Slay the Spire 2** (game build **v0.107.1**, Windows) that
connects the real, running game to an external AI over a local TCP socket.
Two ways to use it:

- **Advisor mode** — you play; the mod streams each decision to an AI backend
  and renders its advice in-game (card scores, ranked moves, win chance, map
  route). Switches itself on when the game starts (Windows); untick
  "Turn the coach on when the game starts" in the mod settings to stop that.
- **Autopilot mode** — an external agent plays whole runs by itself
  (used for unattended sweeps and evaluation against the headless
  [`sts2-rs`](../sts2-rs) simulator).

Spire Mentor started as a port of zhiyue's **STS2BridgeMod**
([zhiyue/sts2-rl-agent](https://github.com/zhiyue/sts2-rl-agent)) onto game
build v0.107.1. In 2026-10 everything derived from it, and everything that
mirrored the game's own AutoSlay framework, was rewritten clean-room from a
behavioural spec, so the mod carries no code from either (decision D5 in
[`../docs/open-source-plan.md`](../docs/open-source-plan.md)). It is MIT.

## How it works

1. Harmony-patches `NGame.IsReleaseGame()` to `false`, unlocking the game's
   built-in `AutoSlay` debug framework (`ICardSelector` override,
   `NonInteractiveMode`, epoch unlocks).
2. Runs a newline-delimited-JSON TCP server on `127.0.0.1:9002`
   (configurable). [`PROTOCOL.md`](PROTOCOL.md) is the complete wire
   reference: every message type, field, and inbound verb.
3. In Autopilot, `SimAutoSlayer` (a mirror of the game's own `AutoSlayer`)
   drives menus, rooms, and screens, with every decision handler replaced by
   one that asks the connected agent — **never a random fallback**: if no
   agent answers, the mod waits.
4. In Advisor mode nothing is patched into the game flow; the mod watches
   public game events, requests advice over the same socket, and draws it via
   the in-game HUD (`SimHud.cs`).

All three behavior patches self-gate on autopilot actually being engaged
(mode Autopilot + slayer running + agent connected), so an installed but idle
mod leaves the game 100% vanilla.

## Requirements

- Windows, with your own Steam install of Slay the Spire 2 at **v0.107.1**
  (the mod patches game internals and clicks concrete UI scene paths — a game
  update can break it silently).
- .NET 9 SDK (`winget install Microsoft.DotNet.SDK.9`, or the repo root's
  `install-windows-deps.ps1`).
- [Alchyr's BaseLib](https://github.com/Alchyr/BaseLib-StS2) (MIT, v3.4.1
  verified) — it provides the mod settings page, and the build **fails with a
  clear error** if it is missing. The repo root's `install-windows-deps.ps1`
  installs it (hash-checked) into `<game>/mods/BaseLib/`, finding the game
  as the build does. By hand: from the
  [v3.4.1 release](https://github.com/Alchyr/BaseLib-StS2/releases) download
  `BaseLib.dll`, `BaseLib.pck` and `BaseLib.json` into
  `<game>/mods/BaseLib/` (create the folder; `<game>` is the folder holding
  `SlayTheSpire2.exe`, e.g.
  `C:\Program Files (x86)\Steam\steamapps\common\Slay the Spire 2`).
- For the in-game advisor: the repo's Python side, set up once (below).

## Build and install

Players don't need any of this: the mod is on the
[Steam Workshop](https://steamcommunity.com/sharedfiles/filedetails/?id=3821016481).

```powershell
cd spire-mentor-mod
dotnet build
```

Coming from an older build? Delete `<game>/mods/SimBridge/` or
`<game>/mods/SpireCoach/` first — the game would load both, and only one can
listen on port 9002.

`SpireMentor.csproj` finds the game like `just import-game` does: in every
Steam library listed in Steam's `libraryfolders.vdf` (other drives included),
falling back to `C:/Program Files (x86)/Steam/steamapps/common/Slay the Spire 2`.
`deploy.ps1` uses the same folder. To pick a specific install, set
`STS2_GAME_DIR` (environment variable or MSBuild property):

```powershell
dotnet build -p:STS2_GAME_DIR="D:/SteamLibrary/steamapps/common/Slay the Spire 2"
```

**Compile-only on a machine without the game (Linux dev box):** the csproj
only needs the game-dir *layout*, so assemble one from what the monorepo
already carries and point `STS2_GAME_DIR` at it —
`<dir>/data_sts2_windows_x86_64/{sts2.dll,0Harmony.dll,GodotSharp.dll}`
(`sts2.dll` from `sts2_binaries/v0.107.1/…`; the other two from any sibling
mod's `lib/`) plus `<dir>/mods/BaseLib/BaseLib.dll` (v3.4.1 from the BaseLib
release, NOT an older v0.1.0 copy — that one lacks `ConfigIgnore` /
`ConfigSlider`). Add `-p:SpireMentorSkipDeploy=true` so the post-build copy
does not run. This proves the mod compiles; it does not run it.

It compiles against `sts2.dll` / `0Harmony.dll` / `GodotSharp.dll` straight
from the game's data dir (plain `Microsoft.NET.Sdk`, no Godot SDK or NuGet
feeds). Every successful build auto-deploys `SpireMentor.dll` +
`SpireMentor.json` + `SpireMentor.conf.template` to `<game>/mods/SpireMentor/`
(skip with `-p:SpireMentorSkipDeploy=true`, e.g. while the game is running and
locking the DLL); `./deploy.ps1` redeploys without rebuilding.

To assemble a self-contained mod zip for another machine (bundling BaseLib
and the packaged advisor backend), see `package.ps1` and
`packaging/README-dist.txt` — and note the D5 distribution caveat above.
The package holds no `.json` but `SpireMentor.json`: the game reads every
`.json` under a mod's folder as a manifest (`ModManager.cs`,
`ReadModsInDirRecursive`), so the workbench's and the advisor's data ship
gzipped as `.json.gz`, and `package.ps1` fails on any it finds.

For the Steam Workshop, `workshop.ps1` stages the same files (without
BaseLib, which the item lists as a required Workshop item) into
`workshop-workspace/` for Mega Crit's
[uploader](https://github.com/megacrit/sts2-mod-uploader); `-Upload` runs
it. The item's title, tags and dependencies are in
`packaging/workshop/workshop.json`, its page text in
`packaging/workshop/description.bbcode`, its preview image in
`packaging/workshop/image.png` (Steam needs it under 1 MB), and its id in
`packaging/workshop/mod_id.txt` once the first upload has created it. To
test the subscribed copy, remove `<game>/mods/SpireMentor/` first: the
game disables a Workshop mod whose id is also in the local mods folder
(`ModManager.cs:296`).

## Configuration (`SpireMentor.conf`)

On first run the mod copies `SpireMentor.conf.template` to `SpireMentor.conf`
next to the DLL. Player-install defaults never let the AI play by itself; the
coach (Advisor mode) switches on at launch unless you untick that in the mod
settings. Fields (authoritative docs: `SimConfig.cs`):

| Field | Default | Meaning |
|---|---|---|
| `port` | `9002` | TCP listen port (loopback only) |
| `auto_start` | `false` | Launch Autopilot at boot (the unattended sweep lane). `false` = Off, or Advisor if the "Turn the coach on when the game starts" setting is ticked (default; Windows only) |
| `advisor_autoplay` | `false` | Testing: once a run is live, switch to Advisor mode and toggle AI autoplay (the F8+F7 path) unattended |
| `character` / `ascension` / `seed` | `"Ironclad"` / `0` / `null` | Run setup for Autopilot; `seed: null` = fresh random seed |
| `num_runs` | `1` | Runs to play in sequence before going idle |
| `resume_run` | `false` | Click Continue instead of starting a new run (recover a run after a game crash) |
| `speed_patches` | `true` | Faster-than-realtime patches while Autopilot is engaged |
| `fast_mode` | `"normal"`* | Game `FastModeType` at run start: `"instant"` / `"fast"` / `"normal"` |
| `time_scale` | `1.0` | Whole-game `Engine.TimeScale` (0.1–4.0), Autopilot only |
| `agent_timeout_seconds` | `0` | `0` = wait indefinitely for the agent; `>0` = watchdog log cadence while waiting (never a fallback action) |
| `client_run_config` | `false` | Let the TCP client pick character/ascension/seed via the `run_config`/`start_run` handshake (PROTOCOL.md) |
| `debug_player_hp` | `0` | Testing cheat: `>0` sets player HP at run start; never use for measurements |
| `overlay` | `true` | Render autopilot `annotations` (status chip + per-card score labels) in the HUD |
| `stall_timeout_seconds` | `300` | Mod-side stall limit replacing the game's 30 s AutoSlay watchdog |
| `startup_wait_seconds` | `60` | Autopilot's wait at launch for the game node and a visible main menu, in all (no retry). Below 1 keeps the default. Raise it for an emulated launch (box64), where the menu can take about two minutes |
| `wait_scale` | `1.0` | Factor on autopilot's fixed waits once it runs (first room, event proceed, rewards, combat phases, the game's `nodeWaitTimeout` and `mapScreenTimeout`, ...); below 1 or not finite keeps 1, so no wait is shortened. Raise it for an emulated launch (box64) |
| `run_timeout_minutes` | `360` | Absolute per-run wall-clock cap |

\* the template ships `"normal"` for player installs; the in-code default is
`"instant"` (best for unattended experiments).

## In-game controls

- **Settings page** — main menu → Mod Configuration (BaseLib) → Spire Mentor:
  advisor-mode toggle, game-speed slider, HUD toggles, advisor-backend launch
  settings. Main-menu only.
- **Toolbar** (`SimToolbar.cs`) — the clickable `SpireMentor` strip in the
  top-left mirrors every hotkey. The live controls (coach, AI once / auto,
  speed) are flat buttons; the one-shot verbs sit in three dropdowns
  (`SimUiKit.MakeMenuButton`): **Rewind ▾** (`⟲ Turn` / `⟲ Room` /
  `⟲⟲ Back`), **Panels ▾** (log, HUD, help, mid-run **Settings** —
  `SimSettingsPanel.cs`) and **Workbench ▾** (open the browser view, copy
  state). Click the pill to collapse the strip.
- **Hotkeys** (`SimHotkeys.cs`) — `F1` help panel (the in-game documentation:
  current mode, every key, file locations), `F3` action-log panel, `F4` HUD
  on/off, `F6` AI takes this one decision, `F7` AI plays until you stop,
  `F8` Advisor mode on/off (from Autopilot: stops the AI and hands you the
  controls), `F9`/`F10` game speed −/+, `Ctrl+Z` undo to the start of the
  combat turn, `Ctrl+Shift+Z` restart the room. `F2` and `F11` belong to the
  game.
- **Rewind** (`SimSnapshots.cs` / `SimRestore.cs` / `SimCombatUndo.cs`) —
  the mod snapshots every game save (room entry + post-combat) under
  `%LOCALAPPDATA%\SpireMentor\snapshots\` and can reload any of them
  in-process, bit-exact (RNG counters included). Turn undo replays the
  game's own combat tape on top of the room-entry save; completed fights'
  tapes are persisted next to their snapshots, so PAST combats replay too.
  Protocol §7 (`list_snapshots` / `restore_snapshot` / `undo_turn`) exposes
  the same to the workbench, whose game tree rewinds to any room — or any
  recorded combat turn — and tracks forks.
- **Console** (`F11`) — `spirementor` (status), `spirementor mode off|advisor`,
  `spirementor speed <0.1-4.0>`, `spirementor hud on|off`, `spirementor log`,
  `spirementor waitmult <f>` / `spirementor animmult <f>` (Autopilot-only
  multipliers).

## Advisor backend

The AI on the other end of the socket lives in
[`../sts2-pycli`](../sts2-pycli): an MCTS search for combat plus a trained
run policy for map routes, card rewards, shops, events, and rest sites.
`SimAdvisorProcess.cs` auto-launches it when Advisor mode needs one and no
client is connected, preferring (in order) the command configured on the
settings page, the dev launcher `SimAdvisor.cmd` written next to the DLL at
deploy time (runs `sts2-pycli/python/realgame_advisor.py` via `uv`), and the
packaged torch-free `SimAdvisor` exe bundled by `package.ps1`. Any process
that speaks [`PROTOCOL.md`](PROTOCOL.md) can serve instead — connect it to
`127.0.0.1:<port>` and answer the decision messages.

**Set it up once before the first game launch.** The dev launcher runs
`uv run` in `sts2-pycli/`; on a fresh clone that first syncs the whole
environment — downloads torch and friends and compiles the `sts2_rs` Rust
extension — which takes 10+ minutes, while the game waits for its advisor.
Do it ahead:

```powershell
cd sts2-pycli
uv sync                 # BUILDING-WINDOWS.md §4 if the sts2-py build cannot find Python
```

After a `git pull` that touches Rust code, the next launch rebuilds `sts2_rs`
again (a few minutes); `uv sync` beforehand avoids the wait there too.

## Repository tour

| File | Purpose |
|---|---|
| `MainFile.cs` | `[ModInitializer]` entry point, Harmony patches, logging |
| `BridgeServer.cs` | TCP server: newline-JSON, `request_id` correlation, advice lane |
| `SimConfig.cs` | `SpireMentor.conf` loader — the config reference |
| `SimAutoSlayer.cs` | Autopilot run orchestrator (menu nav, room/screen dispatch) |
| `SimAgentGate.cs` | Blocking "ask the agent, never guess" helper |
| `SimStateExtractor.cs` | Shared full-fidelity state serialization (cards, creatures, intents, relics, map graph) |
| `SimProtocolHelpers.cs` | Stale-state (`state_id`) guard, structured errors, choice parsing |
| `SimCombatHandler.cs`, `SimMapHandler.cs`, `SimNonCombatRoomHandlers.cs`, `SimCardSelector.cs`, `Sim*ScreenHandler.cs` | Per-surface decision handlers |
| `SimAdvisor.cs`, `SimScreenAdvice.cs`, `SimAdvisorAutoplay.cs`, `SimAdvisorProcess.cs` | Advisor mode: observe-and-advise loop, screen describers, F7 autoplay, backend auto-launch |
| `SimHud.cs`, `SimActionLog.cs`, `SimHelp.cs`, `SimHotkeys.cs`, `SimConsoleCommands.cs`, `SpireMentorModConfig.cs` | In-game UI and controls |
| `SimMode.cs`, `SimSpeed.cs`, `SimShutdown.cs`, `SimJobObject.cs` | Runtime mode/speed state, clean shutdown, child-process containment |
| `SpireMentor.json` | Mod manifest (`dependencies: [{ id: "BaseLib", min_version: "3.4.7" }]`, `affects_gameplay: true`, `min_game_version: "0.107.1"`) |
| `PROTOCOL.md` | Full wire-protocol reference |
| `tools/` | Unattended drivers and log graders: `ascension_climb.py` (drive the advisor lane up the ascension ladder), `check_f7_log.py` (assert known stall signatures in a game log) |

How the mod fits the rest of the stack: [`../ARCHITECTURE.md`](../ARCHITECTURE.md).

## The workbench link

The toolbar's **Workbench ▾ → open** opens the *Workbench URL* setting.
Blank (the default) tries two pages in order (`SimWorkbenchLink.cs`):

1. **The bundled workbench**, `http://127.0.0.1:8079/?follow=1`. The release zip
   ships the browser-only build in `mods/SpireMentor/workbench/`
   (`package.ps1` builds it with `VITE_BRIDGE_SAME_ORIGIN=1`, with the art
   and sound of the local `just import-game` tree unless `-NoArt`), and the
   advisor's control plane hosts it next to its JSON endpoints
   (`--workbench-dir`; in a dev checkout, `sts2-pycli/web/dist` whenever
   that is a static build). Page and control plane share an origin, so the
   live game's tree, live search and rewind work with no opt-in, and
   `?follow=1` follows the advisor's run log as on the dev workbench (the
   page mirrors it from the control plane's `GET /log`, `web/src/liveLog.ts`).
   It answers `GET /api/health` only while it has a build to serve.
2. **The dev workbench**, `http://127.0.0.1:8077/?follow=1`: the
   Python-backed one, which the advisor starts on its own in dev installs.
   It serves whatever is built in `sts2-pycli/web/dist`:

- **Server build** — `cd sts2-pycli/web; npm run build`. `?follow=1` opens
  the advisor's live run log in the replay view and follows it.
- **Browser-only build** — what the repo root's `just workbench-local`
  builds (and serves on `http://127.0.0.1:4173`). It has no server to proxy
  for it, so it talks to the advisor's control plane (`127.0.0.1:8079`)
  straight from the page: it can show the live game's tree, run a live search
  and rewind the game. That is off unless you allow the page's origin: set
  **Browser workbench origin** in the mod's settings to it (e.g.
  `http://127.0.0.1:4173`, and set **Workbench URL** to the same page), which
  the mod passes to the advisor as `--control-allow-origin`; the control
  plane then answers that origin with the CORS / Private Network Access
  headers and nothing else changes. In the page, Tools → **Live game…** takes
  the control plane's URL. Chrome asks once for local-network access; Firefox
  does not. Details and the browser measurements: `docs/browser-workbench.md`,
  "Live-game bridge".
