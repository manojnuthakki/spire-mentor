// MainFile.cs -- SpireMentor mod entry point, Harmony patches and the shared logger.
// Originally ported from zhiyue/sts2-rl-agent (STS2BridgeMod); rewritten clean-room.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Threading;
using System.Threading.Tasks;
using Godot;
using HarmonyLib;
using MegaCrit.Sts2.Core.AutoSlay.Helpers;
using MegaCrit.Sts2.Core.Commands;
using MegaCrit.Sts2.Core.Helpers;
using MegaCrit.Sts2.Core.Modding;
using MegaCrit.Sts2.Core.Nodes;
using MegaCrit.Sts2.Core.Nodes.Vfx.Utilities;

namespace SpireMentor;

[ModInitializer(nameof(Initialize))]
public static class MainFile
{
    public const string ModId = "SpireMentor";
    // Keep in sync with SpireMentor.json "version" (package.ps1 reads that one).
    public const string ModVersion = "0.5.2";

    private static Harmony? _patcher;
    private static SimAutoSlayer? _autoSlayer;

    private static readonly TimeSpan GameStartupWait = TimeSpan.FromSeconds(60);
    private const string MainMenuNodePath = "/root/Game/RootSceneContainer/MainMenu";

    /// <summary>
    /// Mod entry point, called by the game's mod loader on the main thread.
    /// Each phase is isolated so one failing component never blocks the rest.
    /// </summary>
    public static void Initialize()
    {
        Logger.Log($"=== {ModId} v{ModVersion} initializing ===");

        SimConfig config = SimConfig.Instance;
        SimRuntime.InitializeFrom(config);
        Logger.Log($"Startup mode: {SimRuntime.Mode} (auto_start={config.AutoStart})");

        try
        {
            _patcher = new Harmony(ModId);

            ApplyPatch(typeof(IsReleaseGamePatch));
            ApplyPatch(typeof(CardSelectContextPatch));
            ApplyPatch(typeof(CardSelectContextNoPrefsPatch));
            ShieldBaseLibDamagePreview();
            ApplyPatch(typeof(HitStopTimeScalePatch));
            ApplyPatch(typeof(GameQuitPatch));
            SimReduceEffects.Install(_patcher);

            try
            {
                SimSnapshots.Install(_patcher);
            }
            catch (Exception ex)
            {
                Logger.Log($"  SKIP: SimSnapshots - {ex.Message}");
            }

            if (config.SpeedPatches)
            {
                ApplyPatch(typeof(WaitSpeedPatch));
                ApplyPatch(typeof(AnimationSpeedPatch));
                ApplyPatch(typeof(CardTrailOffPatch));
                ApplyPatch(typeof(HandCardSnapPatch));
                ApplyPatch(typeof(PunchOffIdleLoopPatch));
            }
            else
            {
                Logger.Log("Speed patches disabled by config (speed_patches=false)");
            }
        }
        catch (Exception ex)
        {
            Logger.Log($"Harmony patching failed: {ex}");
        }

        try
        {
            BridgeServer.Instance.Start(config.Port);
            Logger.Log($"TCP bridge started on port {config.Port}");
        }
        catch (Exception ex)
        {
            Logger.Log($"TCP bridge failed to start: {ex.Message}");
        }

        try
        {
            SpireMentorModConfig.Register();
        }
        catch (Exception ex)
        {
            Logger.Log($"Settings page registration failed (non-fatal): {ex.Message}");
        }

        TaskHelper.RunSafely(SimVfxCap.InstallWhenReadyAsync());
        TaskHelper.RunSafely(SimHotkeys.InstallWhenReadyAsync());
        TaskHelper.RunSafely(SimToolbar.Instance.InstallWhenReadyAsync());
        TaskHelper.RunSafely(SimHelp.ShowBootHintWhenReadyAsync());

        SimAdvisor.Instance.Prime();

        if (SimRuntime.Mode == SimMode.Autopilot)
        {
            TaskHelper.RunSafely(LaunchSimAutoSlayAsync(config));
        }
        else
        {
            Logger.Log("Auto-start is off; autopilot not launched.");
        }

        if (config.AdvisorAutoplay)
        {
            SimAdvisorAutoplay.ArmWhenRunStarts();
        }
        else if (SimRuntime.Mode == SimMode.Off && SpireMentorModConfig.CoachOnAtLaunch &&
                 OperatingSystem.IsWindows())
        {
            // Enabling the mod in the game's mod list should be the only step;
            // a second "now switch the coach on" confused players. Autopilot
            // (auto_start, every sweep) and the advisor_autoplay harness never
            // reach this branch. Windows only: elsewhere the bundled advisor
            // cannot start (SimAdvisorProcess) and the toolbar would just read
            // "crashed".
            TaskHelper.RunSafely(CoachOnWhenReadyAsync());
        }

        // Backstop only: GameQuitPatch is the graceful shutdown path.
        AppDomain.CurrentDomain.ProcessExit += (_, _) => SimShutdown.Run("ProcessExit");

        Logger.Log($"=== {ModId} ready ===");
    }

