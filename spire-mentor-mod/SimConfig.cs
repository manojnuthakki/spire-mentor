// SimConfig.cs -- SpireMentor runtime configuration.
//
// Port note: new file, not present in the donor (zhiyue/sts2-rl-agent bridge_mod),
// which hardcoded its port/character/timeouts. Modeled loosely on
// refs/STS2MCP McpMod.LoadPort()'s "read JSON next to the DLL, write a default
// if missing" idiom.
//
// Reads SpireMentor.conf (JSON). Read precedence: next to SpireMentor.dll first
// (dev machines and the climb tooling keep their existing conf), else
// %LOCALAPPDATA%\SpireMentor (the only place a packaged install under Program
// Files can write -- GH #380). All fields are optional; missing/invalid
// fields fall back to defaults. When BOTH exist the mod-dir one wins and the
// ignored path is printed as an error -- README.txt sends users to the
// %LOCALAPPDATA% file, so a leftover mod-dir conf otherwise makes their edits
// look inert. If neither file exists, a copy of SpireMentor.conf.template is
// written to %LOCALAPPDATA%\SpireMentor so the user has something to edit; the
// mod dir is never written.

using System;
using System.IO;
using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;
using Godot;

namespace SpireMentor;

public sealed class SimConfig
{
    private const string ConfigFileName = "SpireMentor.conf";
    private const string TemplateFileName = "SpireMentor.conf.template";

    public int Port { get; init; } = 9002;
    public string Character { get; init; } = "Ironclad";
    public int Ascension { get; init; } = 0;
    public string? Seed { get; init; } = null;
    public int NumRuns { get; init; } = 1;
    public bool SpeedPatches { get; init; } = true;

    /// <summary>
    /// Per-request wait for an agent decision, in seconds. 0 means wait
    /// indefinitely. Per the real-game-bridge plan, this NEVER triggers a
    /// random/default fallback action -- on expiry we just log and keep
    /// waiting for the same decision. It only controls log/watchdog cadence.
    /// </summary>
    public int AgentTimeoutSeconds { get; init; } = 0;

    /// <summary>
    /// Arm autopilot at boot: the AI drives the main menu, Embarks and plays
    /// the whole run by itself (it only ENGAGES once an agent connects -- see
    /// SimRuntime.AutopilotEngaged).
    ///
    /// Defaults to FALSE, which is also what SpireMentor.conf.template ships.
    /// The mod must never take the controls off someone who did not ask for
    /// it: a packaged install used to boot armed, and since the advisor
    /// auto-launches and IS an agent, starting the game started a run
    /// (reported 2026-08-08 by the first outside tester). Every sweep and
    /// climb driver writes this flag explicitly (tools/ascension_climb.py),
    /// so the unattended lane is unaffected by the default.
    /// </summary>
    public bool AutoStart { get; init; } = false;

    /// <summary>
    /// Hands-free coached run: once a run is live, hand it from autopilot to
    /// the ADVISOR lane and arm autoplay, exactly as pressing F8 then F7
    /// would. Default false.
    ///
    /// This is the advisor lane's only regression harness. F7 is reachable
    /// solely by hotkey, so before this existed nothing could exercise
    /// `SimScreenAdvice.Describe()` without a human at the keyboard -- which
    /// is how six separate F7 stalls shipped together (2026-08-06), every one
    /// of them reproducible within ten minutes of an unattended run.
    ///
    /// Pair it with `auto_start=true` (SimAutoSlayer drives the main menu and
    /// Embark, then blocks harmlessly at the first decision because no bridge
    /// client is connected) and a fast advisor command, e.g.
    /// `--sims 8 --boss-sims 8`, which makes the play terrible and the screen
    /// coverage quick. Never use a run recorded this way for anything that
    /// measures play quality.
    /// </summary>
    public bool AdvisorAutoplay { get; init; } = false;

    /// <summary>
    /// Resume the run already in progress instead of starting a new one:
    /// SimAutoSlayer clicks the main menu's Continue button (visible and
    /// enabled exactly when a resumable `current_run.save` exists,
    /// NMainMenu.cs:710) and skips character select, the ascension panel and
    /// the seed override entirely -- the saved run already fixes all three.
    ///
    /// This exists because a wedge is not always the advisor's fault. The game
    /// itself can lock up (a render loop that spits `Parameter "particles" is
    /// null` at 30 MB/s and never advances a room, floor 13, 2026-08-06), and
    /// the only cure is killing the process. Without resume that costs the
    /// whole run -- at ~45 min a run, on a ladder that needs 35 wins, that is
    /// the difference between a climb and a treadmill. The game writes
    /// `current_run.save` on every room transition, so the loss is one room.
    ///
    /// Falls back to a normal new run when there is nothing to continue, so a
    /// driver may set it unconditionally on a retry.
    /// </summary>
    public bool ResumeRun { get; init; } = false;

