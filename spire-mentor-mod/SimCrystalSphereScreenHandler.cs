// SimCrystalSphereScreenHandler.cs -- Crystal Sphere minigame, decided by the agent.
// Originally ported from zhiyue/sts2-rl-agent (STS2BridgeMod); rewritten clean-room.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Godot;
using MegaCrit.Sts2.Core.AutoSlay;
using MegaCrit.Sts2.Core.AutoSlay.Handlers;
using MegaCrit.Sts2.Core.AutoSlay.Helpers;
using MegaCrit.Sts2.Core.Context;
using MegaCrit.Sts2.Core.Nodes.CommonUi;
using MegaCrit.Sts2.Core.Nodes.Events.Custom.CrystalSphere;
using MegaCrit.Sts2.Core.Nodes.GodotExtensions;
using MegaCrit.Sts2.Core.Nodes.Screens.Map;
using MegaCrit.Sts2.Core.Nodes.Screens.Overlays;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

public class SimCrystalSphereScreenHandler : IScreenHandler, IHandler
{
    private const string ScreenName = nameof(NCrystalSphereScreen);
    private const string ProceedButtonPath = "%ProceedButton";
    private const string CellsPath = "%Cells";
    private const int OpeningSettleMs = 1000;
    private const int CellClickSettleMs = 500;
    private static TimeSpan OutcomeWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(15));
    private static TimeSpan ProceedCloseWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));
    private const int OverlayRemovalSettleMs = 100;

    public Type ScreenType => typeof(NCrystalSphereScreen);

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        AutoSlayLog.EnterScreen(ScreenName);
        NCrystalSphereScreen screen = AutoSlayer.GetCurrentScreen<NCrystalSphereScreen>();
        await Task.Delay(OpeningSettleMs, ct);

        while (IsShowing(screen))
        {
            ct.ThrowIfCancellationRequested();

            if (AnotherScreenOnTop(screen))
            {
                // E.g. a rewards screen the sphere opened; the drain comes back here later.
                AutoSlayLog.Info("[SimCrystalSphere] Another screen is on top; returning to the drain");
                AutoSlayLog.ExitScreen(ScreenName);
                return;
            }

            NProceedButton? proceed = screen.GetNodeOrNull<NProceedButton>(ProceedButtonPath);
            List<NCrystalSphereCell> cells = HiddenCells(screen);
            bool proceedEnabled = proceed != null && proceed.IsEnabled;

            if (cells.Count == 0 && !proceedEnabled)
            {
                // A reveal is animating: nothing to decide, nothing to send.
                await WaitForOutcomeAsync(screen, ct);
                continue;
            }

            CrystalSphereChoice choice = await ChooseCrystalSphereOption(screen, cells, proceedEnabled, ct);
            switch (choice.Kind)
            {
                case CrystalSphereChoiceKind.Proceed:
                    // Re-read: proceed was validated as enabled when the reply arrived.
                    proceed = screen.GetNodeOrNull<NProceedButton>(ProceedButtonPath);
                    if (proceed != null)
                        await ProceedAndCloseAsync(screen, proceed, ct);
                    AutoSlayLog.ExitScreen(ScreenName);
                    return;

                case CrystalSphereChoiceKind.Wait:
                    await WaitForOutcomeAsync(screen, ct);
                    break;

                case CrystalSphereChoiceKind.Cell:
                    NCrystalSphereCell cell = choice.Cell!;
                    AutoSlayLog.Info($"[SimCrystalSphere] Divining cell ({cell.Entity.X}, {cell.Entity.Y})");
                    cell.EmitSignal(NClickableControl.SignalName.Released, cell);
                    await Task.Delay(CellClickSettleMs, ct);
                    break;
            }
        }

        AutoSlayLog.ExitScreen(ScreenName);
    }

    private static bool IsShowing(NCrystalSphereScreen screen) =>
        GodotObject.IsInstanceValid(screen) && screen.IsVisibleInTree();

    private static bool AnotherScreenOnTop(NCrystalSphereScreen screen)
    {
        NOverlayStack? stack = NOverlayStack.Instance;
        return stack != null && stack.ScreenCount > 0 && !ReferenceEquals(stack.Peek(), screen);
    }

    /// <summary>Visible cells still hiding their content (empty if %Cells is missing).</summary>
    private static List<NCrystalSphereCell> HiddenCells(NCrystalSphereScreen screen)
    {
        Control? container = screen.GetNodeOrNull<Control>(CellsPath);
        if (container == null)
            return new List<NCrystalSphereCell>();
        return UiHelper.FindAll<NCrystalSphereCell>(container)
            .Where(c => c.Visible && c.Entity.IsHidden)
            .ToList();
    }

    /// <summary>Waits for a reveal to settle: screen gone, proceed enabled, or a screen pushed on top.</summary>
    private static Task WaitForOutcomeAsync(NCrystalSphereScreen screen, CancellationToken ct) =>
        WaitHelper.Until(() =>
            !IsShowing(screen) ||
            (screen.GetNodeOrNull<NProceedButton>(ProceedButtonPath)?.IsEnabled ?? false) ||
            AnotherScreenOnTop(screen),
            ct, OutcomeWait, "Crystal Sphere showed no outcome");

    private static async Task ProceedAndCloseAsync(NCrystalSphereScreen screen, NProceedButton proceed, CancellationToken ct)
    {
        AutoSlayLog.Action("Crystal Sphere: clicking proceed");
        await UiHelper.Click(proceed);
        await WaitHelper.Until(() => !IsShowing(screen) || MapVisible(), ct, ProceedCloseWait,
            "Crystal Sphere did not close after proceed");

        if (IsShowing(screen) && MapVisible())
        {
            // The map is already up but the sphere never popped itself.
            AutoSlayLog.Info("[SimCrystalSphere] Map is visible but the sphere is still up; removing it");
            NOverlayStack.Instance?.Remove(screen);
            await Task.Delay(OverlayRemovalSettleMs, ct);
        }
    }

    private static bool MapVisible()
    {
        NMapScreen? map = NMapScreen.Instance;
        return map != null && GodotObject.IsInstanceValid(map) && map.IsVisibleInTree();
    }

    /// <summary>
    /// One option per hidden cell plus a trailing "proceed" when enabled.
    /// Nothing offered means "wait" without asking. Proceed is re-checked for
    /// every ask and again when the reply arrives.
    /// </summary>
    private static async Task<CrystalSphereChoice> ChooseCrystalSphereOption(
        NCrystalSphereScreen screen, List<NCrystalSphereCell> cells, bool proceedEnabled, CancellationToken ct)
    {
        if (cells.Count == 0 && !proceedEnabled)
        {
            return CrystalSphereChoice.Wait;
        }

        bool proceedOffered = false;

        int index = await SimAgentChoice.AskAsync(
            (_, _) =>
            {
                List<Dictionary<string, object>> options = cells.Select((c, i) => CellOption(c, i)).ToList();
                proceedOffered = ProceedEnabled(screen);
                if (proceedOffered)
                {
                    options.Add(new Dictionary<string, object>
                    {
                        ["index"] = cells.Count,
                        ["action"] = NonCombatBridgeProtocol.ProceedAction,
                        ["enabled"] = true,
                    });
                }
                return new Dictionary<string, object>
                {
                    [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.CrystalSphereState,
                    ["options"] = options,
                };
            },
            cells.Count + 1, NonCombatBridgeProtocol.CrystalSphereState, "SimCrystalSphere", ct,
            rejectAtReply: i => i == cells.Count && !(proceedOffered && ProceedEnabled(screen))
                ? $"index {i} (proceed) is not enabled"
                : null);

        return index < cells.Count ? CrystalSphereChoice.Click(cells[index]) : CrystalSphereChoice.Proceed;
    }

    private static bool ProceedEnabled(NCrystalSphereScreen screen)
    {
        if (!GodotObject.IsInstanceValid(screen))
            return false;
        NProceedButton? proceed = screen.GetNodeOrNull<NProceedButton>(ProceedButtonPath);
        return proceed != null && proceed.IsEnabled;
    }

    private static Dictionary<string, object> CellOption(NCrystalSphereCell cell, int index) => new()
    {
        ["index"] = index,
        ["action"] = NonCombatBridgeProtocol.DivineCellAction,
        ["x"] = cell.Entity.X,
        ["y"] = cell.Entity.Y,
        ["enabled"] = true,
    };

    private enum CrystalSphereChoiceKind
    {
        Proceed,
        Cell,
        Wait,
    }

    private readonly record struct CrystalSphereChoice(CrystalSphereChoiceKind Kind, NCrystalSphereCell? Cell)
    {
        public static CrystalSphereChoice Proceed => new(CrystalSphereChoiceKind.Proceed, null);

        public static CrystalSphereChoice Wait => new(CrystalSphereChoiceKind.Wait, null);

        public static CrystalSphereChoice Click(NCrystalSphereCell cell) => new(CrystalSphereChoiceKind.Cell, cell);
    }
}