    /// <summary>
    /// Stop the autopilot session so a human can take the controls, and
    /// report whether there was one to stop. Called by every "leave Autopilot"
    /// entry point (F8, `spirementor mode`, the settings page).
    ///
    /// Autopilot is armed by SpireMentor.conf's persistent `auto_start=true`,
    /// which the sweep lane needs -- so refusing to leave it (the old
    /// behavior) meant any human session started after a sweep was stuck with
    /// the AI's game modifiers and no way out.
    /// </summary>
    public static bool StopAutopilot()
    {
        SimAutoSlayer? slayer = _autoSlayer;
        _autoSlayer = null;
        if (slayer == null)
        {
            return false;
        }
        try
        {
            slayer.Stop();
            Logger.Log("[SpireMentor] Autopilot stopped by user; game handed back to the player.");
            return true;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SpireMentor] Autopilot stop failed: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Stop the autopilot session without the "handed back to the player"
    /// framing -- the player is leaving, not taking over. Used by
    /// <see cref="SimShutdown"/>.
    /// </summary>
    internal static void StopAutopilotQuietly()
    {
        SimAutoSlayer? slayer = _autoSlayer;
        _autoSlayer = null;
        slayer?.Stop();
    }

    /// <summary>
    /// Stop a BaseLib v3.4.1 NullReferenceException in
    /// <c>ModifyBaseDamagePatches.AdjustBaseUnenchanted</c> from taking the
    /// combat down with it.
    ///
    /// It throws while building a card's DAMAGE PREVIEW -- the number drawn on
    /// the card face -- but it is reached from <c>NCard.UpdateVisuals</c>
    /// inside <c>CardPileCmd.Add</c>, so the exception faults the pile-add
    /// task. The hand is then half-built: the new card cannot be played
    /// (the takeover reads it as "changed nothing"), and ending the turn
    /// rethrows from <c>CombatManager.FlushPlayerHand</c>, so even the
    /// refusal escape hatch cannot get out. Combat wedges until a watchdog
    /// kills the game.
    ///
    /// Silent finds it immediately -- every Shiv added to hand -- while runs
    /// 001-046 on Ironclad and Defect logged it zero times. Run 048 logged it
    /// 616 times in one fight.
    ///
    /// Suppressing it is safe HERE because the only casualty is a preview
    /// number on a card face: the agent reads game state over the bridge and
    /// never parses card text, and nobody is watching an autopilot run. This
    /// is a local shim, not a fix -- the defect is BaseLib's, and a BaseLib
    /// release that fixes it should retire this.
    ///
    /// Patched by name so a BaseLib without the type is a logged skip rather
    /// than a load failure.
    /// </summary>
    private static void ShieldBaseLibDamagePreview()
    {
        const string target = "BaseLib.Patches.Hooks.ModifyBaseDamagePatches:AdjustBaseUnenchanted";
        try
        {
            MethodBase? victim = AccessTools.Method(target);
            if (victim == null)
            {
                Logger.Log($"  SKIP: BaseLib damage-preview shield - {target} not found");
                return;
            }
            _patcher!.Patch(
                victim,
                finalizer: new HarmonyMethod(
                    typeof(MainFile).GetMethod(
                        nameof(SwallowDamagePreviewThrow),
                        BindingFlags.NonPublic | BindingFlags.Static)));
            Logger.Log("  Patched: BaseLib damage-preview shield (AdjustBaseUnenchanted)");
        }
        catch (Exception ex)
        {
            Logger.Log($"  SKIP: BaseLib damage-preview shield - {ex.Message}");
        }
    }

    /// <summary>Harmony finalizer: returning null swallows the exception.
    /// Logged once per session -- 616 copies of the same stack is what the
    /// wedged run looked like, and it buried everything else.</summary>
    private static bool _damagePreviewThrowLogged;

    private static Exception? SwallowDamagePreviewThrow(Exception? __exception)
    {
        if (__exception != null && !_damagePreviewThrowLogged)
        {
            _damagePreviewThrowLogged = true;
            Logger.Log(
                "[SpireMentor] BaseLib AdjustBaseUnenchanted threw " +
                $"({__exception.GetType().Name}); suppressed so the card pile add can " +
                "finish. Card damage PREVIEWS may be wrong from here; game state is " +
                "unaffected. Further occurrences are silent.");
        }
        return null;
    }

    /// <summary>Applies one [HarmonyPatch] class; a failure is logged as a skip.</summary>
    private static void ApplyPatch(Type patchType)
    {
        try
        {
            _patcher!.CreateClassProcessor(patchType).Patch();
            Logger.Log($"  patch applied: {patchType.Name}");
        }
        catch (Exception ex)
        {
            Logger.Log($"  patch skipped: {patchType.Name} ({ex.Message})");
        }
    }

    /// <summary>
    /// Switch the coach on once the game node exists -- through the same
    /// Mode setter as F8, so the advisor subscribes, its process starts and
    /// every surface refreshes. Runs before the main menu shows, so the boot
    /// hint (SimHelp) already sees the coach on. Skips if anything changed
    /// the mode in the meantime.
    /// </summary>
    private static async Task CoachOnWhenReadyAsync()
    {
        using var cts = new CancellationTokenSource(GameStartupWait);
        await WaitHelper.Until(() => NGame.Instance != null, cts.Token, GameStartupWait,
            "NGame.Instance did not appear");
        Callable.From(() =>
        {
            if (SimRuntime.Mode != SimMode.Off)
            {
                return;
            }
            Logger.Log("Coach on at launch (settings: \"Turn the coach on when the game starts\").");
            SimRuntime.Mode = SimMode.Advisor;
        }).CallDeferred();
    }

    /// <summary>
    /// Waits for the game node and a visible main menu, then starts the
    /// autopilot. Timeouts propagate to TaskHelper.RunSafely (no retry).
    /// </summary>
    private static async Task LaunchSimAutoSlayAsync(SimConfig config)
    {
        TimeSpan wait = config.StartupWait;
        Logger.Log($"Autopilot: waiting for the game and the main menu (up to {wait.TotalSeconds:0} s)...");
        using var startupCts = new CancellationTokenSource(wait);
        CancellationToken ct = startupCts.Token;

        await WaitHelper.Until(() => NGame.Instance != null, ct, wait,
            "NGame.Instance did not appear");

        await WaitHelper.Until(() =>
        {
            Window root = ((SceneTree)Engine.GetMainLoop()).Root;
            return root.GetNodeOrNull<Control>(MainMenuNodePath) is Control menu && menu.IsVisibleInTree();
        }, ct, wait, "Main menu did not become visible");

        _autoSlayer = new SimAutoSlayer(config);
        _autoSlayer.Start();
    }
}

// ---------------------------------------------------------------------------
// Harmony Patches
// ---------------------------------------------------------------------------

/// <summary>
/// Reports the game as a non-release build while autopilot is engaged.
/// IsReleaseGame()==false unlocks the NonInteractiveMode/AutoSlay code paths
/// the autopilot relies on, but it also loosens NClickableControl's
/// SceneTree.Paused guard and exposes debug affordances -- so a human-played
/// game must keep seeing the vanilla answer.
/// </summary>
[HarmonyPatch(typeof(NGame), nameof(NGame.IsReleaseGame))]
public static class IsReleaseGamePatch
{
    [HarmonyPrefix]
    static bool Prefix(ref bool __result)
    {
        if (!SimRuntime.AutopilotEngaged)
        {
            return true; // run original
        }
        __result = false;
        return false;
    }
}

/// <summary>
/// Scales the game's fast/standard wait durations while speedups are engaged.
/// Harmony binds prefix arguments by name, hence the exact parameter names.
/// </summary>
[HarmonyPatch(typeof(Cmd), nameof(Cmd.CustomScaledWait))]
public static class WaitSpeedPatch
{
    [HarmonyPrefix]
    static void Prefix(ref float fastSeconds, ref float standardSeconds)
    {
        if (!SimRuntime.SpeedupsEngaged)
        {
            return;
        }
        float multiplier = SimRuntime.WaitMultiplier;
        fastSeconds *= multiplier;
        standardSeconds *= multiplier;
    }
}

/// <summary>
/// Skip PunchOff's decorative punching loop while the speedups are engaged.
///
/// PunchOff.PunchEachOther is an unbounded `while (!_punchCts.IsCancellation-
/// Requested)` that runs until the player picks an option. Its only brake is
/// four awaits, all of which we have removed: SimAutoSlayer sets
/// PrefsSave.FastMode to Instant (SimAutoSlayer.cs:408), and Cmd.Wait returns
/// synchronously in that mode -- no timer, no yield (Cmd.cs:35). The loop
/// becomes a hot spin.
///
/// Live on 2026-08-27 that cost two graded runs of logs/barrier-test2 and
/// every attempt to resume them. Each iteration instantiates a hit-spark
/// scene; the renderer had no elements left, so `particles` came back null
/// and Godot logged ~35 lines plus a C# backtrace PER iteration, at ~1000
/// iterations a second -- 135 MB of log in 80 s. The flood is the visible
/// half. The damaging half is that a spinning async loop never yields the
/// main thread, so the advisor could not pick the option that cancels
/// `_punchCts`, and nothing but killing the process ended it.
///
/// The loop is purely cosmetic: it triggers Attack/Hit animations, plays a
/// blunt-impact VFX and mirrors one creature's scale. Options, encounter and
/// rewards are all generated elsewhere. Skipping it removes the spin at the
/// source rather than trying to out-run it.
///
/// Gated on whether Cmd.Wait will actually yield, NOT on SpeedupsEngaged.
/// That distinction cost a verification run. SpeedupsEngaged requires
/// AdvisorAutoplayArmed, and autoplay arms on a delay after launch, while a
/// RESUMED run enters its room almost immediately -- so AfterEventStarted
/// fired with the gate still false, the original loop ran, and by the time
/// autoplay armed the spin already owned the main thread. The condition that
/// actually decides whether this loop is dangerous is the one inside Cmd.Wait
/// (Cmd.cs:35): NonInteractiveMode.IsActive or FastMode == Instant means the
/// awaits return synchronously and the loop cannot pace itself. Gate on the
/// hazard, not on a flag that happens to correlate with it.
///
/// A human at normal speed still gets the animation, because for them Cmd.Wait
/// really does wait.
/// </summary>
[HarmonyPatch(typeof(MegaCrit.Sts2.Core.Models.Events.PunchOff), "PunchEachOther")]
public static class PunchOffIdleLoopPatch
{
    [HarmonyPrefix]
    static bool Prefix(ref System.Threading.Tasks.Task __result)
    {
        if (!LoopWouldSpin())
        {
            return true; // run original
        }
        Logger.Log("[SpireMentor] PunchOff: skipping the idle punch loop (waits do not yield).");
        __result = System.Threading.Tasks.Task.CompletedTask;
        return false;
    }

