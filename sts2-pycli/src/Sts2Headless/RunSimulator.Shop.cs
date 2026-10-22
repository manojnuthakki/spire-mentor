// sts2-agent fork: the shop as a decision, in agent mode.
//
// Upstream lists the stock and buys through blocking calls: a failed buy is an
// unstructured error, a potion bought with a full belt silently does nothing, `proceed`
// stays in the shop, a completed card removal is never marked used (in the game only
// the shop's UI node calls MerchantCardRemovalEntry.SetUsed), rewards a purchase offers
// are auto-taken, and a purchase that offers card rewards (Orrery) returns to the shop
// half-done and can be bought again. Lord's Parasol throws on entering a shop: its
// PurchaseEverything calls NRun's UI without a TestMode guard at the first relic. In
// agent mode:
//
//   shop: buy_card{card_index}, buy_relic{relic_index}, buy_potion{potion_index},
//         remove_card, proceed (leave), use_potion, discard_potion
//
// Each slot says whether the game's purchase check would pass (`purchasable`) and, if
// not, why (`reason`): sold_out, not_enough_gold, potion_forbidden, belt_full; the card
// removal likewise (`card_removal: {cost, available, reason}`, reason removal_used). A
// buy is checked the same way before anything runs. A purchase runs through the game's
// MerchantEntry.OnTryPurchaseWrapper on the thread pool, like a rest-site option: what it
// opens (Kifuda's enchant, Dolly's Mirror's pick, the removal's cancelable prompt)
// reaches the client as the ordinary card selection, and rewards it offers (Cauldron's
// potions, Orrery's card rewards) as the `rewards` decision (source custom); the shop is
// shown again only once the purchase has finished. A completed removal marks the
// removal used. Lord's Parasol's purchases run in the game's order without its UI calls:
// every stocked card, relic and potion, then the card removal (not cancelable), all with
// the cost ignored; the decision that follows reports
// `forced: [{kind: "purchase_everything", items}]`. The fake merchant's relics use the same
// listing, checks and purchases (RunSimulator.FakeMerchant.cs). With agent mode off none of
// this runs.

using HarmonyLib;
using MegaCrit.Sts2.Core.Entities.Merchant;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Hooks;
using MegaCrit.Sts2.Core.Models.Relics;
using MegaCrit.Sts2.Core.Rooms;

namespace Sts2Headless;

public partial class RunSimulator
{
    /// <summary>A purchase still running (suspended at what it opened).</summary>
    private Task<bool>? _agentShopTask;

    /// <summary>The card removal being bought, marked used once the purchase succeeds.</summary>
    private MerchantCardRemovalEntry? _agentShopRemoval;

    /// <summary>Lord's Parasol's purchases, started on entering a shop (set by its prefix).</summary>
    private static Task? _agentParasolTask;

    /// <summary>What Lord's Parasol has bought and not yet reported (a forced step on the next decision).</summary>
    private static readonly List<Dictionary<string, object?>> _agentParasolItems = new();

    private static bool _shopPatchesInstalled;

    private void ResetAgentShop()
    {
        _agentShopTask = null;
        _agentShopRemoval = null;
    }

    private static void InstallShopPatches()
    {
        if (_shopPatchesInstalled) return;
        _shopPatchesInstalled = true;
        var method = AccessTools.Method(typeof(LordsParasol), "PurchaseEverything");
        if (method == null)
        {
            Log("[WARN] LordsParasol.PurchaseEverything not found; Lord's Parasol not patched");
            return;
        }
        new Harmony("sts2headless.lordsparasol").Patch(method,
            new HarmonyMethod(typeof(RunSimulator), nameof(LordsParasolPrefix)));
    }

    /// <summary>Agent mode: Lord's Parasol buys everything without the game's UI calls.</summary>
    private static bool LordsParasolPrefix(MerchantInventory inventory, ref Task __result)
    {
        if (!AgentModeActive) return true;
        lock (_agentParasolItems) _agentParasolItems.Clear();
        __result = _agentParasolTask = PurchaseEverythingHeadless(inventory);
        return false;
    }

