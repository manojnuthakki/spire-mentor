// SimAutoSlayer.cs -- SpireMentor's autopilot run driver.
// Originally ported from zhiyue/sts2-rl-agent (STS2BridgeMod); rewritten clean-room.
//
// Plays configured runs end to end on top of the game's AutoSlay framework
// interfaces (IRoomHandler / IScreenHandler, WaitHelper, UiHelper, Watchdog),
// but with SpireMentor's own handlers, so every gameplay decision is made by
// the connected bridge agent. This file owns:
//   - the multi-run loop, per-run timeout, failure recovery and the
//     "exactly one run_complete per run" guarantee;
//   - main-menu navigation (resume / abandon / character / ascension / seed)
//     and the optional client run_config handshake;
//   - the room state machine and the overlay-screen drain;
//   - the run watchdog (mirrored into the game's AutoSlayer.CurrentWatchdog)
//     and its configurable stall-limit keepalive.
// SimGameOverScreenHandler (bottom of the file) reports game_over.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Reflection;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Godot;
using MegaCrit.Sts2.Core.AutoSlay;
using MegaCrit.Sts2.Core.AutoSlay.Handlers;
using MegaCrit.Sts2.Core.AutoSlay.Handlers.Screens;
using MegaCrit.Sts2.Core.AutoSlay.Helpers;
using MegaCrit.Sts2.Core.Commands;
using MegaCrit.Sts2.Core.Context;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Helpers;
using MegaCrit.Sts2.Core.Map;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Nodes;
using MegaCrit.Sts2.Core.Nodes.CommonUi;
using MegaCrit.Sts2.Core.Nodes.Events;
using MegaCrit.Sts2.Core.Nodes.Events.Custom.CrystalSphere;
using MegaCrit.Sts2.Core.Nodes.GodotExtensions;
using MegaCrit.Sts2.Core.Nodes.Screens;
using MegaCrit.Sts2.Core.Nodes.Screens.CardSelection;
using MegaCrit.Sts2.Core.Nodes.Screens.CharacterSelect;
using MegaCrit.Sts2.Core.Nodes.Screens.GameOverScreen;
using MegaCrit.Sts2.Core.Nodes.Screens.Map;
using MegaCrit.Sts2.Core.Nodes.Screens.Overlays;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Runs;
using MegaCrit.Sts2.Core.Saves;
using MegaCrit.Sts2.Core.Settings;
using MegaCrit.Sts2.Core.Timeline;
using MegaCrit.Sts2.Core.Timeline.Epochs;

namespace SpireMentor;

public class SimAutoSlayer
{
    // ---- main-menu node paths (relative to the main menu control) ----
    private const string MainMenuNodePath = "/root/Game/RootSceneContainer/MainMenu";
    private const string MenuAbandonButtonPath = "MainMenuTextButtons/AbandonRunButton";
    private const string ContinueRunMenuButtonPath = "MainMenuTextButtons/ContinueButton";
    private const string SingleplayerMenuButtonPath = "MainMenuTextButtons/SingleplayerButton";
    private const string CharSelectScreenNodePath = "Submenus/CharacterSelectScreen";
    private const string StandardModeButtonPath = "Submenus/SingleplayerSubmenu/StandardButton";
    private const string CharConfirmButtonNodePath = "Submenus/CharacterSelectScreen/ConfirmButton";
    private const string CharButtonContainerNodePath = "CharSelectButtons/ButtonContainer";
    private const string AscensionPanelUniquePath = "%AscensionPanel";
    private const string AbandonPopupYesPrimaryPath = "VerticalPopup/YesButton";
    private const string AbandonPopupYesFallbackPath = "YesButton";