    /// <summary>Would PunchEachOther's awaits return without yielding? Mirrors
    /// the guard in Cmd.Wait. Anything unreadable answers "yes": skipping a
    /// decorative animation is always cheaper than risking the wedge.</summary>
    private static bool LoopWouldSpin()
    {
        try
        {
            if (MegaCrit.Sts2.Core.Helpers.NonInteractiveMode.IsActive)
            {
                return true;
            }
            return MegaCrit.Sts2.Core.Saves.SaveManager.Instance.PrefsSave.FastMode ==
                   MegaCrit.Sts2.Core.Settings.FastModeType.Instant;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SpireMentor] PunchOff gate unreadable, skipping the loop: {ex.Message}");
            return true;
        }
    }
}

/// <summary>
/// Scales Spine animation playback while speedups are engaged. The prefix
/// parameter must be named <c>scale</c> to match the original signature.
/// </summary>
[HarmonyPatch(typeof(MegaCrit.Sts2.Core.Bindings.MegaSpine.MegaAnimationState),
    nameof(MegaCrit.Sts2.Core.Bindings.MegaSpine.MegaAnimationState.SetTimeScale))]
public static class AnimationSpeedPatch
{
    [HarmonyPrefix]
    static void Prefix(ref float scale)
    {
        if (SimRuntime.SpeedupsEngaged)
        {
            scale *= SimRuntime.AnimMultiplier;
        }
    }
}

