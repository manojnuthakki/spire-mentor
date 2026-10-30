// SimOutOfCombatPotions.cs -- the `potion` verb outside a fight (sts2-agent fork, D1b).
//
// The game's belt stays clickable outside combat: its popup (NPotionPopup) enables Discard
// for any potion that isn't queued while the owner lives and may remove potions, and Use
// only for an AnyTime one that also passes the potion's own usability check
// (PassesCustomUsabilityCheck). Use then goes through NPotionHolder.UsePotion: a potion that
// needs a target (an enemy, the merchant: Foul Potion) is aimed by hand, any other is
// enqueued at once (PotionModel.EnqueueManualUse, a UsePotionAction). Discard enqueues a
// DiscardPotionGameAction. The decisions that opt in (map, event, rest site, treasure, shop,
// rewards screen, card reward) mark the message `potion_verb: true` and each held potion slot
// in its `run_state` with `use_now` and `discard_now`; the bridge accepts `potion` there only
// where those say so. Of the aimed potions only Foul Potion is offered, thrown at the merchant
// (FoulPotion.GetFoulPotionMerchantTarget): in the shop the bridge closes the shopfront first,
// as the game needs, and the shop handler opens it again; any other aimed potion is never
// offered (`use_now` false).
//
// Each action is enqueued here and awaited by its own lifecycle (GameAction.State: Finished,
// or Canceled, which leaves the belt as it was and is answered as a refusal), with the screens
// it opens drained above the decision's own (SimAutoSlayer.AwaitWithScreenDrainAsync's floor).
// A wedged action has no timeout of its own: the run watchdog's stall limit ends the run.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using MegaCrit.Sts2.Core.AutoSlay;
using MegaCrit.Sts2.Core.Combat;
using MegaCrit.Sts2.Core.Entities.Actions;
using MegaCrit.Sts2.Core.Entities.Cards;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Entities.Potions;
using MegaCrit.Sts2.Core.GameActions;
using MegaCrit.Sts2.Core.AutoSlay.Helpers;
using MegaCrit.Sts2.Core.Entities.Creatures;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Models.Potions;
using MegaCrit.Sts2.Core.Nodes.CommonUi;
using MegaCrit.Sts2.Core.Nodes.Rooms;
using MegaCrit.Sts2.Core.Nodes.Screens.Overlays;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