    // ---- timings (tuned; see SPEC 12.4) ----
    private const int RoomSettleMs = 500;
    private const int MenuClickSettleMs = 500;
    private const int CharacterSelectSettleDelayMs = 100;
    private const int AbandonClickSettleDelayMs = 500;
    private const int AbandonSettleMs = 1000;
    private const int AbandonTimeoutSeconds = 15;
    private const int ScreenDrainSettleDelayMs = 100;
    private const int ScreenDrainPumpPollMs = 100;
    private const int MaxScreenDrainsPerCommand = 8;
    private const int MaxSameScreenRepeats = 3;
    private const int MaxEventFinishResumes = 8;
    private static TimeSpan RunStateWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(60));
    private static TimeSpan RoomAssignmentWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(60));
    private static TimeSpan PostBossTransitionWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));
    private static TimeSpan ActTransitionWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(5));
    private static TimeSpan EventProceedWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(5));
    private static TimeSpan RewardsScreenWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));
    private static TimeSpan MainMenuWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(30));
    private static TimeSpan AbandonPopupWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(5));

    /// <summary>Highest ascension the client may request (v0.107.1 cap).</summary>
    private const int MaxAscensionLevel = 10;

    /// <summary>
    /// Runaway guard for the room loop -- NOT the end of a run. A real run
    /// ends by victory or defeat, both of which return from the loop.
    /// RunState.TotalFloor counts *visited map points*
    /// (RunState.cs:423 sums MapPointHistory), so it is path-length
    /// dependent: a longer act-3 route reaches the boss above floor 49.
    /// The game's own AutoSlayer caps at 49 (AutoSlayer.cs:177) and we
    /// copied it; on 2026-08-03 that silently abandoned live Silent and
    /// Regent runs on seed 1004 with map nodes still ahead of the act-3
    /// boss (GH #354). Set high enough that only a genuinely stuck loop
    /// trips it -- 3-4 acts of ~20 rows cannot approach this.
    /// </summary>
    private const int MaxRunFloorGuard = 200;


    private readonly SimConfig _config;
    private readonly Dictionary<RoomType, IRoomHandler> _roomsByType;
    private readonly Dictionary<Type, IScreenHandler> _overlayHandlers;
    private readonly SimMapHandler _mapHandler;

    private CancellationTokenSource? _runCts;
    private IDisposable? _selectorOverride;
    private Watchdog? _watchdog;
    private Rng _rng = new Rng(0u);

    // Set by Stop(); a run that dies afterwards was handed back on purpose.
    private volatile bool _stopRequested;
    // One run_complete per run: set by whichever path reports the run's end.
    private bool _runCompleteSent;
    // The seed the current run actually plays (after run_config negotiation,
    // then read back from the live RunState); logged when the run completes.
    private string _playedSeed = "";
    // debug_player_hp: applied once per run, as soon as the creature exists.
    private bool _hpCheatPending;

    /// <summary>
    /// Applies the debug_player_hp testing cheat once per run, as soon as the
    /// player's Creature exists (it is not constructed yet when the run state
    /// first appears). Setters are public engine API (Creature.cs:488/493).
    /// </summary>
    private void TryApplyHpCheat(RunState runState)
    {
        if (!_hpCheatPending)
            return;
        var creature = LocalContext.GetMe(runState)?.Creature;
        if (creature == null)
            return;
        creature.SetMaxHpInternal(_config.DebugPlayerHp);
        creature.SetCurrentHpInternal(_config.DebugPlayerHp);
        _hpCheatPending = false;
        Logger.Log($"[SimAutoSlayer] debug_player_hp: set player HP to {_config.DebugPlayerHp}/{_config.DebugPlayerHp}");
    }


    /// <summary>
    /// The slayer currently driving a run, so a room/screen handler that is
    /// blocked inside a game command can pump this instance's overlay drain
    /// (see <see cref="AwaitWithScreenDrainAsync"/>) instead of hand-rolling
    /// a second screen dispatcher. Exactly one SimAutoSlayer exists per
    /// process (MainFile.cs:160); null while stopped.
    /// </summary>
    private static SimAutoSlayer? _active;


    /// <summary>True from <see cref="Start"/> until <see cref="Stop"/>.</summary>
    public static bool IsActive { get; private set; }

    /// <summary>
    /// The current run's watchdog (null between runs). Always the same object
    /// as the game's <c>AutoSlayer.CurrentWatchdog</c>; see
    /// <see cref="SetSharedCurrentWatchdog"/>.
    /// </summary>
    public static Watchdog? CurrentWatchdog { get; private set; }

    /// <summary>
    /// Cancellation token of the run currently being played (the per-run
    /// timeout token WaitHelper.WithTimeout hands PlayRunAsync), or
    /// CancellationToken.None between runs. For game-invoked callbacks
    /// (SimCardSelector) that receive no token from the game and must not
    /// outlive their run.
    /// </summary>
    public static CancellationToken CurrentRunToken { get; private set; } = CancellationToken.None;


    public SimAutoSlayer(SimConfig config)
    {
        _config = config;

        // One combat handler instance serves every fight room type.
        var combat = new SimCombatHandler();
        _roomsByType = new Dictionary<RoomType, IRoomHandler>
        {
            [RoomType.Monster] = combat,
            [RoomType.Elite] = combat,
            [RoomType.Boss] = combat,
            [RoomType.Event] = new SimEventRoomHandler(),
            [RoomType.Shop] = new SimShopRoomHandler(),
            [RoomType.Treasure] = new SimTreasureRoomHandler(),
            [RoomType.RestSite] = new SimRestSiteRoomHandler(),
        };

        // Keyed by the overlay node's exact runtime type. The stock deck/card
        // selection handlers are defensive fallbacks only: with SimCardSelector
        // installed those screens should never be pushed.
        IScreenHandler[] overlays =
        {
            new SimRewardsScreenHandler(),
            new SimCardRewardScreenHandler(),
            new DeckUpgradeScreenHandler(),
            new DeckTransformScreenHandler(),
            new DeckEnchantScreenHandler(),
            new DeckCardSelectScreenHandler(),
            new SimpleCardSelectScreenHandler(),
            new ChooseACardScreenHandler(),
            new SimCardBundleScreenHandler(),
            new SimChooseARelicScreenHandler(),
            new SimGameOverScreenHandler(),
            new SimCrystalSphereScreenHandler(),
        };
        _overlayHandlers = overlays.ToDictionary(h => h.ScreenType);

        _mapHandler = new SimMapHandler();
    }

    /// <summary>Arms the slayer and starts the multi-run loop in the background.</summary>
    public void Start()
    {
        _stopRequested = false;
        IsActive = true;
        _active = this;
        SetNonInteractiveCheck(true);
        _runCts = new CancellationTokenSource();
        _ = TaskHelper.RunSafely(RunLoopAsync(_runCts.Token));
    }

    /// <summary>
    /// Hands the game back to the human right away. Never throws: each step
    /// is isolated so one failure cannot leave the game half-automated.
    /// </summary>
    public void Stop()
    {
        _stopRequested = true;
        IsActive = false;
        Interlocked.CompareExchange(ref _active, null, this);

        Guarded("disarm non-interactive mode", () => SetNonInteractiveCheck(false));

        CancellationTokenSource? cts = Interlocked.Exchange(ref _runCts, null);
        if (cts != null)
        {
            Guarded("cancel run loop", cts.Cancel);
            Guarded("dispose run loop token", cts.Dispose);
        }

        // Do not wait for the run task to unwind: while the selector scope is
        // alive every card choice still blocks on the agent.
        ReleaseCardSelector("[SimAutoSlayer] Card selector release on stop failed");

        CurrentRunToken = CancellationToken.None;
        Guarded("clear watchdog", () => PublishWatchdog(null));
        Guarded("stop keepalive", StopWatchdogKeepAlive);
    }

    private static void Guarded(string what, Action step)
    {
        try
        {
            step();
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimAutoSlayer] Stop: could not {what}: {ex.Message}");
        }
    }

    /// <summary>
    /// Set AutoSlayer.IsActive via NonInteractiveMode.AutoSlayerCheck.
    /// The game's own AutoSlayer static constructor wires this up, but since
    /// we're running our own slayer, we set it directly to report our state.
    ///
    /// The active check is EVALUATED LIVE (SimRuntime.AutopilotEngaged), not
    /// latched: a slayer that is armed but still waiting for its agent must
    /// not put the game in non-interactive mode, and taking over with F8 has
    /// to restore interactive play instantly even while a run task is still
    /// unwinding.
    /// </summary>
    private static void SetNonInteractiveCheck(bool active)
    {
        NonInteractiveMode.AutoSlayerCheck =
            active ? () => SimRuntime.AutopilotEngaged : () => false;
    }


    private async Task RunLoopAsync(CancellationToken ct)
    {
        int numRuns = Math.Max(1, _config.NumRuns);
        for (int runIndex = 0; runIndex < numRuns; runIndex++)
        {
            ct.ThrowIfCancellationRequested();
            string seed = _config.Seed ?? SeedHelper.GetRandomSeed();
            _runCompleteSent = false;
            SimGameOverScreenHandler.Observed = false;
            SimGameOverScreenHandler.LastWasVictory = false;
            // With client_run_config, this provisional seed is only a
            // placeholder -- the negotiated client seed replaces it, and
            // PlayMainMenuAsync applies whichever seed wins to
            // NGame.DebugSeedOverride right before the confirm click (any
            // earlier write is clobbered, see NCharacterSelectScreen.cs:1102).
            Logger.Log($"[SimAutoSlayer] Starting run {runIndex + 1}/{numRuns}" +
                (_config.ClientRunConfig ? " (seed: negotiated with client)" : $" with seed: {seed}"));
            await RunOnceAsync(seed, ct);
        }
        Logger.Log("[SimAutoSlayer] All configured runs completed.");
    }


    /// <summary>
    /// Plays one run under the per-run time budget and always leaves the
    /// per-run state torn down with exactly one run_complete sent.
    /// </summary>
    private async Task RunOnceAsync(string provisionalSeed, CancellationToken ct)
    {
        TimeSpan budget = TimeSpan.FromMinutes(Math.Max(1, _config.RunTimeoutMinutes));
        _playedSeed = provisionalSeed;
        try
        {
            await WaitHelper.WithTimeout(runCt => PlayRunAsync(provisionalSeed, runCt), budget, ct);
            Logger.Log($"[SimAutoSlayer] Run completed with seed: {_playedSeed}");
        }
        catch (Exception) when (_stopRequested)
        {
            // The player took over mid-run; leave that run alone.
            Logger.Log("[SimAutoSlayer] Stopped: autopilot handed the game back to the player.");
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimAutoSlayer] Run failed: {ex}");
            await TryRecoverToMainMenuAsync(ct);
        }
        finally
        {
            CurrentRunToken = CancellationToken.None;
            PublishWatchdog(null);
            StopWatchdogKeepAlive();
            ReleaseCardSelector("[SimAutoSlayer] Card selector release after run failed");
            if (!_runCompleteSent)
                ReportRunComplete(NonCombatBridgeProtocol.TerminatedResult);
        }
    }

    /// <summary>
    /// Best-effort recovery after a failed run: if the game is stranded
    /// mid-run (no main menu on screen), drive NGame.ReturnToMainMenu() —
    /// the same fade-out + RunManager.CleanUp() + LoadMainMenu() the game
    /// itself uses after a run — so the next PlayMainMenuAsync finds the
    /// menu instead of dying with "MainMenu not found". Never throws.
    /// </summary>
    private static async Task TryRecoverToMainMenuAsync(CancellationToken ct)
    {
        try
        {
            NGame game = NGame.Instance;
            if (game == null)
            {
                return;
            }
            if (game.MainMenu != null)
            {
                return; // already on the menu; nothing to recover
            }
            Logger.Log("[SimAutoSlayer] Recovering after failed run: returning to main menu");
            await game.ReturnToMainMenu();
            await WaitHelper.Until(() => NGame.Instance?.MainMenu != null, ct,
                SimConfig.Instance.Scaled(TimeSpan.FromSeconds(30)), "Main menu did not load during recovery");
            Logger.Log("[SimAutoSlayer] Recovery complete: main menu loaded");
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimAutoSlayer] Recovery to main menu failed: {ex.Message}");
        }
    }


    // ----------------------------------------------------------------
    // One run
    // ----------------------------------------------------------------

    private async Task PlayRunAsync(string provisionalSeed, CancellationToken ct)
    {
        // Game callbacks without a token of their own (SimCardSelector) bind
        // to this run through it.
        CurrentRunToken = ct;

        await WaitHelper.Until(() => NGame.Instance != null, ct,
            AutoSlayConfig.gameInitTimeout, "NGame.Instance never became available");

        ApplyGamePreferences();
        ApplyTimeScale();
        RevealCharacterEpochs();

        // Handlers receive this Rng per the IHandler contract; the Sim
        // handlers do not draw from it.
        _rng = new Rng((uint)StringHelper.GetDeterministicHashCode(provisionalSeed));

        _selectorOverride = CardSelectCmd.UseSelector(new SimCardSelector());

        var watchdog = new Watchdog();
        PublishWatchdog(watchdog);
        StartWatchdogKeepAlive(watchdog, ct);

        // Settle what to play before touching the menu: the negotiation can
        // block for a long time, and menu nodes may be rebuilt meanwhile.
        string character = _config.Character;
        int ascension = _config.Ascension;
        string seed = provisionalSeed;
        if (_config.ClientRunConfig)
        {
            CurrentWatchdog?.Reset("Negotiating run config with client");
            (string characterId, int chosenAscension, string? chosenSeed) = await NegotiateClientRunConfigAsync(ct);
            character = characterId;
            ascension = chosenAscension;
            if (chosenSeed != null)
            {
                seed = chosenSeed;
                Logger.Log($"[SimAutoSlayer] Client run_config seed: {seed}");
            }
        }
        _playedSeed = seed;

        CurrentWatchdog?.Reset("Playing main menu");
        await PlayMainMenuAsync(character, ascension, seed, ct);

        await WaitHelper.Until(() => CurrentRunState() != null, ct, RunStateWait, "RunState never appeared");
        RunState runState = CurrentRunState()!;
        Logger.Log($"[SimAutoSlayer] RunState available. Floor: {runState.TotalFloor}");
        // The authoritative seed: covers a client start_run without a seed.
        if (!string.IsNullOrEmpty(runState.Rng?.StringSeed))
            _playedSeed = runState.Rng.StringSeed;

        _hpCheatPending = _config.DebugPlayerHp > 0;
        TryApplyHpCheat(runState);

        await WaitHelper.Until(() =>
        {
            AbstractRoom? room = CurrentRunState()?.CurrentRoom;
            if (room == null)
                return false;
            Logger.Log($"[SimAutoSlayer] Waiting for room... type={room.RoomType}");
            return room.RoomType != RoomType.Unassigned;
        }, ct, RoomAssignmentWait, "No room was entered after the run started");

        await PlayRoomsAsync(ct);
    }

    private static RunState? CurrentRunState() => RunManager.Instance.DebugOnlyGetState();

    private void ApplyGamePreferences()
    {
        FastModeType fastMode = (_config.FastMode ?? "").ToLowerInvariant() switch
        {
            "fast" => FastModeType.Fast,
            "normal" => FastModeType.Normal,
            _ => FastModeType.Instant,
        };
        SaveManager.Instance.PrefsSave.FastMode = fastMode;
        SaveManager.Instance.SetFtuesEnabled(enabled: false);
    }

    private void ApplyTimeScale()
    {
        if (Math.Abs(_config.TimeScale - 1.0) <= 1e-9)
            return;
        SimSpeed.Apply(_config.TimeScale, showVfx: false);
        Logger.Log($"[SimAutoSlayer] time_scale: {SimSpeed.Current:0.0}x");
    }

    /// <summary>Makes every character selectable on the character screen.</summary>
    private static void RevealCharacterEpochs()
    {
        string[] epochIds =
        {
            EpochModel.GetId<Silent1Epoch>(),
            EpochModel.GetId<Regent1Epoch>(),
            EpochModel.GetId<Defect1Epoch>(),
            EpochModel.GetId<Necrobinder1Epoch>(),
        };
        foreach (string epochId in epochIds)
            SaveManager.Instance.ObtainEpochOverride(epochId, EpochState.Revealed);
    }

    private static string EnteringActivity(RunState runState, RoomType roomType) =>
        $"Entering {roomType} room (Act {runState.CurrentActIndex + 1}, Floor {runState.ActFloor})";

    private static bool IsFightRoom(RoomType roomType) =>
        roomType is RoomType.Monster or RoomType.Elite or RoomType.Boss;

    /// <summary>
    /// The room state machine. Returns when the run ends by defeat/game over
    /// or by the final-act victory; falls out to an abandon only when the
    /// runaway floor guard trips.
    /// </summary>
    private async Task PlayRoomsAsync(CancellationToken ct)
    {
        // CLEANROOM-Q4: a vanished RunState ends the loop like the floor guard does.
        while (CurrentRunState() is { } runState && runState.TotalFloor < MaxRunFloorGuard)
        {
            ct.ThrowIfCancellationRequested();
            TryApplyHpCheat(runState);

            RoomType roomType = runState.CurrentRoom?.RoomType ?? RoomType.Unassigned;
            CurrentWatchdog?.Reset(EnteringActivity(runState, roomType));
            Logger.Log($"[SimAutoSlayer] Entering {roomType} (Act {runState.CurrentActIndex + 1}, Floor {runState.ActFloor})");

            await PlayRoomAsync(roomType, ct);

            if (IsFightRoom(roomType))
                await AwaitRewardsScreenAsync(ct);
            else
                await Task.Delay(RoomSettleMs, ct);

            await ClearOverlaysAsync(ct);

            // Before the game-over check: a page uncovered by the drain can
            // still start a fight, and a death there must be seen below.
            if (roomType == RoomType.Event)
                await FinishEventRoomAsync(ct);

            if (SimGameOverScreenHandler.Observed)
            {
                CurrentWatchdog?.Reset("Waiting for main menu after defeat");
                await AwaitMainMenuAsync(ct);
                bool won = SimGameOverScreenHandler.LastWasVictory;
                // The victory line is parsed by tools/record_winning_run.ps1 -- keep verbatim.
                Logger.Log(won
                    ? "[SimAutoSlayer] Victory! Run completed and returned to main menu"
                    : "[SimAutoSlayer] Defeat. Run over, returned to main menu");
                ReportRunComplete(won
                    ? NonCombatBridgeProtocol.VictoryResult
                    : NonCombatBridgeProtocol.DefeatResult);
                return;
            }

            if (roomType == RoomType.RestSite)
                await ClickRestSiteProceedIfEnabledAsync();

            if (roomType == RoomType.Boss && IsFirstOfDoubleBoss(CurrentRunState()))
            {
                // Not act-ending: the second boss is the next map point.
                Logger.Log("[SimAutoSlayer] First of two final bosses down; on to the second boss");
            }
            else if (roomType == RoomType.Boss)
            {
                bool runWon = await HandleAfterBossAsync(ct);
                if (runWon)
                    return;
            }

            CurrentWatchdog?.Reset("Navigating map");
            await _mapHandler.HandleAsync(_rng, ct);
        }

        int floor = CurrentRunState()?.TotalFloor ?? -1;
        Logger.Log($"[SimAutoSlayer] ANOMALY: runaway floor guard tripped at TotalFloor={floor} " +
                   $"(>= {MaxRunFloorGuard}) with neither victory nor defeat observed. Abandoning");
        await AbandonRunAsync(ct);
    }

    /// <summary>
    /// True right after the first boss of a double-boss act (Ascension 10's
    /// DoubleBoss level adds a second boss to the final act). The map then
    /// has a SecondBossMapPoint, the only child of BossMapPoint, and the
    /// player is still standing on BossMapPoint: the terminal rewards screen
    /// leads back to the map, not to an act change or the victory event.
    /// Keyed on the map shape rather than the ascension level, so it holds
    /// for any act that carries a second boss point.
    /// </summary>
    private static bool IsFirstOfDoubleBoss(RunState? runState)
    {
        if (runState?.Map?.SecondBossMapPoint == null)
            return false;
        MapCoord? here = runState.CurrentMapCoord;
        return here.HasValue && here.Value == runState.Map.BossMapPoint.coord;
    }

    /// <summary>
    /// After a boss: either the final act's closing event (play it, report
    /// victory, return true) or an ordinary act transition (wait for it,
    /// return false).
    /// </summary>
    private async Task<bool> HandleAfterBossAsync(CancellationToken ct)
    {
        CurrentWatchdog?.Reset("Waiting for act transition after boss");
        RoomType nextType = RoomType.Boss;
        await WaitHelper.Until(() =>
        {
            AbstractRoom? room = CurrentRunState()?.CurrentRoom;
            if (room == null || room.RoomType == RoomType.Boss)
                return false;
            nextType = room.RoomType;
            return true;
        }, ct, PostBossTransitionWait, "Room did not change after the boss");
        Logger.Log($"[SimAutoSlayer] Post-boss transition: room type is now {nextType}");

        RunState? runState = CurrentRunState();
        bool finalAct = runState != null && runState.CurrentActIndex >= runState.Acts.Count - 1;
        if (nextType == RoomType.Event && finalAct)
        {
            CurrentWatchdog?.Reset(EnteringActivity(runState!, RoomType.Event));
            await PlayRoomAsync(RoomType.Event, ct);
            await Task.Delay(RoomSettleMs, ct);
            await ClearOverlaysAsync(ct);

            CurrentWatchdog?.Reset("Waiting for main menu after victory");
            await AwaitMainMenuAsync(ct);
            // Parsed by tools/record_winning_run.ps1 -- keep verbatim.
            Logger.Log("[SimAutoSlayer] Victory! Run completed and returned to main menu");
            ReportRunComplete(NonCombatBridgeProtocol.VictoryResult);
            return true;
        }

        // CLEANROOM-Q1: "wait up to" uses WaitHelper.Until, which throws on timeout.
        await WaitHelper.Until(() => CurrentRunState()?.VisitedMapCoords.Count == 0, ct,
            ActTransitionWait, "Act transition did not complete");
        return false;
    }

    private async Task PlayRoomAsync(RoomType roomType, CancellationToken ct)
    {
        if (!_roomsByType.TryGetValue(roomType, out IRoomHandler? handler))
        {
            Logger.Log($"[SimAutoSlayer] No handler for room type: {roomType}");
            return;
        }
        await WaitHelper.WithTimeout(t => handler.HandleAsync(_rng, t), handler.Timeout, ct);
    }

    private static IOverlayScreen? TopOverlay()
    {
        NOverlayStack? stack = NOverlayStack.Instance;
        return stack != null && stack.ScreenCount > 0 ? stack.Peek() : null;
    }

    private static bool MapIsOpen() => NMapScreen.Instance?.IsOpen ?? false;

    /// <summary>
    /// Services every overlay screen currently stacked, top first, by
    /// handing each to the handler registered for its exact type.
    /// </summary>
    private async Task ClearOverlaysAsync(CancellationToken ct)
    {
        if (NOverlayStack.Instance == null)
        {
            await WaitHelper.Until(() => NOverlayStack.Instance != null, ct,
                SimConfig.Instance.Scaled(AutoSlayConfig.nodeWaitTimeout), "Overlay stack never appeared");
        }

        var handledThisDrain = new HashSet<object>(ReferenceEqualityComparer.Instance);
        int repeatsOfSameScreen = 0;

        while ((NOverlayStack.Instance?.ScreenCount ?? 0) > 0)
        {
            ct.ThrowIfCancellationRequested();

            IOverlayScreen? screen = NOverlayStack.Instance!.Peek();
            if (screen == null)
                break;
            Type screenType = screen.GetType();

            if (handledThisDrain.Add(screen))
            {
                repeatsOfSameScreen = 0;
            }
            else if (++repeatsOfSameScreen >= MaxSameScreenRepeats)
            {
                Logger.Log($"[SimAutoSlayer] Infinite loop: screen {screenType.Name} not closing after {MaxSameScreenRepeats} attempts");
                throw new InvalidOperationException(
                    $"Overlay screen {screenType.Name} did not close after {MaxSameScreenRepeats} handler passes");
            }

            if (!_overlayHandlers.TryGetValue(screenType, out IScreenHandler? handler))
            {
                Logger.Log($"[SimAutoSlayer] No handler for screen type: {screenType.Name}");
                break;
            }

            CurrentWatchdog?.Reset($"Handling screen: {screenType.Name}");
            // Tooling greps NGameOverScreen out of this line -- keep the shape.
            Logger.Log($"[SimAutoSlayer] Handling screen: {screenType.Name}");
            await WaitHelper.WithTimeout(t => handler.HandleAsync(_rng, t), handler.Timeout, ct);

            if (screen is NRewardsScreen && MapIsOpen())
                break;

            await Task.Delay(ScreenDrainSettleDelayMs, ct);
        }
    }

    /// <summary>
    /// Awaits a GAME command (not a UI click) that may open an overlay screen
    /// while it runs, pumping the very same overlay drain the main loop uses
    /// so the screen is answered by the AGENT and the command can finish.
    /// </summary>
    /// <remarks>
    /// Every other place this mod drives the game goes through
    /// <c>UiHelper.Click</c>, which is fire-and-forget (it emits the Released
    /// signal and returns after a 100 ms delay — AutoSlay/Helpers/UiHelper.cs),
    /// so the game's async continuation and our handler run concurrently and a
    /// screen it opens is picked up by <see cref="ClearOverlaysAsync"/>
    /// on the next iteration. A directly awaited command is the opposite: the
    /// handler is parked inside it, so nothing can service the screen it just
    /// pushed, and the screen's completion source is exactly what the command
    /// is waiting on. That is a hard deadlock — the whole mod goes silent and
    /// every later client connect gets nothing (live, Necrobinder A1 sweep,
    /// seed 1001, Act 1 floor 13, 2026-08-03: bought ORRERY in a shop).
    ///
    /// Bounding: no new magic timeout. While the command is quiet we call the
    /// run watchdog's own <c>Check()</c> — the same stall convention
    /// <c>WaitHelper.Until</c>/<c>ForTask</c> use (AutoSlay/Helpers/
    /// WaitHelper.cs) — so a genuinely wedged command dies on the configured
    /// stall limit (<see cref="StartWatchdogKeepAlive"/> keeps feeding the
    /// stock fuse for as long as anything, including SimAgentGate's
    /// heartbeat during a long MCTS think, reports real progress). A screen
    /// we actually drained counts as progress; a runaway that keeps
    /// re-opening screens is capped by
    /// <see cref="MaxScreenDrainsPerCommand"/>.
    /// </remarks>
    internal static async Task AwaitWithScreenDrainAsync(Task work, CancellationToken ct, string context)
    {
        SimAutoSlayer? slayer = _active;
        if (slayer == null)
        {
            // No live slayer (shouldn't happen: handlers only run under one).
            // Better to await plainly than to silently drop the command.
            await work;
            return;
        }

        int drains = 0;
        while (!work.IsCompleted)
        {
            ct.ThrowIfCancellationRequested();

            if ((NOverlayStack.Instance?.ScreenCount ?? 0) > 0)
            {
                if (++drains > MaxScreenDrainsPerCommand)
                {
                    throw new AutoSlayTimeoutException(
                        $"{context} kept opening overlay screens ({MaxScreenDrainsPerCommand} drained) without completing");
                }
                Logger.Log($"[SimAutoSlayer] {context}: overlay screen opened mid-command, draining it inline");
                CurrentWatchdog?.Reset($"Draining screen opened by {context}");
                // Same pre-drain settle the main loop gives a non-combat room
                // (PlayRunAsync's RoomSettleMs before its own
                // ClearOverlaysAsync). A freshly pushed NRewardsScreen
                // builds its reward buttons and then defers UpdateScreenState
                // (NRewardsScreen.cs:468), so draining on the very first frame
                // could see zero reward buttons and have
                // SimRewardsScreenHandler click "proceed" -- which on a
                // non-terminal set means SkipLocalRewardsSet
                // (NRewardsScreen.cs:619), silently throwing the rewards away.
                await Task.Delay(RoomSettleMs, ct);
                await slayer.ClearOverlaysAsync(ct);
                CurrentWatchdog?.Reset($"Drained screen opened by {context}");
                continue;
            }

            CurrentWatchdog?.Check();
            await Task.WhenAny(work, Task.Delay(ScreenDrainPumpPollMs, ct));
        }

        // Rethrows whatever the command threw, exactly as a plain await would.
        await work;
    }


    /// <summary>
    /// Drains every overlay screen stacked now, with the main loop's settle
    /// first, for a room handler that goes on after a command whose screen
    /// outlived it (the shop after a purchase). Nothing stacked: returns.
    /// </summary>
    internal static async Task DrainOverlaysAsync(CancellationToken ct, string context)
    {
        SimAutoSlayer? slayer = _active;
        if (slayer == null || (NOverlayStack.Instance?.ScreenCount ?? 0) == 0)
            return;
        Logger.Log($"[SimAutoSlayer] {context}: overlay screen still up, draining it inline");
        await Task.Delay(RoomSettleMs, ct);
        await slayer.ClearOverlaysAsync(ct);
        CurrentWatchdog?.Reset($"Drained screen left by {context}");
    }

    /// <summary>
    /// Leaves an event room cleanly after the main drain: clicks a final
    /// proceed option, drains screens the event keeps opening, and hands any
    /// new decision page back to the (agent-driven) event handler.
    /// </summary>
    private async Task FinishEventRoomAsync(CancellationToken ct)
    {
        for (int resume = 0; resume < MaxEventFinishResumes; resume++)
        {
            if (SimGameOverScreenHandler.Observed)
                return;

            Node? room = SimRoomNodes.SceneRoot.GetNodeOrNull<Node>(SimRoomNodes.EventRoomPath);
            if (room == null)
                return;

            CurrentWatchdog?.Reset("Finishing event room");
            NEventOptionButton? proceedOption = null;
            // CLEANROOM-Q5: conditions are checked in the listed order (a)..(e);
            // a proceed option is only looked for when no overlay is up.
            await WaitHelper.Until(() =>
            {
                if (!SimRoomNodes.IsLive(room) || MapIsOpen() || SimRoomNodes.OverlayUp())
                    return true;
                List<NEventOptionButton> unlocked = SimRoomNodes.UnlockedEventOptions(room);
                proceedOption = unlocked.FirstOrDefault(b => b.Option.IsProceed);
                return unlocked.Count > 0;
            }, ct, EventProceedWait, "Event room reached no finishable state");

            if (proceedOption != null)
            {
                Logger.Log("[SimAutoSlayer] Clicking event proceed option");
                await UiHelper.Click(proceedOption);
                return;
            }

            if (!SimRoomNodes.IsLive(room) || MapIsOpen())
                return;

            if (SimRoomNodes.OverlayUp())
            {
                await ClearOverlaysAsync(ct);
                continue;
            }

            Logger.Log("[SimAutoSlayer] Event continued past the overlay with new options; re-entering the event handler");
            await PlayRoomAsync(RoomType.Event, ct);
            await Task.Delay(RoomSettleMs, ct);
            await ClearOverlaysAsync(ct);
        }

        Logger.Log($"[SimAutoSlayer] Event room did not finish after {MaxEventFinishResumes} resume attempts");
    }

    private static async Task ClickRestSiteProceedIfEnabledAsync()
    {
        NClickableControl? proceed =
            SimRoomNodes.SceneRoot.GetNodeOrNull<NClickableControl>(SimRoomNodes.RestSiteProceedButtonPath);
        if (proceed == null || !proceed.IsEnabled)
            return;
        Logger.Log("[SimAutoSlayer] Clicking rest site proceed button");
        await UiHelper.Click(proceed);
    }

    private static async Task AwaitRewardsScreenAsync(CancellationToken ct)
    {
        Logger.Log("[SimAutoSlayer] Waiting for rewards screen");
        // A death ends the fight without rewards: the game-over screen counts.
        await WaitHelper.Until(
            () => TopOverlay() is NRewardsScreen or NGameOverScreen || MapIsOpen(),
            ct, RewardsScreenWait, "Neither rewards nor game over appeared after combat");
    }

    private static async Task AwaitMainMenuAsync(CancellationToken ct)
    {
        Logger.Log("[SimAutoSlayer] Waiting for main menu");
        await WaitHelper.Until(
            () => SimRoomNodes.SceneRoot.GetNodeOrNull<Control>(MainMenuNodePath)?.IsVisibleInTree() ?? false,
            ct, MainMenuWait, "Main menu did not come back");
    }

    /// <summary>
    /// Set NEOW_EPOCH to Revealed on the save so runs start with the Neow
    /// blessing. RunManager.SetStartedWithNeowFlag (RunManager.cs:508) only
    /// starts a run at the Neow ancient when the epoch is EXACTLY Revealed;
    /// a save where it sits at Obtained (earned but never clicked on the
    /// Timeline screen) silently opens every run on map_select, amputating
    /// the blessing + floor-0 rewards the AI's run policy was trained with
    /// (native neow_start=true). ObtainEpochOverride is set-or-create
    /// (ProgressState.cs:332), i.e. the same pure data mutation the Timeline
    /// click performs (NEpochSlot.RevealEpoch -> SaveManager.RevealEpoch),
    /// minus the metric upload and UI. One-time: no-op once Revealed.
    /// </summary>
    private static void EnsureNeowEpochRevealed()
    {
        try
        {
            SaveManager save = SaveManager.Instance;
            string epochId = EpochModel.GetId<NeowEpoch>();
            if (save.Progress.IsEpochRevealed(epochId))
                return;
            save.ObtainEpochOverride(epochId, EpochState.Revealed);
            save.SaveProgressFile();
            Logger.Log("[SimAutoSlayer] NEOW_EPOCH revealed on save (was unrevealed) -- runs now start with the Neow blessing");
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimAutoSlayer] EnsureNeowEpochRevealed failed: {ex.Message}, continuing without Neow");
        }
    }


    /// <summary>
    /// Click the main menu's Continue button, resuming the run the last
    /// process left behind. Returns false -- caller falls through to a normal
    /// new run -- when there is nothing to continue, which is the common case
    /// and not an error.
    ///
    /// Character, ascension and seed are NOT applied on this path: the saved
    /// run already fixes all three, and re-driving the ascension panel here
    /// would only rewrite the profile's preferred level for a run that is not
    /// about to start.
    ///
    /// The button's visibility is the game's own answer to "is there a
    /// resumable save" (`NMainMenu.cs:710` toggles visible+enabled together
    /// off `_readRunSaveResult`), so asking it beats stat-ing
    /// `current_run.save` ourselves and guessing whether it parses.
    /// </summary>
    private async Task<bool> TryResumeSavedRunAsync(Control mainMenu, CancellationToken ct)
    {
        try
        {
            NButton? continueBtn = mainMenu.GetNodeOrNull<NButton>(ContinueRunMenuButtonPath);
            if (continueBtn == null || !continueBtn.Visible || !continueBtn.IsEnabled)
            {
                Logger.Log(
                    "[SimAutoSlayer] resume_run: nothing to continue " +
                    $"(button {(continueBtn == null ? "missing" : continueBtn.Visible ? "disabled" : "hidden")}); " +
                    "starting a new run.");
                return false;
            }
            Logger.Log("[SimAutoSlayer] resume_run: continuing the saved run.");
            await UiHelper.Click(continueBtn);
            await Task.Delay(MenuClickSettleMs, ct);
            return true;
        }
        catch (Exception ex)
        {
            // A failed resume must not cost the launch -- fall through and
            // start a new run rather than dying in the menu.
            Logger.Log($"[SimAutoSlayer] resume_run failed ({ex.Message}); starting a new run.");
            return false;
        }
    }


    // ----------------------------------------------------------------
    // Main menu
    // ----------------------------------------------------------------

    /// <summary>
    /// Drives the main menu into a new run with the given character,
    /// ascension and seed (or resumes the saved run when configured to).
    /// Nothing in here waits on the agent, so the node references taken stay
    /// fresh.
    /// </summary>
    private async Task PlayMainMenuAsync(string character, int ascension, string seed, CancellationToken ct)
    {
        Logger.Log("[SimAutoSlayer] Playing main menu");
        Control mainMenu = await WaitHelper.ForNode<Control>(
            SimRoomNodes.SceneRoot, MainMenuNodePath, ct, MainMenuWait);

        EnsureNeowEpochRevealed();

        // Must come before the abandon step, which would delete this save.
        if (_config.ResumeRun && await TryResumeSavedRunAsync(mainMenu, ct))
            return;

        await AbandonLeftoverRunAsync(mainMenu, ct);

        Logger.Log("[SimAutoSlayer] Clicking singleplayer");
        await UiHelper.Click(RequireNode<NClickableControl>(mainMenu, SingleplayerMenuButtonPath));

        // The submenu may not exist yet, so both nodes are looked up afresh
        // on every poll.
        await WaitHelper.Until(
            () => IsShown(mainMenu.GetNodeOrNull<Control>(CharSelectScreenNodePath))
                  || IsShown(mainMenu.GetNodeOrNull<Control>(StandardModeButtonPath)),
            ct, SimConfig.Instance.Scaled(AutoSlayConfig.nodeWaitTimeout), "Neither character select nor standard run appeared");

        // CLEANROOM-Q3: "visible" for menu nodes is read as IsVisibleInTree.
        NClickableControl? standardRun = mainMenu.GetNodeOrNull<NClickableControl>(StandardModeButtonPath);
        if (IsShown(standardRun) && !IsShown(mainMenu.GetNodeOrNull<Control>(CharSelectScreenNodePath)))
        {
            Logger.Log("[SimAutoSlayer] Clicking standard run");
            await UiHelper.Click(standardRun!);
            await WaitHelper.Until(
                () => IsShown(mainMenu.GetNodeOrNull<Control>(CharSelectScreenNodePath)),
                ct, SimConfig.Instance.Scaled(AutoSlayConfig.nodeWaitTimeout), "Character select did not open");
        }

        Control charSelect = RequireNode<Control>(mainMenu, CharSelectScreenNodePath);

        await SelectCharacterAsync(charSelect, character, ct);
        ApplyAscension(charSelect, ascension);

        NClickableControl confirm = await WaitHelper.ForNode<NClickableControl>(
            mainMenu, CharConfirmButtonNodePath, ct);

        // The character screen's singleplayer init wipes DebugSeedOverride,
        // so it is written only now, right before the confirm click.
        Logger.Log($"[SimAutoSlayer] Setting DebugSeedOverride: {seed}");
        NGame.Instance!.DebugSeedOverride = seed;
        Logger.Log("[SimAutoSlayer] Confirming character");
        await UiHelper.Click(confirm);
    }

    private static bool IsShown(Control? control) =>
        control != null && GodotObject.IsInstanceValid(control) && control.IsVisibleInTree();

    private static T RequireNode<T>(Node parent, string path) where T : class
    {
        return parent.GetNodeOrNull<T>(path)
               ?? throw new InvalidOperationException($"Required node '{path}' ({typeof(T).Name}) not found");
    }

    /// <summary>
    /// Best effort: abandons a run left over from an earlier session so the
    /// menu offers a fresh singleplayer start. Never fails the run.
    /// </summary>
    private static async Task AbandonLeftoverRunAsync(Control mainMenu, CancellationToken ct)
    {
        try
        {
            NClickableControl? abandon = mainMenu.GetNodeOrNull<NClickableControl>(MenuAbandonButtonPath);
            if (abandon == null || !abandon.Visible)
                return;

            Logger.Log("[SimAutoSlayer] Abandoning existing run");
            await UiHelper.Click(abandon);
            await Task.Delay(AbandonClickSettleDelayMs, ct);

            try
            {
                await WaitHelper.Until(() => NModalContainer.Instance?.OpenModal != null, ct,
                    AbandonPopupWait, "Abandon confirmation did not open");
                Node? modal = NModalContainer.Instance?.OpenModal as Node;
                NClickableControl? yes = modal?.GetNodeOrNull<NClickableControl>(AbandonPopupYesPrimaryPath)
                                         ?? modal?.GetNodeOrNull<NClickableControl>(AbandonPopupYesFallbackPath);
                if (yes != null)
                {
                    await UiHelper.Click(yes);
                    await Task.Delay(AbandonClickSettleDelayMs, ct);
                }
            }
            // CLEANROOM-Q7: run cancellation is not swallowed by the best-effort catches.
            catch (Exception) when (!ct.IsCancellationRequested)
            {
                Logger.Log("[SimAutoSlayer] Popup not found, trying to continue anyway");
            }

            await Task.Delay(AbandonSettleMs, ct);
        }
        catch (Exception ex) when (!ct.IsCancellationRequested)
        {
            Logger.Log($"[SimAutoSlayer] Could not abandon run: {ex.Message}, continuing...");
        }
    }

    /// <summary>
    /// Picks the requested character: exact id match first, then substring,
    /// then (config fallback only -- never an agent decision) the first
    /// unlocked character.
    /// </summary>
    private static async Task SelectCharacterAsync(Control charSelect, string character, CancellationToken ct)
    {
        Control container = RequireNode<Control>(charSelect, CharButtonContainerNodePath);
        List<NCharacterSelectButton> buttons = UiHelper.FindAll<NCharacterSelectButton>(container);
        foreach (NCharacterSelectButton button in buttons)
            button.UnlockIfPossible();

        List<NCharacterSelectButton> unlocked = buttons.Where(b => !b.IsLocked).ToList();
        NCharacterSelectButton pick =
            unlocked.FirstOrDefault(b => string.Equals(b.Character.Id.Entry, character, StringComparison.OrdinalIgnoreCase))
            ?? unlocked.FirstOrDefault(b => b.Character.Id.Entry.Contains(character, StringComparison.OrdinalIgnoreCase))
            ?? unlocked.FirstOrDefault()
            ?? throw new InvalidOperationException("No selectable character on the character select screen");

        Logger.Log($"[SimAutoSlayer] Selecting character: {pick.Character.Id} (requested \"{character}\")");
        pick.Select();
        await Task.Delay(CharacterSelectSettleDelayMs, ct);
    }

    /// <summary>
    /// Sets the ascension level unconditionally (also for 0): selecting a
    /// character loads the profile's preferred ascension, which would
    /// otherwise silently apply.
    /// </summary>
    private static void ApplyAscension(Control charSelect, int ascension)
    {
        NAscensionPanel? panel = charSelect.GetNodeOrNull<NAscensionPanel>(AscensionPanelUniquePath)
                                 ?? UiHelper.FindFirst<NAscensionPanel>(charSelect);
        if (panel == null)
        {
            Logger.Log("[SimAutoSlayer] Could not find NAscensionPanel; ascension config ignored (runtime risk, see README)");
            return;
        }
        Logger.Log($"[SimAutoSlayer] Setting ascension level to {ascension} (panel was {panel.Ascension})");
        panel.SetAscensionLevel(ascension);
    }

    /// <summary>
    /// Phase 1b client-driven run start (config.client_run_config). Sends a
    /// "run_config" message (available characters from ModelDb.AllCharacters
    /// -- deliberately UI-free: this method blocks indefinitely (never-guess
    /// policy, see SimAgentGate.cs) for a client "start_run" action, and any
    /// scene-tree reference held across that wait dies to Godot node
    /// disposal -- max_ascension, and this mod's config values as
    /// "defaults"). Every rejection (unknown character, out-of-range
    /// ascension, malformed seed) is a structured error + re-send of the
    /// same run_config message, never a silent fallback. Returns the chosen
    /// character's id entry; the caller resolves it to a live UI button
    /// during (non-blocking) menu navigation.
    /// </summary>
    private async Task<(string characterId, int ascension, string? seed)> NegotiateClientRunConfigAsync(
        CancellationToken ct)
    {
        List<CharacterModel> allCharacters = ModelDb.AllCharacters.ToList();
        var characters = allCharacters.Select(c => new Dictionary<string, object>
        {
            ["id"] = c.Id.Entry,
            ["name"] = c.Title.GetFormattedText(),
        }).ToList();

        var defaults = new Dictionary<string, object?>
        {
            ["character"] = _config.Character,
            ["ascension"] = _config.Ascension,
            ["seed"] = _config.Seed,
        };

        while (true)
        {
            var msg = new Dictionary<string, object>
            {
                ["type"] = NonCombatBridgeProtocol.RunConfigState,
                ["characters"] = characters,
                ["max_ascension"] = MaxAscensionLevel,
                ["defaults"] = defaults,
            };
            // No active run yet -- ApplyEnvelope degrades gracefully to
            // defaults/empty collections for a null runState/player.
            SimStateExtractor.ApplyEnvelope(msg, null, null);
            long stateId = (long)msg["state_id"];
            string stateJson = JsonSerializer.Serialize(msg);

            Logger.Log("[SimAutoSlayer] client_run_config: waiting for agent \"start_run\"...");
            string responseJson = await SimAgentGate.RequestActionAsync(stateJson, ct, "run_config");

            if (!SimProtocolHelpers.TryParse(responseJson, null, null, out JsonElement root))
                continue;
            if (!SimProtocolHelpers.CheckStateId(root, stateId, null, null))
                continue;

            string action = root.TryGetProperty("action", out var actionProp) ? actionProp.GetString() ?? "" : "";
            if (action != NonCombatBridgeProtocol.StartRunAction)
            {
                SimProtocolHelpers.SendError("unknown_action", $"Expected \"start_run\", got \"{action}\"", null, null);
                continue;
            }

            string? characterId = root.TryGetProperty("character", out var cp) ? cp.GetString() : null;
            CharacterModel? matched = characterId == null ? null :
                allCharacters.FirstOrDefault(c => c.Id.Entry.Equals(characterId, StringComparison.OrdinalIgnoreCase))
                ?? allCharacters.FirstOrDefault(c => c.Id.Entry.Contains(characterId, StringComparison.OrdinalIgnoreCase));
            if (matched == null)
            {
                SimProtocolHelpers.SendError("invalid_character",
                    $"\"{characterId}\" is not one of the available characters", null, null);
                continue;
            }

            int ascension = 0;
            if (root.TryGetProperty("ascension", out var ap) && ap.ValueKind == JsonValueKind.Number)
            {
                ascension = ap.GetInt32();
            }
            if (ascension < 0 || ascension > MaxAscensionLevel)
            {
                SimProtocolHelpers.SendError("invalid_ascension",
                    $"ascension {ascension} out of range [0,{MaxAscensionLevel}]", null, null);
                continue;
            }

            string? seed = null;
            if (root.TryGetProperty("seed", out var sp) &&
                sp.ValueKind != JsonValueKind.Null && sp.ValueKind != JsonValueKind.Undefined)
            {
                if (sp.ValueKind == JsonValueKind.Number && sp.TryGetInt64(out long seedLong))
                {
                    seed = seedLong.ToString(CultureInfo.InvariantCulture);
                }
                else
                {
                    SimProtocolHelpers.SendError("malformed_action", "\"seed\" must be an integer or null", null, null);
                    continue;
                }
            }
            seed ??= SeedHelper.GetRandomSeed();

            Logger.Log($"[SimAutoSlayer] Client chose character={matched.Id.Entry} ascension={ascension} seed={seed}");
            return (matched.Id.Entry, ascension, seed);
        }
    }


    /// <summary>
    /// Ends a run that tripped the runaway floor guard. Drives the MODEL,
    /// not the pause menu: the UI route the game's own AutoSlayer uses
    /// (AutoSlayer.cs:448) clicks
    /// /root/.../Run/GlobalUi/TopBar/RightAlignedStuff/Options, which does
    /// not exist in v0.107.1 -- the pause button is the unique-named
    /// %PauseButton owned by NTopBar (NTopBar.cs:287) -- so it always threw
    /// "Node ... of type NButton not found" and turned an abandon into a
    /// failed run. RunManager.Abandon() is exactly what the confirm popup
    /// invokes on Yes (NAbandonRunConfirmPopup.cs:158). Never throws.
    /// </summary>
    private async Task AbandonRunAsync(CancellationToken ct)
    {
        await Task.Delay(AbandonSettleMs, ct);
        try
        {
            RunManager.Instance.Abandon();
            await WaitHelper.Until(() => RunManager.Instance.IsAbandoned, ct,
                SimConfig.Instance.Scaled(TimeSpan.FromSeconds(AbandonTimeoutSeconds)), "Run did not abandon");
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimAutoSlayer] Abandon via RunManager failed: {ex.Message}");
        }
        // Abandon() kills the players asynchronously and may leave a game-over
        // overlay up; force the menu so the next run's PlayMainMenuAsync finds it.
        await TryRecoverToMainMenuAsync(ct);
    }


    /// <summary>
    /// The fire-and-forget <c>run_complete</c> notification (no request_id).
    /// The run may already be torn down, so the envelope tolerates nulls.
    /// </summary>
    private static string RunCompleteState(string result)
    {
        RunState? runState = RunManager.Instance.DebugOnlyGetState();
        var message = new Dictionary<string, object>
        {
            [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.RunCompleteState,
            [NonCombatBridgeProtocol.ResultField] = result,
        };
        SimStateExtractor.ApplyEnvelope(message, runState, LocalContext.GetMe(runState));
        return JsonSerializer.Serialize(message);
    }


    private static readonly PropertyInfo? GameCurrentWatchdogProperty =
        typeof(AutoSlayer).GetProperty(nameof(AutoSlayer.CurrentWatchdog), BindingFlags.Public | BindingFlags.Static);

    /// <summary>
    /// Points the game's <c>AutoSlayer.CurrentWatchdog</c> at our watchdog.
    /// Every WaitHelper poll checks the GAME's watchdog, so it must be the
    /// same object we reset. The setter is private, hence reflection.
    /// Never throws.
    /// </summary>
    private static void SetSharedCurrentWatchdog(Watchdog? watchdog)
    {
        try
        {
            if (GameCurrentWatchdogProperty == null)
            {
                Logger.Log("[SimAutoSlayer] AutoSlayer.CurrentWatchdog not found; game waits will not see our watchdog");
                return;
            }
            GameCurrentWatchdogProperty.SetValue(null, watchdog);
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimAutoSlayer] Could not set AutoSlayer.CurrentWatchdog: {ex.Message}");
        }
    }


    private const string KeepAliveActivity = "SpireMentor stall-guard keepalive";

    private CancellationTokenSource? _keepAliveCts;

    /// <summary>
    /// Replaces the game's hard-coded 30-second watchdog fuse with the
    /// configurable <see cref="SimConfig.StallTimeoutSeconds"/> stall limit.
    /// `AutoSlayConfig.watchdogTimeout` is `static readonly` (not patchable),
    /// and per-wait-site Reset calls keep missing windows: the agent-gate
    /// heartbeat covers decision waits, but the 2026-08-02 live sweep still
    /// logged near-misses up to 25.2 s during game-over screen handling and
    /// 6-11 s inside combat settling — one slow effect queue away from a
    /// spurious kill. Instead of chasing every site, this loop polls the
    /// watchdog's own private progress pair (time + activity) every 5 s: any
    /// change made by anyone else counts as REAL progress, and while real
    /// progress happened within the stall limit the loop feeds the stock
    /// fuse. When real progress stops, the loop stops feeding, and the stock
    /// fuse aborts the run within stall limit + 30 s — a true wedge still
    /// dies, just on our clock.
    /// </summary>
    private void StartWatchdogKeepAlive(Watchdog watchdog, CancellationToken runCt)
    {
        _keepAliveCts?.Cancel();
        _keepAliveCts?.Dispose();
        _keepAliveCts = CancellationTokenSource.CreateLinkedTokenSource(runCt);
        CancellationToken ct = _keepAliveCts.Token;

        FieldInfo? progressField = typeof(Watchdog).GetField(
            "_lastProgressTime", BindingFlags.NonPublic | BindingFlags.Instance);
        FieldInfo? activityField = typeof(Watchdog).GetField(
            "_lastActivity", BindingFlags.NonPublic | BindingFlags.Instance);
        if (progressField == null || activityField == null)
        {
            Logger.Log("[SimAutoSlayer] Watchdog keepalive disabled: private fields " +
                       "not found (game update?). The stock 30s fuse applies.");
            return;
        }

        TimeSpan stallLimit = TimeSpan.FromSeconds(
            Math.Max(30, SimConfig.Instance.StallTimeoutSeconds));
        Logger.Log($"[SimAutoSlayer] Watchdog keepalive armed: stall limit " +
                   $"{stallLimit.TotalSeconds:0}s (stock fuse 30s remains the backstop).");

        _ = Task.Run(async () =>
        {
            DateTime lastRealProgress = DateTime.UtcNow;
            DateTime lastSeenTime = DateTime.MinValue;
            string lastSeenActivity = "";
            while (!ct.IsCancellationRequested)
            {
                try
                {
                    var t = (DateTime)progressField.GetValue(watchdog)!;
                    var activity = (string)activityField.GetValue(watchdog)!;
                    if (activity != KeepAliveActivity
                        && (t != lastSeenTime || activity != lastSeenActivity))
                    {
                        lastRealProgress = DateTime.UtcNow;
                        lastSeenTime = t;
                        lastSeenActivity = activity;
                    }
                    if (DateTime.UtcNow - lastRealProgress < stallLimit)
                    {
                        watchdog.Reset(KeepAliveActivity);
                    }
                }
                catch (Exception ex)
                {
                    Logger.Log($"[SimAutoSlayer] Watchdog keepalive error: {ex.Message}");
                }
                try
                {
                    await Task.Delay(TimeSpan.FromSeconds(5), ct).ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    return;
                }
            }
        }, ct);
    }

    // ----------------------------------------------------------------
    // Per-run plumbing
    // ----------------------------------------------------------------

    private void ReportRunComplete(string result)
    {
        BridgeServer.Instance.SendState(RunCompleteState(result));
        _runCompleteSent = true;
    }

    /// <summary>Points ours and the game's current watchdog at the same object.</summary>
    private void PublishWatchdog(Watchdog? watchdog)
    {
        _watchdog = watchdog;
        CurrentWatchdog = watchdog;
        SetSharedCurrentWatchdog(watchdog);
    }

    private void StopWatchdogKeepAlive()
    {
        CancellationTokenSource? keepAlive = Interlocked.Exchange(ref _keepAliveCts, null);
        if (keepAlive == null)
            return;
        try
        {
            keepAlive.Cancel();
        }
        finally
        {
            keepAlive.Dispose();
        }
    }

    private void ReleaseCardSelector(string failurePrefix)
    {
        IDisposable? scope = Interlocked.Exchange(ref _selectorOverride, null);
        if (scope == null)
            return;
        try
        {
            scope.Dispose();
        }
        catch (Exception ex)
        {
            Logger.Log($"{failurePrefix}: {ex.Message}");
        }
    }
}