/// <summary>
/// Skip the card-trail VFX (<c>NCardTrail._Process</c>) while speedups are
/// engaged.
///
/// At sped-up animation rates a card can move a large distance between two
/// rendered frames, and <c>NCardTrail.CreatePoint</c> back-fills the gap
/// with one interpolated point every 48 px in an UNBOUNDED loop
/// (NCardTrail.cs:144). The first unattended advisor-autoplay run at
/// anim x5 + TimeScale 4 died to exactly that:
/// <c>System.OutOfMemoryException: Array dimensions exceeded supported
/// range</c> out of <c>List&lt;T&gt;.AddWithResize</c> under
/// <c>NCardTrail.CreatePoint</c> (2026-08-10 smoke run, act 2), which means
/// the computed hop was effectively infinite. The trail is pure eye candy;
/// with no audience it is dropped entirely rather than bounded. The prefix
/// leaves the node's state untouched, so an attended session (patch
/// inactive) resumes normally.
/// </summary>
[HarmonyPatch(typeof(MegaCrit.Sts2.Core.Nodes.Vfx.NCardTrail), "_Process")]
public static class CardTrailOffPatch
{
    [HarmonyPrefix]
    static bool Prefix()
    {
        return !SimRuntime.SpeedupsEngaged; // false = skip original
    }
}