internal static class SimOutOfCombatPotions
{
    public const string PotionVerbField = "potion_verb";
    private const int SettlePollMs = 16;
    private const int InventoryCloseSettleMs = 300;
    private static TimeSpan InventoryCloseWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(5));

    /// <summary>Drink or discard is possible outside a fight at all (no fight running).</summary>
    private static bool OutsideCombat => !(CombatManager.Instance?.IsInProgress ?? false);

    private static bool CanDiscard(Player player, PotionModel? potion) =>
        potion != null && !potion.IsQueued && !player.Creature.IsDead && player.CanRemovePotions;

    /// <summary>
    /// The popup's Use outside a fight, for a potion that needs no aiming, and Foul Potion
    /// thrown at the merchant: in the shop while the merchant is there (the game wants the
    /// shopfront closed, which <see cref="HandleAsync"/> does first), elsewhere (the fake
    /// merchant's event) by the potion's own check.
    /// </summary>
    private static bool CanUse(Player player, PotionModel? potion)
    {
        if (!CanDiscard(player, potion) || potion!.Usage != PotionUsage.AnyTime)
            return false;
        TargetType t = potion.TargetType;
        if (t == TargetType.TargetedNoCreature)
            return potion is FoulPotion && (InShop(player) ? NMerchantRoom.Instance?.MerchantButton != null : Passes(potion));
        if (t == TargetType.AnyEnemy || potion.CanThrowAtAlly())
            return false;
        return Passes(potion);
    }

    private static bool Passes(PotionModel potion)
    {
        try { return potion.PassesCustomUsabilityCheck; }
        catch { return false; }
    }

    private static bool InShop(Player player) => player.RunState.CurrentRoom is MerchantRoom;

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
    /// then drinks or discards the potion in `slot` (`discard: true`) and waits until the
    /// game's action has finished, draining any screen it opens. A refusal is a structured error
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

        // The screens up now belong to the decision being answered (the rewards screen under a
        // drink): only those the action opens are drained.
        int floor = NOverlayStack.Instance?.ScreenCount ?? 0;
        GameAction action;
        if (discard)
        {
            if (!CanDiscard(player, potion))
            {
                SimProtocolHelpers.SendError("potion_not_discardable",
                    $"potion {potion.Id.Entry} in slot {slot} can't be discarded now", runState, player);
                return;
            }
            Logger.Log($"[SimPotions] Discarding {potion.Id.Entry} in slot {slot} outside combat");
            action = new DiscardPotionGameAction(player, (uint)slot, false);
        }
        else
        {
            if (!CanUse(player, potion))
            {
                SimProtocolHelpers.SendError("potion_not_usable",
                    $"potion {potion.Id.Entry} in slot {slot} can't be used now", runState, player);
                return;
            }
            if (potion.TargetType == TargetType.TargetedNoCreature && InShop(player)
                && !await CloseShopfrontAsync(ct))
            {
                SimProtocolHelpers.SendError("potion_not_usable",
                    $"potion {potion.Id.Entry}: the shopfront did not close", runState, player);
                return;
            }
            if (!Passes(potion))
            {
                SimProtocolHelpers.SendError("potion_not_usable",
                    $"potion {potion.Id.Entry} in slot {slot} can't be used now", runState, player);
                return;
            }
            Logger.Log($"[SimPotions] Using {potion.Id.Entry} in slot {slot} outside combat");
            // EnqueueManualUse's target: the player for a potion that may target them, none
            // for Foul Potion (TargetedNoCreature) and the untargeted ones.
            Creature? target = potion.IsValidTarget(player.Creature) ? player.Creature : null;
            action = new UsePotionAction(potion, target, false);
        }
        RunManager.Instance.ActionQueueSynchronizer.RequestEnqueue(action);

        await SimAutoSlayer.AwaitWithScreenDrainAsync(SettledAsync(action, ct), ct,
            $"potion {potion.Id.Entry} outside combat", floor);
        if (action.State == GameActionState.Canceled)
        {
            // The game cancelled it before it changed anything (no potion, an invalid target).
            SimProtocolHelpers.SendError(discard ? "potion_not_discardable" : "potion_not_usable",
                $"the game cancelled the {(discard ? "discard" : "use")} of {potion.Id.Entry} in slot {slot}", runState, player);
        }
    }

    /// <summary>
    /// Completes once the action has run to its end (Finished) or been cancelled; a paused
    /// action (a choice it gathers) is waited through. Unbounded by itself: the caller's
    /// drain checks the run watchdog.
    /// </summary>
    private static async Task SettledAsync(GameAction action, CancellationToken ct)
    {
        while (action.State != GameActionState.Finished && action.State != GameActionState.Canceled)
        {
            ct.ThrowIfCancellationRequested();
            await Task.Delay(SettlePollMs, ct);
        }
    }

    /// <summary>
    /// Closes the merchant's shopfront (its back button, as a player does) so Foul Potion
    /// can find the merchant; the shop handler opens it again after the throw.
    /// </summary>
    private static async Task<bool> CloseShopfrontAsync(CancellationToken ct)
    {
        NMerchantRoom? room = NMerchantRoom.Instance;
        if (room?.Inventory == null)
            return false;
        if (!room.Inventory.IsOpen)
            return true;
        NBackButton? back = UiHelper.FindFirst<NBackButton>(room);
        if (back == null)
            return false;
        Logger.Log("[SimPotions] Closing the shopfront for Foul Potion");
        await UiHelper.Click(back);
        try
        {
            await WaitHelper.Until(() => !room.Inventory.IsOpen, ct, InventoryCloseWait, "Shopfront did not close");
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            return false;
        }
        await Task.Delay(InventoryCloseSettleMs, ct);
        return true;
    }

}