    /// <summary>
    /// The game's FastModeType applied at run start: "instant" (skip
    /// animations entirely — same as the dev-console `instant` command,
    /// InstantConsoleCmd.cs; best for experiments), "fast", or "normal".
    /// Default "instant".
    /// </summary>
    public string FastMode { get; init; } = "instant";

    /// <summary>
    /// Whole-game speed (<c>Engine.TimeScale</c>) applied at run start,
    /// clamped by <see cref="SimSpeed"/> to the game's own [0.1, 4.0].
    /// Default 1.0 = untouched, which is the historical behaviour.
    ///
    /// Applied ONLY in Autopilot. In Advisor/Off a human is watching, and
    /// their speed is theirs to choose from the slider or F9/F10 — the
    /// unattended lane must not reach in and retune it.
    ///
    /// This is worth having because roughly 63% of a climb run's wall-clock
    /// is the game animating, not the search thinking (measured over runs
    /// 032/033: MCTS accounts for 37%, and it runs in another process where
    /// TimeScale cannot reach it). That caps the achievable speed-up near
    /// 1.9x however high this goes.
    ///
    /// It stacks with the two Autopilot multipliers — CustomScaledWait x0.1
    /// and Spine animations x5 — so at 4.0 the game's internal settle-waits
    /// run ~40x shorter than vanilla. Run 046 crashed with an access
    /// violation in the Spine animation path while a hand-set 4.0 was in
    /// effect (issue 15), n=1 and unattributed. Raise it deliberately, and
    /// watch refusals as well as crashes.
    /// </summary>
    public double TimeScale { get; init; } = 1.0;

    /// <summary>
    /// Phase 1b: when true, the mod does NOT auto-select a character/
    /// ascension/seed from this config at run start. Instead, once a client
    /// connects and no run is active, it sends a "run_config" message
    /// (available characters, max_ascension, and these config values as
    /// "defaults") and blocks indefinitely (never-guess policy, see
    /// SimAgentGate.cs) for a client "start_run" action naming the exact
    /// character/ascension/seed to use. Default false -- old auto-start
    /// behavior (character/ascension/seed straight from this config,
    /// no negotiation) is unchanged when this is off.
    /// </summary>
    public bool ClientRunConfig { get; init; } = false;

    /// <summary>
    /// Testing cheat: when &gt; 0, the player's max HP and current HP are set
    /// to this value right after the run starts (e.g. 999 so a weak/random
    /// agent survives deep enough to exercise late screens). 0 = off.
    /// NOTE: cheated runs are useless for eval/parity — gate this out of any
    /// experiment that measures win rate or compares against the headless sim.
    /// </summary>
    public int DebugPlayerHp { get; init; } = 0;

    /// <summary>
    /// Phase 4: whether the in-game HUD (SimHud.cs) renders autopilot
    /// annotations -- a persistent status chip plus per-hand-card score
    /// labels, driven by the optional "annotations" object any inbound
    /// action may carry (see PROTOCOL.md "annotations"). Purely visual/
    /// fire-and-forget; never affects gameplay. When false,
    /// SimHud.ApplyAnnotations is a no-op -- the annotations lane is inert
    /// (advisor-mode advice, which does not ride on actions, is governed by
    /// the BaseLib settings page instead). Default true.
    /// </summary>
    public bool Overlay { get; init; } = true;

    /// <summary>
    /// Mod-side stall limit, in seconds, replacing the game's hard-coded
    /// 30-second AutoSlay watchdog fuse (AutoSlayConfig.watchdogTimeout is
    /// static readonly — not patchable). A keep-alive loop feeds the game's
    /// fuse while REAL progress happened within this window and stops
    /// feeding it when progress stops, so a true wedge still aborts within
    /// stall_timeout_seconds + 30. The 2026-08-02 live sweep showed
    /// near-misses up to 25.2 s (game-over screen handling) against the
    /// stock 30 s fuse. Values below 30 are clamped to 30.
    /// </summary>
    public int StallTimeoutSeconds { get; init; } = 300;