/// <summary>
/// Snap hand cards to their target transform instead of gliding, while
/// speedups are engaged.
///
/// The hand's card motion is NOT a tween and NOT a Spine animation, so
/// neither WaitSpeedPatch nor AnimationSpeedPatch touches it.
/// <c>NHandCardHolder</c> runs three async per-frame lerp loops
/// (<c>AnimPosition</c>/<c>AnimAngle</c>/<c>AnimScale</c>, whose own comment
/// says "This is NOT a timer-based tween") that ease toward the target at a
/// hardcoded rate — <c>delta * 7f</c> for position, 10f angle, 8f scale
/// (NHandCardHolder.cs:557/578/600).
///
/// `delta` scales with <c>Engine.TimeScale</c>, so at time_scale 4 the glide
/// is 4x faster — but WaitMultiplier 0.1 makes the pacing BETWEEN actions
/// ~40x faster. That 10:1 mismatch is visible as cards being flung onto the
/// screen faster than they can land: each new draw restarts the lerp while
/// the previous cards are still in flight, so a partially-drawn card sits
/// over the board most of the time and the run becomes unwatchable (reported
/// 2026-08-10 during the first A8 run at the new budgets).
///
/// Fix the mismatch at the cheap end: with no audience the glide conveys
/// nothing, so postfix the three public target setters to apply the value
/// immediately. Angle and scale reuse the game's own
/// <c>SetAngleInstantly</c>/<c>SetScaleInstantly</c> (which cancel the
/// in-flight loop); position has no such helper, so it assigns directly —
/// the loop then finds itself already at the target and exits through its
/// own snap threshold. Nothing is skipped and no private state is touched,
/// so an attended session (patch inactive) animates exactly as before.
/// </summary>
[HarmonyPatch(typeof(MegaCrit.Sts2.Core.Nodes.Cards.Holders.NHandCardHolder))]
public static class HandCardSnapPatch
{
    [HarmonyPostfix]
    [HarmonyPatch(nameof(MegaCrit.Sts2.Core.Nodes.Cards.Holders.NHandCardHolder.SetTargetPosition))]
    static void PositionPostfix(
        MegaCrit.Sts2.Core.Nodes.Cards.Holders.NHandCardHolder __instance,
        Vector2 position)
    {
        if (!SimRuntime.SpeedupsEngaged)
        {
            return;
        }
        ((Control)(object)__instance).Position = position;
    }

    [HarmonyPostfix]
    [HarmonyPatch(nameof(MegaCrit.Sts2.Core.Nodes.Cards.Holders.NHandCardHolder.SetTargetAngle))]
    static void AnglePostfix(
        MegaCrit.Sts2.Core.Nodes.Cards.Holders.NHandCardHolder __instance,
        float angle)
    {
        if (!SimRuntime.SpeedupsEngaged)
        {
            return;
        }
        __instance.SetAngleInstantly(angle);
    }

    [HarmonyPostfix]
    [HarmonyPatch(nameof(MegaCrit.Sts2.Core.Nodes.Cards.Holders.NHandCardHolder.SetTargetScale))]
    static void ScalePostfix(
        MegaCrit.Sts2.Core.Nodes.Cards.Holders.NHandCardHolder __instance,
        Vector2 scale)
    {
        if (!SimRuntime.SpeedupsEngaged)
        {
            return;
        }
        __instance.SetScaleInstantly(scale);
    }
}

/// <summary>
/// Keep the game's hit-stop VFX from cancelling the chosen game speed.
///
/// <c>NHitStop.HitStopTask</c> (v0.107.1) drives <c>Engine.TimeScale</c> to
/// 0.1, ramps it back, and finishes with a hardcoded <c>SetTimeScale(1f)</c>
/// -- an ABSOLUTE write that knows nothing about the player's slider, the
/// F9/F10 hotkeys or <c>time_scale</c> in SpireMentor.conf. One hit-stop
/// therefore reset the whole game to 1.0x for the rest of the session:
/// reported 2026-08-08 as "4.0x in act 1 was 1.0x in act 2", which is just
/// where the first hit-stop monster showed up (CeremonialBeast, MechaKnight
/// and Vantom are the only v0.107.1 callers of <c>NGame.DoHitStop</c>).
///
/// The sweep lane was silently exposed to the same thing: an autopilot run
/// with <c>time_scale: 4.0</c> lost its speed-up on the first Vantom.
///
/// Rewriting the write as a MULTIPLIER of <see cref="SimSpeed.Desired"/>
/// keeps the effect (a fifth of current speed, ramping back) and lands on
/// the intended speed instead of 1.0. At the default 1.0x the prefix hands
/// straight back to the original, so an untouched game is untouched code.
/// </summary>
[HarmonyPatch(typeof(NHitStop), "SetTimeScale")]
public static class HitStopTimeScalePatch
{
    [HarmonyPrefix]
    static bool Prefix(float timeScale)
    {
        double desired = SimSpeed.Desired;
        if (Math.Abs(desired - 1.0) < 1e-9)
        {
            return true; // run original
        }
        Engine.TimeScale = timeScale * desired;
        return false;
    }
}

/// <summary>
/// Shut the mod down on the game's own quit path.
///
/// <c>NGame.Quit</c> is what the main menu's Quit button calls
/// (NMainMenu.cs:959) and it ends in <c>GetTree().Quit(0)</c>. The
/// <c>AppDomain.ProcessExit</c> hook that used to be the only shutdown path
/// does not reliably run when Godot tears the runtime down, which left the
/// advisor child process alive -- and Steam keeps reporting the GAME as
/// running for as long as any process in its tree is (reported 2026-08-08:
/// "quit the game, Steam still shows it running, had to force-terminate").
/// This is the graceful path; <see cref="SimJobObject"/> is the guarantee
/// for the paths that never reach any managed code at all (Alt+F4, Steam's
/// stop button, a crash).
/// </summary>
[HarmonyPatch(typeof(NGame), nameof(NGame.Quit))]
public static class GameQuitPatch
{
    [HarmonyPrefix]
    static void Prefix() => SimShutdown.Run("NGame.Quit");
}