    /// <summary>A purchase Lord's Parasol completed (a full belt or a hook can refuse one).</summary>
    private static void ParasolBought(string type, string? id)
    {
        lock (_agentParasolItems) _agentParasolItems.Add(new() { ["type"] = type, ["id"] = id });
    }

    /// <summary>
    /// LordsParasol.PurchaseEverything (v0.107.1) without its UI: character cards,
    /// colorless cards, relics, potions, each stocked entry with the cost ignored, then the
    /// card removal with the cost ignored and no cancel.
    /// </summary>
    private static async Task PurchaseEverythingHeadless(MerchantInventory inventory)
    {
        SurfaceCustomRewards.Value = true;
        var entries = new List<MerchantEntry>();
        entries.AddRange(inventory.CharacterCardEntries);
        entries.AddRange(inventory.ColorlessCardEntries);
        entries.AddRange(inventory.RelicEntries);
        entries.AddRange(inventory.PotionEntries);
        foreach (var entry in entries)
        {
            if (!entry.IsStocked) continue;
            // Read before buying: a completed purchase clears the entry's item.
            var (type, id) = entry switch
            {
                MerchantCardEntry c => ("card", c.CreationResult?.Card.Id.Entry),
                MerchantRelicEntry r => ("relic", r.Model?.Id.Entry),
                MerchantPotionEntry p => ("potion", p.Model?.Id.Entry),
                _ => ("item", null),
            };
            if (await entry.OnTryPurchaseWrapper(inventory, true)) ParasolBought(type, id);
        }
        var removal = inventory.CardRemovalEntry;
        if (removal != null && !removal.Used && await removal.OnTryPurchaseWrapper(inventory, true, false))
        {
            removal.SetUsed();
            ParasolBought("card_removal", null);
        }
    }

    /// <summary>
    /// Drive a running purchase (or Lord's Parasol's) once what it opened has been
    /// answered: it completes or suspends at the next prompt.
    /// </summary>
    private void DriveAgentShopTask()
    {
        if (_agentParasolTask != null && !AnySelectionPending())
        {
            WaitUntilOptionTaskSettles(_agentParasolTask, "Lord's Parasol purchases (until next prompt or completion)");
            if (_agentParasolTask.IsCompleted)
            {
                var task = _agentParasolTask;
                _agentParasolTask = null;
                if (task.IsFaulted) LogException("Lord's Parasol", task.Exception!);
                WaitForActionExecutor();
                _syncCtx.Pump();
            }
        }
        // What it has bought so far, reported with the decision that follows (the rest, after a
        // prompt it opened, with a later one).
        lock (_agentParasolItems)
        {
            if (_agentParasolItems.Count > 0)
            {
                _agentForced.Add(new() { ["kind"] = "purchase_everything", ["items"] = _agentParasolItems.ToList() });
                _agentParasolItems.Clear();
            }
        }
        if (_agentShopTask != null && !AnySelectionPending())
        {
            WaitUntilOptionTaskSettles(_agentShopTask, "shop purchase (until next prompt or completion)");
            if (_agentShopTask.IsCompleted)
            {
                var task = _agentShopTask;
                _agentShopTask = null;
                if (task.IsFaulted) LogException("Shop purchase", task.Exception!);
                else if (task.Result && _agentShopRemoval != null) _agentShopRemoval.SetUsed();
                _agentShopRemoval = null;
                WaitForActionExecutor();
                _syncCtx.Pump();
            }
        }
    }

