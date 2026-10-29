// SimOutOfCombatPotions.cs -- the `potion` verb outside a fight (sts2-agent fork, D1b).
//
// The game's belt stays clickable outside combat: its popup (NPotionPopup) enables Discard
// for any potion that isn't queued while the owner lives and may remove potions, and Use
// only for an AnyTime one that also passes the potion's own usability check
// (PassesCustomUsabilityCheck). Use then goes through NPotionHolder.UsePotion: a potion that
// needs a target (an enemy, the merchant: Foul Potion) is aimed by hand, any other is
// enqueued at once (PotionModel.EnqueueManualUse). Discard enqueues a
// DiscardPotionGameAction. The decisions that opt in (map, event, rest site, treasure, shop,
// rewards screen) mark the message `potion_verb: true` and each held potion slot in its
// `run_state` with `use_now` and `discard_now`; the bridge accepts `potion` there only where
// those say so, and a potion that needs aiming is never offered (`use_now` false).

using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using MegaCrit.Sts2.Core.AutoSlay;
using MegaCrit.Sts2.Core.Combat;
using MegaCrit.Sts2.Core.Entities.Cards;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Entities.Potions;
using MegaCrit.Sts2.Core.GameActions;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

internal static class SimOutOfCombatPotions
{
    public const string PotionVerbField = "potion_verb";
    private const int SettlePollMs = 16;
    private static TimeSpan ResolveWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));

    /// <summary>Drink or discard is possible outside a fight at all (no fight running).</summary>
    private static bool OutsideCombat => !(CombatManager.Instance?.IsInProgress ?? false);

    private static bool CanDiscard(Player player, PotionModel? potion) =>
        potion != null && !potion.IsQueued && !player.Creature.IsDead && player.CanRemovePotions;

    /// <summary>The popup's Use outside a fight, for a potion that needs no aiming.</summary>
    private static bool CanUse(Player player, PotionModel? potion)
    {
        if (!CanDiscard(player, potion) || potion!.Usage != PotionUsage.AnyTime)
            return false;
        TargetType t = potion.TargetType;
        if (t == TargetType.AnyEnemy || t == TargetType.TargetedNoCreature || potion.CanThrowAtAlly())
            return false;
        try { return potion.PassesCustomUsabilityCheck; }
        catch { return false; }
    }

    /// <summary>Some held potion can be drunk or discarded now, outside a fight.</summary>
    public static bool AnyActionNow(Player? player) =>
        player != null && OutsideCombat && player.PotionSlots.Any(p => CanDiscard(player, p));

    /// <summary>
    /// Marks an enveloped message as taking the potion verb, and each held slot of its
    /// run_state with use_now / discard_now. Outside a fight only; a message in a fight is
    /// left as it is.
    /// </summary>
    public static void Offer(Dictionary<string, object> message, Player? player)
    {
        if (player == null || !OutsideCombat)
            return;
        if (!message.TryGetValue("run_state", out object? rs) || rs is not Dictionary<string, object?> runState
            || !runState.TryGetValue("potions", out object? ps) || ps is not List<Dictionary<string, object?>> slots)
            return;
        foreach (Dictionary<string, object?> slot in slots)
        {
            if (slot.TryGetValue("empty", out object? empty) && empty is true)
                continue;
            int index = slot.TryGetValue("slot", out object? s) && s is int i ? i : -1;
            PotionModel? potion = SimCombatHandler.SafeGetPotionAtSlot(player, index);
            slot["use_now"] = CanUse(player, potion);
            slot["discard_now"] = CanDiscard(player, potion);
        }
        message[PotionVerbField] = true;
    }

    /// <summary>
    /// The reply is a `potion` verb (whatever else it holds). Never throws.
    /// </summary>
    public static bool IsPotionVerb(string reply)
    {
        try
        {
            using JsonDocument doc = JsonDocument.Parse(reply);
            return doc.RootElement.ValueKind == JsonValueKind.Object
                && doc.RootElement.TryGetProperty("action", out JsonElement a)
                && a.ValueKind == JsonValueKind.String
                && a.GetString() == "potion";
        }
        catch
        {
            return false;
        }
    }

    /// <summary>
    /// Answers a `potion` reply to a message <see cref="Offer"/> marked: checks the state id,
    /// then drinks or discards the potion in `slot` (`discard: true`) and waits until it has
    /// left the belt, draining any screen it opens. A refusal is a structured error
    /// (empty_potion_slot, potion_not_usable, potion_not_discardable, malformed_action). The
    /// caller asks again either way, with a fresh message.
    /// </summary>
    public static async Task HandleAsync(string reply, long stateId, RunState? runState, Player? player, CancellationToken ct)
    {
        if (!SimProtocolHelpers.TryParse(reply, runState, player, out JsonElement root)
            || !SimProtocolHelpers.CheckStateId(root, stateId, runState, player))
            return;
        if (player == null || !OutsideCombat)
        {
            SimProtocolHelpers.SendError("potion_not_usable", "no potion can be used here", runState, player);
            return;
        }
        if (!root.TryGetProperty("slot", out JsonElement slotEl) || slotEl.ValueKind != JsonValueKind.Number
            || !slotEl.TryGetInt32(out int slot))
        {
            SimProtocolHelpers.SendError("malformed_action", "\"potion\" requires a numeric \"slot\"", runState, player);
            return;
        }
        bool discard = root.TryGetProperty("discard", out JsonElement d) && d.ValueKind == JsonValueKind.True;
        PotionModel? potion = SimCombatHandler.SafeGetPotionAtSlot(player, slot);
        if (potion == null)
        {
            SimProtocolHelpers.SendError("empty_potion_slot", $"potion slot {slot} is empty", runState, player);
            return;
        }

        if (discard)
        {
            if (!CanDiscard(player, potion))
            {
                SimProtocolHelpers.SendError("potion_not_discardable",
                    $"potion {potion.Id.Entry} in slot {slot} can't be discarded now", runState, player);
                return;
            }
            Logger.Log($"[SimPotions] Discarding {potion.Id.Entry} in slot {slot} outside combat");
            RunManager.Instance.ActionQueueSynchronizer.RequestEnqueue(
                new DiscardPotionGameAction(player, (uint)slot, false));
        }
        else
        {
            if (!CanUse(player, potion))
            {
                SimProtocolHelpers.SendError("potion_not_usable",
                    $"potion {potion.Id.Entry} in slot {slot} can't be used now", runState, player);
                return;
            }
            Logger.Log($"[SimPotions] Using {potion.Id.Entry} in slot {slot} outside combat");
            potion.EnqueueManualUse(potion.TargetType == TargetType.Self ? player.Creature : null);
        }

        Task left = WaitUntilGoneAsync(player, potion, slot, ct);
        await SimAutoSlayer.AwaitWithScreenDrainAsync(left, ct, $"potion {potion.Id.Entry} outside combat");
    }

    private static async Task WaitUntilGoneAsync(Player player, PotionModel potion, int slot, CancellationToken ct)
    {
        DateTime deadline = DateTime.UtcNow + ResolveWait;
        // The slot can fill again at once (Entropic Brew): wait for this potion to leave it.
        while (ReferenceEquals(SimCombatHandler.SafeGetPotionAtSlot(player, slot), potion) || potion.IsQueued)
        {
            ct.ThrowIfCancellationRequested();
            if (DateTime.UtcNow > deadline)
            {
                AutoSlayLog.Warn($"[SimPotions] {potion.Id.Entry} still in slot {slot} after {ResolveWait.TotalSeconds:0} s");
                return;
            }
            await Task.Delay(SettlePollMs, ct);
        }
    }
}