/// <summary>
/// Latest CardSelectCmd entry-point context, captured just before the game
/// calls ICardSelector.GetSelectedCards (whose signature drops it). Written
/// by the two patches below; consumed-and-cleared by SimCardSelector when it
/// builds the card_select message. Game logic is single-threaded per
/// selection, and nested CardSelectCmd calls (e.g. FromDeckForRemoval ->
/// FromDeckGeneric) share one prompt, so last-write-wins is correct for the
/// semantic PromptKey; Source is best-effort (innermost entry point).
/// </summary>
public static class SimCardSelectContext
{
    public static string? Source;
    public static string? PromptKey;
    public static string? SourceModel;

    /// <summary>
    /// `CardSelectorPrefs.Cancelable` (CardSelectorPrefs.cs:31) / the
    /// `canSkip` argument, captured verbatim instead of inferred by the
    /// client from `MinSelect &lt;= 0`.
    ///
    /// That inference is wrong in both directions, and the headless DLL
    /// driver already carries the truth for this reason
    /// (RunSimulator.cs HeadlessCardSelectContext.Cancelable). The bite here
    /// is `FromChooseACardScreen`: it calls the selector as
    /// `GetSelectedCards(cards, 0, 1)` — min 0 unconditionally
    /// (CardSelectCmd.cs:230) — while defaulting `canSkip: false`. Ten of its
    /// twelve v0.107.1 callers pass `canSkip: true` explicitly; the two that
    /// take the default (Toolbox, KnowledgeDemon) are MANDATORY and were
    /// being offered to the agent as skippable.
    /// </summary>
    public static bool? Cancelable;

    /// <summary>
    /// What KIND of thing <see cref="SourceModel"/> names -- "card",
    /// "potion", "relic", "power", "affliction", "enchantment" -- taken from
    /// the model's runtime type.
    ///
    /// <para>Without it the client has to infer the kind from the NAME, and
    /// the only inference available is absence: look the id up in the hand,
    /// then in the other piles, and call it "not a card" when neither has it.
    /// That misfires exactly where it matters -- a real card that has fallen
    /// out of the client's copy of the fight reports as "not a card", hiding
    /// a mirror fault behind a category. `TouchOfInsanity` (a POTION opening
    /// a `FromHand` select with `source: this`) and a genuinely lost card
    /// were indistinguishable.</para>
    ///
    /// <para>The game itself switches on precisely this set of types to work
    /// out who owns a choice -- see `HookPlayerChoiceContext`'s constructor
    /// -- so this is the game's own taxonomy, not one invented here.</para>
    /// </summary>
    public static string? SourceKind;

    /// <summary>
    /// The source card's target as an index into the full `Enemies` list,
    /// when it had one, captured WHILE THE PLAY IS STILL IN FLIGHT.
    ///
    /// <para>`CardModel.CurrentTarget` is set at the top of `OnPlayWrapper`
    /// (CardModel.cs:1872), before `OnPlay` runs, and cleared only when the
    /// play completes (2002) -- so during the selection screen, which opens
    /// inside `OnPlay`, it is populated.</para>
    ///
    /// <para>This is the one fact a client cannot get any other way at
    /// screen time. The play is reported over `human_action` as a
    /// `CardPlayFinishedEntry`, and a card that opens a selection does not
    /// FINISH until the selection is answered -- so that message, target and
    /// all, arrives after the advice request it would have answered. Lacking
    /// it, the advisor replayed the card at every legal target on a clone and
    /// kept whichever reproduced the world's enemy-HP vector; an off-by-any
    /// damage prediction matched nothing and it declined
    /// (`no board matched replay of NEOWS_FURY`, live run
    /// 001-ironclad-a8-0824-2150).</para>
    /// </summary>
    public static int? SourceTargetIndex;

    /// <summary>Whether the source card was upgraded. The other half of the
    /// "which play was it" key: two copies of a card in hand are
    /// interchangeable, but Burning Pact and Burning Pact+ draw different
    /// amounts and neither shows in an enemy-HP comparison.</summary>
    public static bool? SourceUpgraded;

    public static void Set(string source, string? promptKey, string? sourceModel, bool? cancelable)
    {
        Source = source;
        PromptKey = promptKey;
        SourceModel = sourceModel;
        Cancelable = cancelable;
    }

    /// <summary>Record the source model's own detail alongside its name.
    /// Separate from <see cref="Set"/> so the four fields that describe the
    /// SCREEN keep their shape and their three call sites; these three
    /// describe the THING THAT OPENED IT.</summary>
    public static void SetSourceDetail(object? sourceModel)
    {
        SourceKind = KindOf(sourceModel);
        SourceTargetIndex = null;
        SourceUpgraded = null;
        if (sourceModel is not MegaCrit.Sts2.Core.Models.CardModel card)
        {
            return;
        }
        try
        {
            SourceUpgraded = card.CurrentUpgradeLevel > 0;
            MegaCrit.Sts2.Core.Entities.Creatures.Creature? target = card.CurrentTarget;
            if (target != null)
            {
                // The SAME frame `SimAdvisor.TargetIndexOf` uses: the full
                // `Enemies` list, not the hittable-only one, so a corpse keeps
                // its slot and the index stays stable across a kill.
                System.Collections.Generic.List<
                    MegaCrit.Sts2.Core.Entities.Creatures.Creature>? enemies =
                    card.CombatState?.Enemies?.ToList();
                int idx = enemies?.IndexOf(target) ?? -1;
                SourceTargetIndex = idx >= 0 ? idx : null;
            }
        }
        catch { /* capture-only: never break the game's selection flow */ }
    }