/// <summary>
/// Game-over overlay. Records the outcome for the room loop, notifies the
/// client with <c>game_over</c> (always before the run's
/// <c>run_complete</c>), then clicks through the summary to the main menu.
/// </summary>
public class SimGameOverScreenHandler : IScreenHandler, IHandler
{
    private static TimeSpan ContinueEnableWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(30));
    private static TimeSpan SummaryWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(90));
    private static TimeSpan CloseWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(30));
    private const int WatchdogRefreshEveryPolls = 20;

    /// <summary>A game-over screen was handled during the current run.</summary>
    public static volatile bool Observed;

    /// <summary>The handled game over was a win.</summary>
    public static volatile bool LastWasVictory;

    public Type ScreenType => typeof(NGameOverScreen);

    public TimeSpan Timeout => SimConfig.Instance.Scaled(TimeSpan.FromMinutes(2));

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        Logger.Log("[SimGameOver] Game over screen appeared");
        NGameOverScreen screen = AutoSlayer.GetCurrentScreen<NGameOverScreen>();

        bool victory = ReadVictory();
        Observed = true;
        LastWasVictory = victory;
        NotifyGameOver(victory);

        NGameOverContinueButton? continueButton = UiHelper.FindFirst<NGameOverContinueButton>(screen);
        if (continueButton == null)
        {
            Logger.Log("[SimGameOver] Continue button not found");
            return;
        }
        await WaitHelper.Until(() => continueButton.IsEnabled, ct, ContinueEnableWait,
            "Game over continue button never enabled");
        await UiHelper.Click(continueButton);

        int polls = 0;
        await WaitHelper.Until(() =>
        {
            if (++polls % WatchdogRefreshEveryPolls == 0)
                SimAutoSlayer.CurrentWatchdog?.Reset("Waiting for game over summary animation");
            return IsGone(screen) || MainMenuButtonReady(screen) != null;
        }, ct, SummaryWait, "Game over summary never offered the main menu button");

        if (IsGone(screen))
            return;

        NReturnToMainMenuButton toMenu = MainMenuButtonReady(screen)
            ?? throw new InvalidOperationException("Game over main menu button disappeared");
        await UiHelper.Click(toMenu);
        await WaitHelper.Until(() => IsGone(screen), ct, CloseWait, "Game over screen did not close");
    }

    private static bool ReadVictory()
    {
        try
        {
            return RunManager.Instance.DebugOnlyGetState()?.CurrentRoom?.IsVictoryRoom ?? false;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimGameOver] Could not determine victory state: {ex.Message}");
            return false;
        }
    }

    /// <summary>Sends the fire-and-forget <c>game_over</c> notification.</summary>
    private static void NotifyGameOver(bool victory)
    {
        RunState? runState = RunManager.Instance.DebugOnlyGetState();
        var message = new Dictionary<string, object>
        {
            [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.GameOverState,
            [NonCombatBridgeProtocol.ResultField] = victory
                ? NonCombatBridgeProtocol.VictoryResult
                : NonCombatBridgeProtocol.DefeatResult,
            [NonCombatBridgeProtocol.MessageField] = NonCombatBridgeProtocol.GameOverMessage,
        };
        SimStateExtractor.ApplyEnvelope(message, runState, LocalContext.GetMe(runState));
        BridgeServer.Instance.SendState(JsonSerializer.Serialize(message));
    }

    private static bool IsGone(NGameOverScreen screen) =>
        !GodotObject.IsInstanceValid(screen) || !screen.IsVisibleInTree();

    private static NReturnToMainMenuButton? MainMenuButtonReady(NGameOverScreen screen)
    {
        NReturnToMainMenuButton? button = UiHelper.FindFirst<NReturnToMainMenuButton>(screen);
        return button != null && button.IsVisibleInTree() && button.IsEnabled ? button : null;
    }
}
