// SimCrystalSphereScreenHandler.cs -- Crystal Sphere minigame, decided by the agent.
// Originally ported from zhiyue/sts2-rl-agent (STS2BridgeMod); rewritten clean-room.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Godot;
using MegaCrit.Sts2.Core.AutoSlay;
using MegaCrit.Sts2.Core.AutoSlay.Handlers;
using MegaCrit.Sts2.Core.AutoSlay.Helpers;
using MegaCrit.Sts2.Core.Context;
using MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent;
using MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereItems;
using MegaCrit.Sts2.Core.Nodes.Events;
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
            CrystalSphereMinigame? game = Minigame(screen);
            List<NCrystalSphereCell> cells = HiddenCells(screen);
            bool proceedEnabled = proceed != null && proceed.IsEnabled;
            if (game != null && game.IsFinished)
            {
                // No divination left: nothing to decide (the Headless host's minigame ends
                // here too). Its rewards came and went on top; leave once proceed is up.
                cells.Clear();
                if (proceedEnabled)
                {
                    await ProceedAndCloseAsync(screen, proceed!, ct);
                    AutoSlayLog.ExitScreen(ScreenName);
                    return;
                }
            }

            if (cells.Count == 0 && !proceedEnabled)
            {
                // A reveal is animating: nothing to decide, nothing to send.
                await WaitForOutcomeAsync(screen, ct);
                continue;
            }

            CrystalSphereChoice choice = await ChooseCrystalSphereOption(screen, game, cells, proceedEnabled, ct);
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
                    if (choice.Tool is { } tool && game != null && game.CrystalSphereTool != tool)
                    {
                        // The screen's own tool button (SetBigDivination / SetSmallDivination).
                        NButton? button = ToolButton(screen, tool);
                        AutoSlayLog.Info($"[SimCrystalSphere] Selecting the {tool} tool");
                        if (button != null)
                            button.EmitSignal(NClickableControl.SignalName.Released, button);
                        else
                            game.SetTool(tool);
                    }
                    AutoSlayLog.Info($"[SimCrystalSphere] Divining cell ({cell.Entity.X}, {cell.Entity.Y})");
                    cell.EmitSignal(NClickableControl.SignalName.Released, cell);
                    await Task.Delay(CellClickSettleMs, ct);
                    break;
            }
        }

        AutoSlayLog.ExitScreen(ScreenName);
    }

    private static readonly FieldInfo? EntityField =
        typeof(NCrystalSphereScreen).GetField("_entity", BindingFlags.Instance | BindingFlags.NonPublic);
    private static readonly FieldInfo? BigButtonField =
        typeof(NCrystalSphereScreen).GetField("_bigDivinationButton", BindingFlags.Instance | BindingFlags.NonPublic);
    private static readonly FieldInfo? SmallButtonField =
        typeof(NCrystalSphereScreen).GetField("_smallDivinationButton", BindingFlags.Instance | BindingFlags.NonPublic);
    private static readonly FieldInfo? CardRewardRarityField =
        typeof(CrystalSphereCardReward).GetField("_rarity", BindingFlags.Instance | BindingFlags.NonPublic);

    /// <summary>The screen's minigame (its private <c>_entity</c>), or null if it can't be read.</summary>
    private static CrystalSphereMinigame? Minigame(NCrystalSphereScreen screen)
    {
        try { return EntityField?.GetValue(screen) as CrystalSphereMinigame; }
        catch { return null; }
    }

    private static NButton? ToolButton(NCrystalSphereScreen screen, CrystalSphereMinigame.CrystalSphereToolType tool)
    {
        FieldInfo? field = tool == CrystalSphereMinigame.CrystalSphereToolType.Big ? BigButtonField : SmallButtonField;
        try { return field?.GetValue(screen) as NButton; }
        catch { return null; }
    }

    private static string ToolName(CrystalSphereMinigame.CrystalSphereToolType tool) =>
        tool == CrystalSphereMinigame.CrystalSphereToolType.Small ? "small" : "big";

    /// <summary>
    /// What the screen shows of the grid, as the Headless host sends it
    /// (RunSimulator.CrystalSphere.cs, CrystalSphereState): its size, the cleared cells
    /// (the corners the game clears first included), the divinations left, the tool
    /// selected, and each item with a cleared cell (kind, footprint, whether all of it is
    /// clear; a card reward's rarity once a cell of its top row, where the banner is, is
    /// clear). Items under the fog are left out.
    /// </summary>
    internal static Dictionary<string, object?> Grid(CrystalSphereMinigame game)
    {
        Vector2I size = game.GridSize;
        var cleared = new List<List<int>>();
        for (int y = 0; y < size.Y; y++)
            for (int x = 0; x < size.X; x++)
                if (!game.cells[x, y].IsHidden) cleared.Add(new List<int> { x, y });

        var items = new List<Dictionary<string, object?>>();
        foreach (CrystalSphereItem item in game.Items)
        {
            int seen = 0, total = 0;
            for (int dx = 0; dx < item.Size.X; dx++)
                for (int dy = 0; dy < item.Size.Y; dy++)
                {
                    total++;
                    if (!game.cells[item.Position.X + dx, item.Position.Y + dy].IsHidden) seen++;
                }
            if (seen == 0) continue;
            bool bannerSeen = Enumerable.Range(0, item.Size.X)
                .Any(dx => !game.cells[item.Position.X + dx, item.Position.Y].IsHidden);
            items.Add(new Dictionary<string, object?>
            {
                ["kind"] = ItemKind(item),
                ["rarity"] = item is CrystalSphereCardReward && bannerSeen
                    ? CardRewardRarityField?.GetValue(item)?.ToString()?.ToLowerInvariant()
                    : null,
                ["x"] = item.Position.X,
                ["y"] = item.Position.Y,
                ["width"] = item.Size.X,
                ["height"] = item.Size.Y,
                ["revealed"] = seen == total,
            });
        }
        return new Dictionary<string, object?>
        {
            ["width"] = size.X,
            ["height"] = size.Y,
            ["cleared"] = cleared,
            ["divinations_left"] = game.DivinationCount,
            ["tool"] = ToolName(game.CrystalSphereTool),
            // By position, not in the order the game generated them (which the screen doesn't show).
            ["items"] = items.OrderBy(i => (int)i["y"]!).ThenBy(i => (int)i["x"]!).ToList(),
        };
    }

    private static string ItemKind(CrystalSphereItem item) => item switch
    {
        CrystalSphereCardReward => "card_reward",
        CrystalSphereCurse => "curse",
        CrystalSphereGold => "gold",
        CrystalSpherePotion => "potion",
        CrystalSphereRelic => "relic",
        _ => item.GetType().Name,
    };

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
        NCrystalSphereScreen screen, CrystalSphereMinigame? game, List<NCrystalSphereCell> cells, bool proceedEnabled,
        CancellationToken ct)
    {
        if (cells.Count == 0 && !proceedEnabled)
        {
            return CrystalSphereChoice.Wait;
        }

        bool proceedOffered = false;
        string? chosenTool = null;  // the accepted reply's `tool`; none keeps the screen's

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
                var body = new Dictionary<string, object>
                {
                    [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.CrystalSphereState,
                    ["options"] = options,
                };
                if (game != null)
                {
                    body["grid"] = Grid(game);
                }
                return body;
            },
            cells.Count + 1, NonCombatBridgeProtocol.CrystalSphereState, "SimCrystalSphere", ct,
            rejectAtReply: i => i == cells.Count && !(proceedOffered && ProceedEnabled(screen))
                ? $"index {i} (proceed) is not enabled"
                : null,
            rejectReply: (i, reply) =>
            {
                chosenTool = null;
                if (!reply.TryGetProperty("tool", out JsonElement t))
                    return null;
                if (i == cells.Count)
                    return ("invalid_tool", "proceed takes no tool");
                if (game == null)
                    return ("invalid_tool", "the tool can't be set here (the minigame is unreadable)");
                string? name = t.ValueKind == JsonValueKind.String ? t.GetString() : null;
                if (name is not ("big" or "small"))
                    return ("invalid_tool", $"tool must be big or small, got {t}");
                chosenTool = name;
                return null;
            });

        if (index == cells.Count)
            return CrystalSphereChoice.Proceed;
        CrystalSphereMinigame.CrystalSphereToolType? tool = null;
        if (chosenTool == "small") tool = CrystalSphereMinigame.CrystalSphereToolType.Small;
        else if (chosenTool == "big") tool = CrystalSphereMinigame.CrystalSphereToolType.Big;
        return CrystalSphereChoice.Click(cells[index], tool);
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

    private readonly record struct CrystalSphereChoice(
        CrystalSphereChoiceKind Kind, NCrystalSphereCell? Cell, CrystalSphereMinigame.CrystalSphereToolType? Tool = null)
    {
        public static CrystalSphereChoice Proceed => new(CrystalSphereChoiceKind.Proceed, null);

        public static CrystalSphereChoice Wait => new(CrystalSphereChoiceKind.Wait, null);

        public static CrystalSphereChoice Click(NCrystalSphereCell cell, CrystalSphereMinigame.CrystalSphereToolType? tool) =>
            new(CrystalSphereChoiceKind.Cell, cell, tool);
    }
}