    /// <summary>The model's kind, by runtime type. Null for an unrecognized
    /// type, which the client must treat as "unknown", never as "not a
    /// card".</summary>
    private static string? KindOf(object? m) => m switch
    {
        null => null,
        MegaCrit.Sts2.Core.Models.CardModel => "card",
        MegaCrit.Sts2.Core.Models.PotionModel => "potion",
        MegaCrit.Sts2.Core.Models.RelicModel => "relic",
        MegaCrit.Sts2.Core.Models.PowerModel => "power",
        MegaCrit.Sts2.Core.Models.AfflictionModel => "affliction",
        MegaCrit.Sts2.Core.Models.EnchantmentModel => "enchantment",
        MegaCrit.Sts2.Core.Models.AbstractModel => "model",
        _ => null,
    };

    /// <summary>
    /// Non-destructive read for advisor mode: no ICardSelector is installed
    /// there, so nothing consumes the capture and the screen the human is
    /// looking at is still described by the latest one.
    /// </summary>
    public static (string? source, string? promptKey, string? sourceModel, bool? cancelable)
        Peek() => (Source, PromptKey, SourceModel, Cancelable);

    public static (string? source, string? promptKey, string? sourceModel, bool? cancelable)
        Consume()
    {
        (string?, string?, string?, bool?) v = (Source, PromptKey, SourceModel, Cancelable);
        Source = null;
        PromptKey = null;
        SourceModel = null;
        Cancelable = null;
        SourceKind = null;
        SourceTargetIndex = null;
        SourceUpgraded = null;
        return v;
    }

    /// <summary>
    /// The `AbstractModel source` argument — "Model that kicked off the
    /// selection" (CardSelectCmd.cs:697). Located by scanning `__args` rather
    /// than bound by name, because only three overloads declare it
    /// (`FromHand`, `FromHandForDiscard`, `FromHandForUpgrade`) and a named
    /// prefix parameter the target lacks fails the patch outright.
    ///
    /// Not every entry point declares one — `FromCombatPile` and
    /// `FromChooseACardScreen` do not — so a null here falls through to
    /// <see cref="ContextModelEntryFrom"/> rather than reaching the client
    /// unattributed.
    /// </summary>
    private static object? ModelFrom(object?[]? args)
    {
        if (args == null) return null;
        foreach (object? a in args)
        {
            if (a is MegaCrit.Sts2.Core.Models.AbstractModel m)
            {
                return m;
            }
        }
        return ContextModelFrom(args);
    }

    /// <summary>
    /// Fallback for the entry points that declare no `source`: ask the choice
    /// itself.
    ///
    /// `PlayerChoiceContext.LastInvolvedModel` (PlayerChoiceContext.cs:20) is
    /// the game's own answer to "which model kicked this off". It exists so
    /// remote players can be shown the right card, and it is maintained on
    /// every path that reaches a selection: `CardModel.OnPlayWrapper` pushes
    /// the card before `OnPlay` runs (CardModel.cs:1870), and
    /// `HookPlayerChoiceContext` pushes its `Source` in the constructor
    /// (HookPlayerChoiceContext.cs:87), so a power-driven selection
    /// (StratagemPower, ForegoneConclusionPower) names the power.
    ///
    /// This is what unblocks the COMBAT PILE screens. `FromCombatPile` is
    /// `(PlayerChoiceContext, CardPile, Player, CardSelectorPrefs)`
    /// (CardSelectCmd.cs:376) — no `AbstractModel` in the signature at all,
    /// so the scan above returned null and every Headbutt / Dredge / Seance /
    /// Secret Technique / Hologram selection reached the client unattributed.
    /// The advisor needs the name to replay the opening play and park its
    /// mirror on the choice; without it, it cannot search and falls back to a
    /// hand-written rarity sort. Following the played card cannot substitute:
    /// `CardPlayFinishedEntry` fires only once the selection is ANSWERED, so
    /// the report arrives after the advice request (PROTOCOL.md section 7).
    ///
    /// Deliberately SECOND, not first. Where both exist the declared `source`
    /// wins, because the two disagree on purpose: the model stack tracks the
    /// INNERMOST model (a Sly'd Prepared autoplayed by Survivor pushes
    /// Prepared), while `FromHand`'s `source` is the card that owns the
    /// selection. The `source`-declaring overloads are the ones already
    /// working in the live lane; this only fills the null.
    ///
    /// It also newly names the generative screens (`FromChooseACardScreen`:
    /// Toolbox / Discovery / Quasar / Splash / the four potions), which the
    /// client already has a by-name table for — `_GENERATIVE_SOURCES` in
    /// `combat_mirror.py`. An unrecognized name costs nothing: classification
    /// falls to the shape test that covers those screens today.
    /// </summary>
    private static object? ContextModelFrom(object?[] args)
    {
        foreach (object? a in args)
        {
            if (a is MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext ctx)
            {
                try { return ctx.LastInvolvedModel; } catch { return null; }
            }
        }
        return null;
    }

