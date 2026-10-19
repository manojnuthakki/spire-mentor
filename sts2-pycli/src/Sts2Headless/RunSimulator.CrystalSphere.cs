// sts2-agent fork: the Crystal Sphere minigame, in agent mode.
//
// Upstream plays the minigame itself: the Big tool on a fixed lattice of six centres
// (PlayCrystalSphereHeadless). In agent mode the host returns the minigame as a
// decision instead, with what the screen shows and nothing more:
//
//   crystal_sphere: divine{x, y, tool} (tool "big" or "small")
//
// The decision lists the grid size, the cells cleared so far (the corner cells the
// game clears before placing items included), the divinations left, the tool selected,
// and every item with at least one cleared cell: its kind and footprint, as the item's
// picture shows through the cleared cells (items of one kind always have the same
// shape). Items under the fog are not listed.
//
// A move is the game's own: SetTool (free on the screen, so it rides along with the
// click) and CellClicked, which spends one divination and clears the cell (Small) or
// the cell and its eight neighbours (Big); an item is revealed once all its cells are
// clear, and a curse goes into the deck when revealed. The screen ignores clicks on a
// cleared cell, so `divine` there is refused (`cell_cleared`), as are cells off the
// grid (`invalid_cell`) and unknown tools (`invalid_tool`). After the last divination
// the game's CompleteMinigame offers the revealed items (OfferCustom), which come to
// the client as a `rewards` decision, and the event finishes. With agent mode off none
// of this runs.

using MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent;

namespace Sts2Headless;

public partial class RunSimulator
{
    /// <summary>Agent mode is on for the current run (read by Harmony prefixes).</summary>
    internal static volatile bool AgentModeActive;

    /// <summary>
    /// The minigame waiting for the client's moves (agent mode). Set by the event option
    /// that started it, which then awaits the game's own completion source.
    /// </summary>
    internal static volatile CrystalSphereMinigame? PendingCrystalSphere;

    private static System.Reflection.FieldInfo? CrystalSphereCompletionField;

    private static readonly System.Reflection.FieldInfo? CardRewardRarityField =
        typeof(MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereItems.CrystalSphereCardReward)
            .GetField("_rarity", System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);

    /// <summary>A divination that has not finished (it hung): reported until it does.</summary>
    private Task? _agentSphereClick;

    private static bool CrystalSpherePending =>
        PendingCrystalSphere is { } game && !game.IsFinished;

    /// <summary>
    /// The agent-mode body of PlayMinigame: wait, as the game's screen does, until the
    /// last divination completes the minigame's completion source.
    /// </summary>
    private static async Task WaitForClientCrystalSphere(CrystalSphereMinigame game)
    {
        if (CrystalSphereCompletionField?.GetValue(game) is not TaskCompletionSource done)
        {
            Console.Error.WriteLine("[WARN] CrystalSphere: completion source unresolved - minigame left unplayed");
            EngineErrors.Record("host", "Crystal Sphere: the minigame's completion source could not be resolved");
            return;
        }
        PendingCrystalSphere = game;
        await done.Task;
    }