    /// <summary>
    /// How long autopilot waits at launch for the game node and a visible
    /// main menu, in seconds, in all (it gives up without retrying). 60 by
    /// default; a value below 1 keeps the default. Under box64 on a 6-core
    /// arm64 machine the main menu can take about two minutes, so an
    /// emulated launch sets it higher.
    /// </summary>
    public int StartupWaitSeconds { get; init; } = DefaultStartupWaitSeconds;

    public const int DefaultStartupWaitSeconds = 60;

    /// <summary>
    /// A factor on autopilot's fixed waits once it runs (the run's first
    /// room, an event's proceed, a rewards screen, a combat phase, ...; see
    /// <see cref="Scaled"/>): 1 by default, and a value below 1 or not finite
    /// keeps 1, so a wait is never shortened. An emulated launch (box64)
    /// raises it: there the run's first room took more than 60 s.
    /// </summary>
    public double WaitScale { get; init; } = 1.0;

    /// <summary><paramref name="wait"/> times <see cref="WaitScale"/> (when it is valid).</summary>
    public TimeSpan Scaled(TimeSpan wait) =>
        double.IsFinite(WaitScale) && WaitScale > 1 ? wait * WaitScale : wait;

    /// <summary>The startup wait as a span, invalid values (below 1) read as the default.</summary>
    public TimeSpan StartupWait =>
        TimeSpan.FromSeconds(StartupWaitSeconds >= 1 ? StartupWaitSeconds : DefaultStartupWaitSeconds);

    /// <summary>
    /// Frame-rate cap applied WHILE an unattended agent/advisor think is in
    /// flight (see SimThinkFps.cs), released as soon as the answer arrives.
    /// The searches run in another process on this same machine, and
    /// rendering an idle decision screen at full rate starves them (~5x
    /// single-threaded throughput on this host, the dominant term in boss
    /// fight wall-clock). The game's own background mode uses MaxFps=30 for
    /// the same idea. 0 disables. Never engages for an attended session:
    /// only the autopilot gate and the advisor_autoplay lane call in.
    /// </summary>
    public int ThinkFps { get; init; } = 10;

    /// <summary>
    /// Per-run wall-clock cap, in minutes (was a hard-coded 60). Honest
    /// beliefdraw searches think 30-120 s per combat decision; a deep
    /// 400+-decision run legitimately exceeds an hour, and hitting the cap
    /// records a spurious "terminated". The stall guard above is what
    /// catches wedges — this cap is only the absolute backstop.
    /// </summary>
    public int RunTimeoutMinutes { get; init; } = 360;

    public static readonly SimConfig Instance = Load();