    /// <summary>
    /// Why the game's purchase check would refuse this entry now, or null if it would pass
    /// (MerchantEntry.OnTryPurchaseWrapper: stock, gold; a potion: Hook.ShouldProcurePotion,
    /// then an open slot).
    /// </summary>
    private string? ShopEntryReason(MerchantEntry entry, Player player)
    {
        if (entry is MerchantCardRemovalEntry removal)
        {
            if (removal.Used) return "removal_used";
        }
        else if (!entry.IsStocked) return "sold_out";
        if (!entry.EnoughGold) return "not_enough_gold";
        // The removal offers the deck's removable cards (CardSelectCmd.FromDeckForRemoval); with
        // none it spends nothing and does nothing.
        if (entry is MerchantCardRemovalEntry && !player.Deck.Cards.Any(c => c != null && c.IsRemovable))
            return "no_removable_cards";
        if (entry is MerchantPotionEntry potion)
        {
            if (!Hook.ShouldProcurePotion(_runState!, player.Creature?.CombatState, potion.Model!, player))
                return "potion_forbidden";
            if (!player.HasOpenPotionSlots) return "belt_full";
        }
        return null;
    }

    /// <summary>Agent mode's additions to the shop decision (RunSimulator.cs, ShopState).</summary>
    private void AgentShopFields(Dictionary<string, object?> state, MerchantInventory inv, Player player)
    {
        // The Hoarder modifier forbids the removal; the game's slot marks it used on display.
        if (inv.CardRemovalEntry is { Used: false } r && !Hook.ShouldAllowMerchantCardRemoval(_runState!, player))
            r.SetUsed();

        void Mark(List<Dictionary<string, object?>> dicts, IEnumerable<MerchantEntry> entries, params string[] itemKeys)
        {
            foreach (var (dict, entry) in dicts.Zip(entries))
            {
                var reason = ShopEntryReason(entry, player);
                dict["purchasable"] = reason == null;
                dict["reason"] = reason;
                if (entry.IsStocked) continue;
                foreach (var key in itemKeys) dict[key] = null;
                dict["cost"] = null;
            }
        }

        var cardEntries = inv.CharacterCardEntries.Concat(inv.ColorlessCardEntries).ToList();
        var cards = (List<Dictionary<string, object?>>)state["cards"]!;
        foreach (var (dict, entry) in cards.Zip(cardEntries))
        {
            var card = entry.CreationResult?.Card;
            dict["rarity"] = card?.Rarity.ToString();
            dict["upgraded"] = card?.IsUpgraded;
            if (card == null) continue;
            // The card's own values: ShopState's ToMutable() throws on this (already mutable) card,
            // so its stats were always empty.
            var stats = new Dictionary<string, object?>();
            try { foreach (var dv in card.DynamicVars.Values) stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue; } catch { }
            dict["stats"] = stats.Count > 0 ? stats : null;
            var kws = card.Keywords?.Where(k => k != MegaCrit.Sts2.Core.Entities.Cards.CardKeyword.None).Select(k => k.ToString()).ToList();
            dict["keywords"] = kws?.Count > 0 ? kws : null;
            AgentCardExtras(dict, card);
        }
        Mark(cards, cardEntries, "id", "name", "type", "card_cost", "description", "stats", "after_upgrade", "rarity", "upgraded", "tinker_time_rider");
        Mark((List<Dictionary<string, object?>>)state["relics"]!, inv.RelicEntries, "id", "name", "description", "vars");
        Mark((List<Dictionary<string, object?>>)state["potions"]!, inv.PotionEntries, "id", "name", "description", "vars");

        var removal = inv.CardRemovalEntry;
        if (removal == null) state["card_removal"] = null;
        else
        {
            var reason = ShopEntryReason(removal, player);
            state["card_removal"] = new Dictionary<string, object?>
            {
                ["cost"] = removal.Cost,
                ["available"] = reason == null,
                ["reason"] = reason,
            };
        }
        state["can_proceed"] = true;
    }