    /// <summary>The model that opened the selection, or null. Shared by both
    /// capture patches — see <see cref="ModelFrom"/>.</summary>
    public static object? SourceObjectFrom(object?[]? args) => ModelFrom(args);

    /// <summary>That model'"'"'s id entry, for <c>source_model</c> on the wire.</summary>
    public static string? EntryOf(object? m)
    {
        try { return (m as MegaCrit.Sts2.Core.Models.AbstractModel)?.Id.Entry; }
        catch { return null; }
    }
}

/// <summary>
/// Capture-only prefix on every CardSelectCmd entry point that takes a
/// CardSelectorPrefs: records the method name + the prompt's loc key
/// ("TO_UPGRADE" / "TO_DISCARD" / "TO_EXHAUST" / "TO_REMOVE" /
/// "TO_TRANSFORM" / "TO_ENCHANT" / ...) for the card_select message.
/// Harmony binds the `prefs` arg by name; v0.107.1 verified every listed
/// method declares it as `prefs` (CardSelectCmd.cs).
/// </summary>
[HarmonyPatch]
public static class CardSelectContextPatch
{
    static IEnumerable<System.Reflection.MethodBase> TargetMethods()
    {
        Type t = typeof(CardSelectCmd);
        foreach (System.Reflection.MethodInfo m in t.GetMethods(
            System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static))
        {
            bool hasPrefs = false;
            foreach (System.Reflection.ParameterInfo p in m.GetParameters())
            {
                if (p.ParameterType == typeof(MegaCrit.Sts2.Core.CardSelection.CardSelectorPrefs))
                {
                    hasPrefs = true;
                    break;
                }
            }
            if (hasPrefs)
                yield return m;
        }
    }

    [HarmonyPrefix]
    static void Prefix(MegaCrit.Sts2.Core.CardSelection.CardSelectorPrefs prefs,
        object?[]? __args,
        System.Reflection.MethodBase __originalMethod)
    {
        try
        {
            object? source = SimCardSelectContext.SourceObjectFrom(__args);
            SimCardSelectContext.Set(
                __originalMethod.Name,
                prefs.Prompt.LocEntryKey,
                SimCardSelectContext.EntryOf(source),
                prefs.Cancelable);
            SimCardSelectContext.SetSourceDetail(source);
        }
        catch { /* capture-only: never break the game's selection flow */ }
    }
}

/// <summary>
/// Same capture for the two prefs-less entry points: FromHandForUpgrade
/// (Armaments-style, semantically TO_UPGRADE) and FromChooseACardScreen
/// ("choose a card to obtain" screens, e.g. 3-random-powers).
/// </summary>
[HarmonyPatch]
public static class CardSelectContextNoPrefsPatch
{
    static IEnumerable<System.Reflection.MethodBase> TargetMethods()
    {
        yield return AccessTools.Method(typeof(CardSelectCmd), "FromHandForUpgrade");
        yield return AccessTools.Method(typeof(CardSelectCmd), "FromChooseACardScreen");
    }

    [HarmonyPrefix]
    static void Prefix(object?[]? __args, System.Reflection.MethodBase __originalMethod)
    {
        try
        {
            bool isUpgrade = __originalMethod.Name == "FromHandForUpgrade";
            string? key = isUpgrade ? "TO_UPGRADE" : "TO_OBTAIN";
            // `FromChooseACardScreen(context, cards, player, bool canSkip)` —
            // the only bool in its signature, and it defaults to FALSE
            // (CardSelectCmd.cs:216) even though the call reaches the selector
            // with min=0. `FromHandForUpgrade` has no skip at all.
            bool? cancelable = isUpgrade
                ? false
                : (__args?.OfType<bool>().Cast<bool?>().FirstOrDefault() ?? false);
            object? source = SimCardSelectContext.SourceObjectFrom(__args);
            SimCardSelectContext.Set(
                __originalMethod.Name,
                key,
                SimCardSelectContext.EntryOf(source),
                cancelable);
            SimCardSelectContext.SetSourceDetail(source);
        }
        catch { }
    }
}

/// <summary>SpireMentor's logger: every line goes to the Godot log with the
/// "[SpireMentor] " prefix that log-scraping tools filter on.</summary>
internal static class Logger
{
    private const string Prefix = "[SpireMentor] ";

    public static void Log(string message)
    {
        GD.Print(Prefix + message);
    }
}
