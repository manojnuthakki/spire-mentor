// SimMapHandler.cs -- agent-driven map navigation (map_select).
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
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Map;
using MegaCrit.Sts2.Core.Nodes;
using MegaCrit.Sts2.Core.Nodes.Screens.Map;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

public class SimMapHandler : IScreenHandler, IHandler
{
    private const string RunNodePath = "/root/Game/RootSceneContainer/Run";
    private static TimeSpan PointEnableWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));

    /// <summary>Completed by <see cref="OnRoomEntered"/> while a map click is in flight.</summary>
    private TaskCompletionSource<bool>? _roomEntered;

    public Type ScreenType => typeof(NMapScreen);

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    /// <summary>
    /// Asks the agent which reachable node to travel to, clicks it, and
    /// returns once the next room has been entered.
    /// </summary>
    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        Logger.Log("[SimMap] Waiting for the map screen");
        NRun runNode = SimRoomNodes.SceneRoot.GetNode<NRun>(RunNodePath);
        await WaitHelper.Until(() => runNode.GlobalUi.MapScreen.IsVisibleInTree(), ct,
            SimConfig.Instance.Scaled(AutoSlayConfig.mapScreenTimeout), "Map screen never became visible");
        NMapScreen map = runNode.GlobalUi.MapScreen;

        RunState? runState = RunManager.Instance.DebugOnlyGetState();
        List<NMapPoint> available = runState != null ? AvailableNodes(map, runState) : new List<NMapPoint>();
        if (available.Count == 0)
        {
            Logger.Log("[SimMap] No reachable map nodes; nothing to choose");
            return;
        }

        Player? player = LocalContext.GetMe(runState);
        List<Dictionary<string, object>> nodes = available.Select((p, i) => new Dictionary<string, object>
        {
            ["index"] = i,
            ["type"] = p.Point.PointType.ToString(),
            ["row"] = p.Point.coord.row,
            ["col"] = p.Point.coord.col,
        }).ToList();
        var message = new Dictionary<string, object>
        {
            [NonCombatBridgeProtocol.TypeField] = "map_select",
            ["nodes"] = nodes,
            ["map"] = SimStateExtractor.SerializeMapGraph(runState!),
        };

        NMapPoint point = await ChooseNodeAsync(available, message, runState, player, ct);

        // Points stay disabled while the map animates in.
        await WaitHelper.Until(() => point.IsEnabled, ct, PointEnableWait, "Chosen map point never became enabled");

        // Default options on purpose: the continuation runs inline inside the
        // RoomEntered callback, the timing proven against the real game.
        var entered = new TaskCompletionSource<bool>();
        _roomEntered = entered;
        RunManager.Instance.RoomEntered += OnRoomEntered;
        try
        {
            await UiHelper.Click(point);
            await WaitHelper.ForTask(entered.Task, ct, SimConfig.Instance.Scaled(AutoSlayConfig.mapScreenTimeout),
                "Next room was not entered after clicking the map point");
        }
        finally
        {
            RunManager.Instance.RoomEntered -= OnRoomEntered;
            _roomEntered = null;
        }
        Logger.Log("[SimMap] Entered the next room");
    }

    /// <summary>
    /// The map points the player can travel to next: row 0 before the first
    /// move, otherwise the children of the last visited point. Shared with
    /// advisor mode so both lanes offer the same set.
    /// </summary>
    internal static List<NMapPoint> AvailableNodes(Node mapScreen, RunState runState)
    {
        List<NMapPoint> points = UiHelper.FindAll<NMapPoint>(mapScreen);
        if (runState.VisitedMapCoords.Count == 0)
        {
            return points.Where(p => p.Point.coord.row == 0).ToList();
        }

        MapCoord current = runState.VisitedMapCoords[runState.VisitedMapCoords.Count - 1];
        NMapPoint? here = points.FirstOrDefault(p => p.Point.coord.Equals(current));
        if (here == null)
        {
            return new List<NMapPoint>();
        }

        var next = new HashSet<MapCoord>(here.Point.Children.Select(child => child.coord));
        return points.Where(p => next.Contains(p.Point.coord)).ToList();
    }

    /// <summary>
    /// Asks until the reply names a valid node. Every ask re-envelopes the
    /// message (fresh state_id, PROTOCOL.md section 3a); the reply's
    /// <c>index</c> is read without an <c>action</c> check.
    /// </summary>
    private static async Task<NMapPoint> ChooseNodeAsync(
        List<NMapPoint> available, Dictionary<string, object> message, RunState? runState, Player? player,
        CancellationToken ct)
    {
        while (true)
        {
            SimStateExtractor.ApplyEnvelope(message, runState, player);
            long stateId = SimAgentChoice.ReadStateId(message);
            string json = JsonSerializer.Serialize(message);
            string reply = await SimAgentGate.RequestActionAsync(json, ct, "map_select");
            try
            {
                if (!SimProtocolHelpers.TryParse(reply, runState, player, out JsonElement root) ||
                    !SimProtocolHelpers.CheckStateId(root, stateId, runState, player))
                {
                    continue;
                }

                if (!root.TryGetProperty("index", out JsonElement indexEl) ||
                    indexEl.ValueKind != JsonValueKind.Number ||
                    !indexEl.TryGetInt32(out int index))
                {
                    SimProtocolHelpers.SendError("malformed_action",
                        "map_select reply requires an integer \"index\"", runState, player);
                    continue;
                }

                if (index >= 0 && index < available.Count)
                {
                    MapPoint chosen = available[index].Point;
                    Logger.Log($"[SimMap] Agent chose node {index}: {chosen.PointType} " +
                               $"(row {chosen.coord.row}, col {chosen.coord.col})");
                    return available[index];
                }

                SimProtocolHelpers.SendError("invalid_index",
                    $"index {index} out of range ({available.Count} nodes)", runState, player);
                Logger.Log($"[SimMap] Invalid node index {index} for {available.Count} nodes; asking again");
            }
            catch (Exception ex)
            {
                Logger.Log($"[SimMap] Error handling map_select reply: {ex.Message}");
                SimProtocolHelpers.SendError("internal_error", ex.Message, runState, player);
            }
        }
    }

    private void OnRoomEntered()
    {
        _roomEntered?.TrySetResult(true);
    }
}
