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
// (as in the game). Throwing a Foul Potion at the merchant (its fight) is not offered: the
// game's usability check needs the NFakeMerchant node, so a held Foul Potion is listed
// `usable: false` and use_potion refuses it. With agent mode off none of this runs.

using MegaCrit.Sts2.Core.Entities.Merchant;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Models.Events;
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
}