    private Dictionary<string, object?> CrystalSphereState(CrystalSphereMinigame game, MegaCrit.Sts2.Core.Entities.Players.Player player)
    {
        var size = game.GridSize;
        var cleared = new List<List<int>>();
        for (int y = 0; y < size.Y; y++)
            for (int x = 0; x < size.X; x++)
                if (!game.cells[x, y].IsHidden) cleared.Add(new() { x, y });

        var items = new List<Dictionary<string, object?>>();
        foreach (var item in game.Items)
        {
            int seen = 0, total = 0;
            for (int dx = 0; dx < item.Size.X; dx++)
                for (int dy = 0; dy < item.Size.Y; dy++)
                {
                    total++;
                    if (!game.cells[item.Position.X + dx, item.Position.Y + dy].IsHidden) seen++;
                }
            if (seen == 0) continue;
            items.Add(new()
            {
                ["kind"] = CrystalSphereItemKind(item),
                // The card reward's banner shows its rarity (common, uncommon, rare).
                ["rarity"] = item is MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereItems.CrystalSphereCardReward
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
            ["type"] = "decision",
            ["decision"] = "crystal_sphere",
            ["context"] = RunContext(),
            ["width"] = size.X,
            ["height"] = size.Y,
            ["cleared"] = cleared,
            ["divinations_left"] = game.DivinationCount,
            ["tool"] = game.CrystalSphereTool == CrystalSphereMinigame.CrystalSphereToolType.Small ? "small" : "big",
            // By position, not in the order the game generated them (which the screen doesn't show).
            ["items"] = items.OrderBy(i => (int)i["y"]!).ThenBy(i => (int)i["x"]!).ToList(),
            ["player"] = PlayerSummary(player),
        };
    }

    private static string CrystalSphereItemKind(CrystalSphereItem item) => item switch
    {
        MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereItems.CrystalSphereCardReward => "card_reward",
        MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereItems.CrystalSphereCurse => "curse",
        MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereItems.CrystalSphereGold => "gold",
        MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereItems.CrystalSpherePotion => "potion",
        MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereItems.CrystalSphereRelic => "relic",
        _ => item.GetType().Name,
    };

    /// <summary>A prompt other than the minigame itself is waiting for the client.</summary>
    private bool OtherSelectionPending() =>
        _cardSelector.HasPending || _cardSelector.HasPendingReward || _pendingBundles != null || CustomRewardsPending;

    private Dictionary<string, object?> SphereClickNotSettled() =>
        Failure("timeout", "started", "the Crystal Sphere divination did not finish", "the divination to complete",
            "divine_not_settled");

    /// <summary>
    /// A divination that hung: until it completes, no decision is presented (its effects
    /// may still land). Null when there is none, or once it has finished.
    /// </summary>
    private Dictionary<string, object?>? PendingSphereClickFailure()
    {
        if (_agentSphereClick == null) return null;
        if (!_agentSphereClick.IsCompleted) return SphereClickNotSettled();
        if (_agentSphereClick.IsFaulted) LogException("Crystal Sphere divination", _agentSphereClick.Exception!);
        _agentSphereClick = null;
        return null;
    }

    private Dictionary<string, object?>? ValidateDivine(Dictionary<string, object?>? args)
    {
        if (PendingCrystalSphere is not { } game || game.IsFinished)
            return Reject("no_pending_minigame", "no Crystal Sphere minigame is waiting for a move");
        if (!TryGetInt(args, "x", out var x) || !TryGetInt(args, "y", out var y))
            return Reject("missing_argument", "divine requires 'x' and 'y'");
        if (args!.GetValueOrDefault("tool") is not string tool)
            return Reject("missing_argument", "divine requires 'tool' (big or small)");
        if (tool is not ("big" or "small"))
            return Reject("invalid_tool", $"tool must be big or small, got {tool}");
        var size = game.GridSize;
        if (x < 0 || x >= size.X || y < 0 || y >= size.Y)
            return Reject("invalid_cell", $"({x},{y}) is outside the {size.X}x{size.Y} grid");
        if (!game.cells[x, y].IsHidden)
            return Reject("cell_cleared", $"({x},{y}) is already clear");
        return null;
    }

    private Dictionary<string, object?> DoDivine(Dictionary<string, object?>? args)
    {
        var game = PendingCrystalSphere!;  // checked by ValidateDivine
        TryGetInt(args, "x", out var x);
        TryGetInt(args, "y", out var y);
        var tool = args!["tool"] as string == "small"
            ? CrystalSphereMinigame.CrystalSphereToolType.Small
            : CrystalSphereMinigame.CrystalSphereToolType.Big;
        Log($"Crystal Sphere: {tool} divination at ({x},{y}), {game.DivinationCount} left before it");
        var click = Task.Run(async () =>
        {
            SurfaceCustomRewards.Value = true;
            game.SetTool(tool);
            await game.CellClicked(game.cells[x, y]);
        });
        // The last divination completes the minigame, and the event option awaiting it
        // goes on to offer the revealed items: wait until the click is done or that offer
        // (or another prompt) is waiting for the client. The minigame's own pending state
        // doesn't count: it is pending before the click starts.
        if (!WaitFor(() => click.IsCompleted || OtherSelectionPending(), SettleCapMs, "Crystal Sphere divination"))
        {
            _agentSphereClick = click;
            return SphereClickNotSettled();
        }
        if (click.IsFaulted) LogException("Crystal Sphere divination", click.Exception!);
        WaitForActionExecutor();
        _syncCtx.Pump();
        if (game.IsFinished)
        {
            PendingCrystalSphere = null;
            DriveEventOptionTask();
        }
        return DetectDecisionPoint();
    }
}