    /// <summary>The entry a buy names, or null (the index is checked by the validation).</summary>
    private static MerchantEntry? ShopEntryFor(string action, Dictionary<string, object?>? args, MerchantInventory inv, out string? missing)
    {
        missing = null;
        if (action == "remove_card") return inv.CardRemovalEntry;
        var (key, entries) = action switch
        {
            "buy_card" => ("card_index", inv.CharacterCardEntries.Concat<MerchantEntry>(inv.ColorlessCardEntries).ToList()),
            "buy_relic" => ("relic_index", inv.RelicEntries.Cast<MerchantEntry>().ToList()),
            _ => ("potion_index", inv.PotionEntries.Cast<MerchantEntry>().ToList()),
        };
        if (!TryGetInt(args, key, out var index)) { missing = key; return null; }
        return index >= 0 && index < entries.Count ? entries[index] : null;
    }

    private Dictionary<string, object?>? ValidateShopBuy(string action, Dictionary<string, object?>? args, Player player)
    {
        if (AgentShopInventory() is not { } inv)
            return Reject("wrong_decision", "not in a shop");
        var entry = ShopEntryFor(action, args, inv, out var missing);
        if (missing != null) return Reject("missing_argument", $"{action} requires '{missing}'");
        if (entry == null)
            return action == "remove_card"
                ? Reject("no_card_removal", "this shop offers no card removal")
                : Reject($"invalid_{action[4..]}_index", $"{action}: no such slot");
        var reason = ShopEntryReason(entry, player);
        return reason == null ? null : Reject(reason, $"{action}: {reason}");
    }

    private Dictionary<string, object?> DoAgentBuy(string action, Dictionary<string, object?>? args)
    {
        var inv = AgentShopInventory()!;  // checked by ValidateShopBuy
        var entry = ShopEntryFor(action, args, inv, out _)!;  // checked by ValidateShopBuy
        Log($"Shop: {action} ({entry.GetType().Name}, {entry.Cost}g)");
        var removal = entry as MerchantCardRemovalEntry;
        _agentShopRemoval = removal;
        _agentShopTask = Task.Run(() =>
        {
            SurfaceCustomRewards.Value = true;
            return removal != null ? removal.OnTryPurchaseWrapper(inv, false, true) : entry.OnTryPurchaseWrapper(inv, false);
        });
        return DetectDecisionPoint();
    }

    /// <summary>
    /// Agent mode: a card's type and Mad Science's rider (Tinker Time's card is a Skill,
    /// Attack or Power, with a rider, so two Mad Science cards differ by both), and its
    /// enchantment, by model id and amount (Kifuda's Adroit 3), so two otherwise equal cards
    /// are told apart as on screen. The enchantment is added only to an enchanted card.
    /// </summary>
    private static void AgentCardExtras(Dictionary<string, object?> dict, MegaCrit.Sts2.Core.Models.CardModel card)
    {
        dict["type"] = card.Type.ToString();
        dict["tinker_time_rider"] = MadScienceRider(card);
        if (card.Enchantment == null) return;
        dict["enchantment_id"] = card.Enchantment.Id.Entry;
        try { dict["enchantment_amount"] = card.Enchantment.Amount; } catch { }
    }

    /// <summary>
    /// Agent mode: a card reward's offers say whether they are upgraded (Molten Egg and the
    /// like upgrade them as they are generated) and enchanted, as the screen shows them.
    /// </summary>
    private static void AgentRewardCardExtras(List<Dictionary<string, object?>> dicts, IEnumerable<MegaCrit.Sts2.Core.Models.CardModel> cards)
    {
        foreach (var (dict, card) in dicts.Zip(cards))
        {
            dict["upgraded"] = card.IsUpgraded;
            AgentCardExtras(dict, card);
        }
    }

    private Dictionary<string, object?> ProceedFromShop()
    {
        if (AgentFakeMerchant() is { } merchant) return ProceedFromFakeMerchant(merchant);
        Log("Shop: proceeding to the map");
        ForceToMap();
        return DetectDecisionPoint();
    }
}
