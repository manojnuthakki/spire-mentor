// SimCardRewardScreenHandler.cs -- the NCardRewardSelectionScreen overlay handler (card_reward).
// Originally ported from zhiyue/sts2-rl-agent (STS2BridgeMod); rewritten clean-room.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Godot;
using MegaCrit.Sts2.Core.AutoSlay.Handlers;
using MegaCrit.Sts2.Core.AutoSlay.Helpers;
using MegaCrit.Sts2.Core.Context;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Nodes.Cards.Holders;
using MegaCrit.Sts2.Core.Nodes.Screens.CardSelection;
using MegaCrit.Sts2.Core.Nodes.Screens.Overlays;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

public class SimCardRewardScreenHandler : IScreenHandler, IHandler
{
    private const int InitialSettleMs = 400;
    private static TimeSpan CloseWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));

    public Type ScreenType => typeof(NCardRewardSelectionScreen);

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    /// <summary>
    /// The card reward outside test mode: the game shows this screen and
    /// consults SimCardSelector only when it can't (TestMode,
    /// NCardRewardSelectionScreen.ShowScreen). Same reply contract as the
    /// selector's card_reward: fresh state_id per ask, the stale-state guard,
    /// and a structured error plus re-ask for any unusable reply (never
    /// throws). It takes the potion verb outside combat
    /// (SimOutOfCombatPotions), as the belt stays clickable over this screen:
    /// the cards on offer stay as they are.
    /// </summary>
    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        Logger.Log("[SimCardReward] Card reward screen appeared");
        var screen = (NCardRewardSelectionScreen)NOverlayStack.Instance!.Peek()!;
        await Task.Delay(InitialSettleMs, ct);

        List<NCardHolder> holders = UiHelper.FindAll<NCardHolder>(screen);
        if (holders.Count == 0)
        {
            Logger.Log("[SimCardReward] No card holders on the screen");
            return;
        }

        var cards = new List<Dictionary<string, object?>>();
        for (int i = 0; i < holders.Count; i++)
        {
            var model = holders[i].CardModel;
            Dictionary<string, object?> card = model != null
                ? SimStateExtractor.SerializeCard(model)
                : new Dictionary<string, object?>();
            card["index"] = i;
            cards.Add(card);
        }

        RunState? runState = RunManager.Instance.DebugOnlyGetState();
        Player? player = LocalContext.GetMe(runState);
        var message = new Dictionary<string, object>
        {
            [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.CardRewardState,
            ["cards"] = cards,
            ["can_skip"] = true,
        };

        NCardHolder? chosen = null;
        while (chosen == null)
        {
            runState = RunManager.Instance.DebugOnlyGetState();
            player = LocalContext.GetMe(runState);
            message.Remove(SimOutOfCombatPotions.PotionVerbField);
            SimStateExtractor.ApplyEnvelope(message, runState, player);
            SimOutOfCombatPotions.Offer(message, player);
            long stateId = SimAgentChoice.ReadStateId(message);
            string reply = await SimAgentGate.RequestActionAsync(
                JsonSerializer.Serialize(message), ct, "card_reward_screen");
            if (message.ContainsKey(SimOutOfCombatPotions.PotionVerbField) && SimOutOfCombatPotions.IsPotionVerb(reply))
            {
                await SimOutOfCombatPotions.HandleAsync(reply, stateId, runState, player, ct);
                continue;
            }
            if (!SimProtocolHelpers.TryParse(reply, runState, player, out JsonElement root) ||
                !SimProtocolHelpers.CheckStateId(root, stateId, runState, player))
            {
                continue;
            }

            try
            {
                string action = root.ValueKind == JsonValueKind.Object &&
                                root.TryGetProperty("action", out JsonElement actionEl) &&
                                actionEl.ValueKind == JsonValueKind.String
                    ? actionEl.GetString() ?? ""
                    : "";

                if (action == NonCombatBridgeProtocol.SkipAction)
                {
                    Logger.Log("[SimCardReward] Agent skipped the card reward");
                    SkipOrLog(screen);
                    return;
                }

                if (action != NonCombatBridgeProtocol.ChooseAction)
                {
                    SimProtocolHelpers.SendError("unknown_action",
                        $"Unrecognized action \"{action}\" for card_reward", runState, player);
                }
                else if (!root.TryGetProperty("index", out JsonElement indexEl) ||
                         indexEl.ValueKind != JsonValueKind.Number ||
                         !indexEl.TryGetInt32(out int index))
                {
                    SimProtocolHelpers.SendError("malformed_action",
                        "\"choose\" requires an integer \"index\"", runState, player);
                }
                else if (index >= holders.Count)
                {
                    Logger.Log($"[SimCardReward] Index {index} >= {holders.Count}: implicit skip");
                    SkipOrLog(screen);
                    return;
                }
                else if (index >= 0)
                {
                    chosen = holders[index];
                    Logger.Log($"[SimCardReward] Agent chose card {index}");
                    continue;
                }
                else
                {
                    SimProtocolHelpers.SendError("invalid_index",
                        $"index {index} out of range ({holders.Count} cards)", runState, player);
                }
            }
            catch (Exception ex)
            {
                Logger.Log($"[SimCardReward] Error handling card_reward reply: {ex.Message}");
                SimProtocolHelpers.SendError("internal_error", ex.Message, runState, player);
            }

            Logger.Log("[SimCardReward] card_reward reply rejected; asking again");
        }

        chosen.EmitSignal(NCardHolder.SignalName.Pressed, chosen);
        await WaitHelper.Until(() => !GodotObject.IsInstanceValid(screen) || !screen.IsVisibleInTree(),
            ct, CloseWait, "Card reward screen did not close after choosing");
        Logger.Log("[SimCardReward] Card reward screen closed");
    }

    private static void SkipOrLog(NCardRewardSelectionScreen screen)
    {
        if (!TrySkipCardReward(screen))
        {
            Logger.Log("[SimCardReward] Skip did not take effect");
        }
    }

    private static bool TrySkipCardReward(NCardRewardSelectionScreen screen)
    {
        // The game's own "skip" is dismissing the overlay: _ExitTree (and the
        // shared backstop click) resolve the screen's OptionSelected() task
        // with null, which CardReward.cs treats as endSelection-without-
        // reward (decomp CardReward.cs:260-267). Complete the same
        // TaskCompletionSource directly (publicized private field) -- the
        // awaiting CardReward then tears the screen down itself.
        try
        {
            FieldInfo? field = typeof(NCardRewardSelectionScreen).GetField(
                "_completionSource", BindingFlags.Instance | BindingFlags.NonPublic);
            var tcs = (TaskCompletionSource<int?>?)field?.GetValue(screen);
            if (tcs == null || tcs.Task.IsCompleted)
                return false;
            tcs.SetResult(null);
            return true;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimCardReward] Skip helper error: {ex.Message}");
            return false;
        }
    }
}
