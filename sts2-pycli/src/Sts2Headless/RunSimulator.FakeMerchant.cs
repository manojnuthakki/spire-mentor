// sts2-agent fork: FAKE_MERCHANT as a relics-only shop, in agent mode.
//
// The game's FakeMerchant event has no options: it is finished from the start (an empty
// page), and everything happens in its NFakeMerchant node, which sells 6 of 9 fake relics
// from the event's own MerchantInventory (FakeMerchant.Inventory: relic entries only, no
// cards, potions or card removal) and leaves through the node's Proceed. Upstream sees a
// finished event and goes to the map, so the shop is lost. In agent mode the event is the
// ordinary `shop` decision over that inventory, with `event_id: FAKE_MERCHANT`: the relic
// slots with `purchasable`/`reason`, no cards or potions, `card_removal: null`. buy_relic
// is checked and runs exactly as at a merchant (RunSimulator.Shop.cs); `proceed` leaves for
// the map, reported as `forced: [{kind: "event_finished", event_id: "FAKE_MERCHANT"}]`.
//
// Prices are the entries' own: MerchantEntry.Cost applies Hook.ModifyMerchantPrice only in
// a MerchantRoom, and the restock after a purchase also needs one, so neither happens here
// (as in the game).
//
// The Foul Potion out of combat (audit batch 6). The game lets it be thrown at a merchant
// (FoulPotion.PassesCustomUsabilityCheck -> GetFoulPotionMerchantTarget: the MerchantRoom's
// or the fake merchant's node, its inventory closed), and Headless has no node, so the check
// always failed. In agent mode the target is the shop decision itself: a held Foul Potion is
// `usable` there (at a merchant and at the fake merchant, nowhere else out of combat) and
// use_potion throws it, with no target (TargetedNoCreature, as the game's merchant button
// gives). At a merchant the game's own OnUse then pays 100 gold (its node part, a dialogue
// line, is skipped when there is no node). At the fake merchant OnUse needs the NFakeMerchant
// node, so a prefix runs what its FakeMerchant branch runs without it:
// FakeMerchant.FoulPotionThrown, which starts FakeMerchantEventEncounter with the rug and the
// unsold relics as extra rewards and doesn't resume the event, so the fight's rewards lead
// to the map. With agent mode off none of this runs.

using HarmonyLib;
using MegaCrit.Sts2.Core.Combat;
using MegaCrit.Sts2.Core.Entities.Merchant;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Models.Events;
using MegaCrit.Sts2.Core.Models.Potions;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Runs;

namespace Sts2Headless;

public partial class RunSimulator
{
    /// <summary>The fake merchant the client has left (its finished event then goes to the map).</summary>
    private FakeMerchant? _agentFakeMerchantLeft;

    /// <summary>The fake merchant whose shop is open now, or null (agent mode).</summary>
    private FakeMerchant? AgentFakeMerchant() =>
        _agentMode && _runState?.CurrentRoom is EventRoom
        && RunManager.Instance.EventSynchronizer?.GetLocalEvent() is FakeMerchant merchant
        && merchant.Inventory != null && !merchant.StartedFight
        && !ReferenceEquals(merchant, _agentFakeMerchantLeft)
            ? merchant : null;

    /// <summary>The inventory a shop action acts on: the merchant room's, or the fake merchant's.</summary>
    private MerchantInventory? AgentShopInventory() =>
        _runState?.CurrentRoom is MerchantRoom room ? room.GetLocalInventory() : AgentFakeMerchant()?.Inventory;

    /// <summary>The fake merchant's shop decision, or null when none is open.</summary>
    private Dictionary<string, object?>? AgentFakeMerchantState(Player player)
    {
        if (AgentFakeMerchant() is not { } merchant) return null;
        var state = ShopState(merchant.Inventory, player);
        state["event_id"] = merchant.Id.Entry;
        return state;
    }

    /// <summary>Leave the fake merchant: its finished event goes to the map (event_finished).</summary>
    private Dictionary<string, object?> ProceedFromFakeMerchant(FakeMerchant merchant)
    {
        Log("Fake merchant: proceeding to the map");
        _agentFakeMerchantLeft = merchant;
        return DetectDecisionPoint();
    }

    /// <summary>
    /// A held Foul Potion can be thrown at the shop shown now (agent mode; called by
    /// AgentShopFields): the game's target is the merchant's node with its inventory closed,
    /// which in Headless is the shop decision itself.
    /// </summary>
    private static void AgentFoulPotionTarget(Dictionary<string, object?> state, Player player)
    {
        if (CombatManager.Instance.IsInProgress || !player.CanRemovePotions) return;
        if (state.GetValueOrDefault("player") is not Dictionary<string, object?> summary
            || summary.GetValueOrDefault("potions") is not List<Dictionary<string, object?>> dicts) return;
        foreach (var (dict, potion) in dicts.Zip(player.Potions.Where(p => p != null)))
            if (potion is FoulPotion) dict["usable"] = true;
    }

    /// <summary>A Foul Potion thrown at the shop the client is answering (agent mode).</summary>
    private bool AgentFoulPotionThrowable(PotionModel potion) =>
        potion is FoulPotion && !CombatManager.Instance.IsInProgress
        && _lastDecision?.GetValueOrDefault("decision") as string == "shop" && AgentShopInventory() != null;

    private static bool _foulPotionPatchInstalled;

    private static void InstallFoulPotionPatch()
    {
        if (_foulPotionPatchInstalled) return;
        _foulPotionPatchInstalled = true;
        var method = AccessTools.Method(typeof(FoulPotion), "OnUse");
        if (method == null)
        {
            Log("[WARN] FoulPotion.OnUse not found; the fake merchant's Foul Potion not patched");
            return;
        }
        new Harmony("sts2headless.foulpotion").Patch(method,
            new HarmonyMethod(typeof(RunSimulator), nameof(FoulPotionOnUsePrefix)));
    }

    /// <summary>
    /// FoulPotion.OnUse at the fake merchant (v0.107.1: if the event's node is an NFakeMerchant,
    /// show the vfx and await each player's FakeMerchant.FoulPotionThrown), without the node.
    /// </summary>
    private static bool FoulPotionOnUsePrefix(FoulPotion __instance, ref Task __result)
    {
        if (!AgentModeActive || CombatManager.Instance.IsInProgress) return true;
        var runState = __instance.Owner.RunState;
        if (runState.CurrentRoom is not EventRoom { CanonicalEvent: FakeMerchant }) return true;
        __result = Task.WhenAll(runState.Players.Select(p =>
            ((FakeMerchant)RunManager.Instance.EventSynchronizer.GetEventForPlayer(p)).FoulPotionThrown(__instance)));
        return false;
    }
}