    private static SimConfig Load()
    {
        try
        {
            string? modDir = Path.GetDirectoryName(Assembly.GetExecutingAssembly().Location);
            string? modPath = modDir != null ? Path.Combine(modDir, ConfigFileName) : null;
            // Resolved in its own try: AppDataDir() creates the directory, and
            // GetFolderPath returns "" when LocalApplicationData is not mapped
            // -- which makes CreateDirectory target the CWD (the install dir)
            // and throw under Program Files. That must not cost us a perfectly
            // readable mod-dir conf, which is what the unattended lane writes.
            string? appDataPath = null;
            try
            {
                appDataPath = Path.Combine(SimAdvisorProcess.AppDataDir(), ConfigFileName);
            }
            catch (Exception ex)
            {
                GD.PrintErr(
                    $"[SpireMentor] %LOCALAPPDATA%\\SpireMentor is unusable ({ex.Message}); " +
                    $"only a {ConfigFileName} next to the DLL can be read.");
            }
            // Mod-dir conf wins when present (back-compat: this dev machine and
            // the ascension-climb tooling write/back up that file); packaged
            // installs only ever have the %LOCALAPPDATA% one.
            string? configPath = null;
            if (modPath != null && File.Exists(modPath))
            {
                configPath = modPath;
                if (appDataPath != null && File.Exists(appDataPath))
                {
                    // Loud, because README.txt points users at the
                    // %LOCALAPPDATA% file: a mod-dir conf left behind by an
                    // earlier install, a dev deploy or the climb tooling makes
                    // every edit they make there look simply inert.
                    GD.PrintErr(
                        $"[SpireMentor] TWO {ConfigFileName} files exist. Using {configPath} and " +
                        $"IGNORING {appDataPath} -- delete the former if you meant to edit the latter.");
                }
            }
            else if (appDataPath != null && File.Exists(appDataPath))
            {
                configPath = appDataPath;
            }
            if (configPath == null)
            {
                if (appDataPath != null)
                {
                    TryWriteDefaultConfig(modDir, appDataPath);
                }
                return new SimConfig();
            }
            GD.Print($"[SpireMentor] Loading config from {configPath}");

            string content = File.ReadAllText(configPath);
            using JsonDocument doc = JsonDocument.Parse(content);
            JsonElement root = doc.RootElement;
            var defaults = new SimConfig();

            return new SimConfig
            {
                Port = ReadInt(root, "port", defaults.Port),
                Character = ReadString(root, "character", defaults.Character),
                Ascension = ReadInt(root, "ascension", defaults.Ascension),
                Seed = ReadNullableString(root, "seed", defaults.Seed),
                NumRuns = ReadInt(root, "num_runs", defaults.NumRuns),
                SpeedPatches = ReadBool(root, "speed_patches", defaults.SpeedPatches),
                AgentTimeoutSeconds = ReadInt(root, "agent_timeout_seconds", defaults.AgentTimeoutSeconds),
                AutoStart = ReadBool(root, "auto_start", defaults.AutoStart),
                AdvisorAutoplay = ReadBool(root, "advisor_autoplay", defaults.AdvisorAutoplay),
                ResumeRun = ReadBool(root, "resume_run", defaults.ResumeRun),
                FastMode = ReadString(root, "fast_mode", defaults.FastMode),
                TimeScale = ReadDouble(root, "time_scale", defaults.TimeScale),
                ClientRunConfig = ReadBool(root, "client_run_config", defaults.ClientRunConfig),
                DebugPlayerHp = ReadInt(root, "debug_player_hp", defaults.DebugPlayerHp),
                Overlay = ReadBool(root, "overlay", defaults.Overlay),
                StallTimeoutSeconds = ReadInt(
                    root, "stall_timeout_seconds", defaults.StallTimeoutSeconds),
                ThinkFps = ReadInt(root, "think_fps", defaults.ThinkFps),
                RunTimeoutMinutes = ReadInt(
                    root, "run_timeout_minutes", defaults.RunTimeoutMinutes),
                StartupWaitSeconds = ReadInt(
                    root, "startup_wait_seconds", defaults.StartupWaitSeconds),
                WaitScale = ReadDouble(root, "wait_scale", defaults.WaitScale),
            };
        }
        catch (Exception ex)
        {
            GD.PrintErr($"[SpireMentor] Failed to load {ConfigFileName}: {ex.Message}. Using defaults.");
            return new SimConfig();
        }
    }

    private static void TryWriteDefaultConfig(string? modDir, string configPath)
    {
        try
        {
            // The template ships in the mod dir (read-only is fine); the
            // editable copy goes to %LOCALAPPDATA%\SpireMentor, which is
            // writable without elevation on a default Steam install.
            string? templatePath = modDir != null ? Path.Combine(modDir, TemplateFileName) : null;
            if (templatePath != null && File.Exists(templatePath))
            {
                File.Copy(templatePath, configPath);
            }
            else
            {
                File.WriteAllText(configPath, JsonSerializer.Serialize(new SimConfig(),
                    new JsonSerializerOptions { WriteIndented = true }));
            }
            GD.Print($"[SpireMentor] Created default config at {configPath}");
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException)
        {
            GD.Print($"[SpireMentor] No config found at {configPath}; using built-in defaults ({ex.Message})");
        }
    }

    private static int ReadInt(JsonElement root, string name, int fallback) =>
        root.TryGetProperty(name, out JsonElement el) && el.ValueKind == JsonValueKind.Number && el.TryGetInt32(out int v)
            ? v
            : fallback;

    private static double ReadDouble(JsonElement root, string name, double fallback) =>
        root.TryGetProperty(name, out JsonElement el) && el.ValueKind == JsonValueKind.Number &&
        el.TryGetDouble(out double v)
            ? v
            : fallback;

    private static bool ReadBool(JsonElement root, string name, bool fallback) =>
        root.TryGetProperty(name, out JsonElement el) &&
        (el.ValueKind == JsonValueKind.True || el.ValueKind == JsonValueKind.False)
            ? el.GetBoolean()
            : fallback;

    private static string ReadString(JsonElement root, string name, string fallback) =>
        root.TryGetProperty(name, out JsonElement el) && el.ValueKind == JsonValueKind.String
            ? el.GetString() ?? fallback
            : fallback;

    private static string? ReadNullableString(JsonElement root, string name, string? fallback) =>
        root.TryGetProperty(name, out JsonElement el)
            ? (el.ValueKind == JsonValueKind.String ? el.GetString() : null)
            : fallback;
}
