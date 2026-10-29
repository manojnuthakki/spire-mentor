// SimNonCombatRoomHandlers.cs -- agent-driven handlers for every non-combat room and the boss/Ancient relic screen.
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
using MegaCrit.Sts2.Core.Entities.Merchant;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Events;
using MegaCrit.Sts2.Core.Hooks;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Nodes.CommonUi;
using MegaCrit.Sts2.Core.Nodes.Events;
using MegaCrit.Sts2.Core.Nodes.Events.Custom;
using MegaCrit.Sts2.Core.Nodes.GodotExtensions;
using MegaCrit.Sts2.Core.Nodes.Relics;
using MegaCrit.Sts2.Core.Nodes.RestSite;
using MegaCrit.Sts2.Core.Nodes.Rooms;
using MegaCrit.Sts2.Core.Nodes.Screens;
using MegaCrit.Sts2.Core.Nodes.Screens.Map;
using MegaCrit.Sts2.Core.Nodes.Screens.Overlays;
using MegaCrit.Sts2.Core.Nodes.Screens.Shops;
using MegaCrit.Sts2.Core.Nodes.Screens.TreasureRoomRelic;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

/// <summary>
/// Scene-tree locations and small UI predicates shared by the room handlers
/// in this file and by <see cref="SimAutoSlayer"/>.
/// </summary>
internal static class SimRoomNodes
{
    internal const string RoomContainer = "/root/Game/RootSceneContainer/Run/RoomContainer";
    internal const string RestSiteRoomPath = RoomContainer + "/RestSiteRoom";
    internal const string RestSiteProceedButtonPath = RestSiteRoomPath + "/ProceedButton";
    internal const string EventRoomPath = RoomContainer + "/EventRoom";
    internal const string TreasureRoomPath = RoomContainer + "/TreasureRoom";
    internal const string MerchantRoomPath = RoomContainer + "/MerchantRoom";

    internal static Node SceneRoot => ((SceneTree)Engine.GetMainLoop()).Root;

    /// <summary>The node is still a live part of the scene tree.</summary>
    internal static bool IsLive(Node? node) =>
        node != null && GodotObject.IsInstanceValid(node) && node.IsInsideTree();

    internal static bool OverlayUp() => (NOverlayStack.Instance?.ScreenCount ?? 0) > 0;

    internal static bool MapOpen() => NMapScreen.Instance?.IsOpen ?? false;

    internal static List<NEventOptionButton> UnlockedEventOptions(Node room) =>
        UiHelper.FindAll<NEventOptionButton>(room).Where(b => !b.Option.IsLocked).ToList();
}

/// <summary>
/// Wire vocabulary of the non-combat decision messages (PROTOCOL.md section 3/4).
/// Names and values are fixed: handlers, the advisor and the Python drivers all
/// key off these strings.
/// </summary>
internal static class NonCombatBridgeProtocol
{
    // Message fields
    public const string TypeField = "type";
    public const string ResultField = "result";
    public const string MessageField = "message";

    // Message types
    public const string CardSelectState = "card_select";
    public const string CardRewardState = "card_reward";
    public const string GameOverState = "game_over";
    public const string RunCompleteState = "run_complete";
    public const string RestSiteState = "rest_site";
    public const string RewardScreenState = "reward_screen";
    public const string CardBundleState = "card_bundle";
    public const string CrystalSphereState = "crystal_sphere";
    public const string ShopState = "shop";
    public const string EventState = "event";
    public const string TreasureState = "treasure";
    public const string BossRelicState = "boss_relic";
    public const string RunConfigState = "run_config";
    public const string ErrorState = "error";

    // Inbound verbs
    public const string ChooseAction = "choose";
    public const string SkipAction = "skip";
    public const string CancelAction = "cancel";
    public const string StartRunAction = "start_run";

    // Descriptive per-option actions
    public const string RestOptionAction = "rest_option";
    public const string LeaveShopAction = "leave_shop";
    public const string BuyCardAction = "buy_card";
    public const string BuyRelicAction = "buy_relic";
    public const string BuyPotionAction = "buy_potion";
    public const string RemoveCardAction = "remove_card";
    public const string BuyItemAction = "buy_item";
    public const string EventChoiceAction = "event_choice";
    public const string CollectTreasureAction = "collect";
    public const string PickRelicAction = "pick_relic";
    public const string PickRewardAction = "pick_reward";
    public const string PickCardBundleAction = "pick_card_bundle";
    public const string DivineCellAction = "divine_cell";
    public const string ProceedAction = "proceed";

    // Terminal results
    public const string VictoryResult = "victory";
    public const string DefeatResult = "defeat";
    public const string TerminatedResult = "terminated";
    public const string GameOverMessage = "Game over";
}

/// <summary>
/// The shared "ask the agent until the answer is valid" loop for positional
/// decisions (SPEC2 section 1.2). Each round builds a FRESH message (new
/// state_id, and the transport adds a new request_id), so an error is always
/// followed by a re-send. There is no timeout and no fallback: only
/// cancellation ends the loop.
/// </summary>
internal static class SimAgentChoice
{
    /// <summary>
    /// Asks until the reply picks an index in [0, <paramref name="optionCount"/>).
    /// </summary>
    /// <param name="buildBody">Builds the message body (type, options, extras)
    /// for the given run state / local player; the envelope is applied here.</param>
    /// <param name="negativeMeansFirst">Shop semantics: "skip" (or any negative
    /// index) selects option 0, and "skip" is an accepted verb.</param>
    /// <param name="rejectAtReply">Re-checks an in-range index when the reply
    /// arrives; a non-null result is the detail of an invalid_index error and
    /// the agent is asked again (e.g. "proceed" no longer enabled).</param>
    /// <param name="rejectReply">Like <paramref name="rejectAtReply"/>, with the
    /// parsed reply (fields beside the index: the Crystal Sphere's tool); a
    /// non-null result is (error code, detail).</param>
    /// <param name="potions">The decision takes the <c>potion</c> verb outside a
    /// fight (SimOutOfCombatPotions): a potion reply is answered and
    /// <see cref="PotionHandled"/> returned, so the caller re-reads its screen
    /// and asks again.</param>
    internal static async Task<int> AskAsync(
        Func<RunState?, Player?, Dictionary<string, object>> buildBody,
        int optionCount,
        string context,
        string logTag,
        CancellationToken ct,
        bool negativeMeansFirst = false,
        Func<int, string?>? rejectAtReply = null,
        Func<int, JsonElement, (string Code, string Detail)?>? rejectReply = null,
        bool potions = false)
    {
        while (true)
        {
            ct.ThrowIfCancellationRequested();

            RunState? runState = RunManager.Instance.DebugOnlyGetState();
            Player? player = LocalContext.GetMe(runState);
            Dictionary<string, object> message = buildBody(runState, player);
            SimStateExtractor.ApplyEnvelope(message, runState, player);
            if (potions)
            {
                SimOutOfCombatPotions.Offer(message, player);
            }
            long stateId = ReadStateId(message);
            string json = JsonSerializer.Serialize(message);

            string reply = await SimAgentGate.RequestActionAsync(json, ct, context);
            if (potions && message.ContainsKey(SimOutOfCombatPotions.PotionVerbField)
                && SimOutOfCombatPotions.IsPotionVerb(reply))
            {
                await SimOutOfCombatPotions.HandleAsync(reply, stateId, runState, player, ct);
                return PotionHandled;
            }
            int? parsed = SimProtocolHelpers.ReadChoiceIndex(reply, stateId, runState, player, negativeMeansFirst);
            if (parsed == null)
            {
                AutoSlayLog.Warn($"[{logTag}] Reply rejected; asking again");
                continue;
            }

            int index = parsed.Value;
            if (negativeMeansFirst && index < 0)
            {
                index = 0;
            }
            if (index >= 0 && index < optionCount)
            {
                string? rejection = rejectAtReply?.Invoke(index);
                if (rejection == null && rejectReply != null)
                {
                    using JsonDocument doc = JsonDocument.Parse(reply);  // parsed once already
                    if (rejectReply(index, doc.RootElement) is { } refused)
                    {
                        SimProtocolHelpers.SendError(refused.Code, refused.Detail, runState, player);
                        AutoSlayLog.Warn($"[{logTag}] Choice {index} rejected ({refused.Detail}); asking again");
                        continue;
                    }
                }
                if (rejection == null)
                {
                    return index;
                }
                SimProtocolHelpers.SendError("invalid_index", rejection, runState, player);
                AutoSlayLog.Warn($"[{logTag}] Choice {index} rejected ({rejection}); asking again");
                continue;
            }

            SimProtocolHelpers.SendError("invalid_index",
                $"index {index} out of range ({optionCount} options)", runState, player);
            AutoSlayLog.Warn($"[{logTag}] Invalid choice index {index} for {optionCount} options; asking again");
        }
    }

    /// <summary>What <see cref="AskAsync"/> returns after answering a potion reply.</summary>
    internal const int PotionHandled = int.MinValue;

    /// <summary>The state_id ApplyEnvelope stored (a boxed long), or -1.</summary>
    internal static long ReadStateId(Dictionary<string, object> message) =>
        message.TryGetValue("state_id", out object? value) && value is long id ? id : -1;
}


/// <summary>
/// The wire shape of one offered relic. Shared by the boss/Ancient screen,
/// the treasure chest and advisor mode, so a relic looks the same to the
/// policy whichever lane is asking (the option sets must not diverge --
/// see SimMapHandler.AvailableNodes for the same rule on map nodes).
/// </summary>
internal static class SimRelicOptions
{
    internal static Dictionary<string, object> Option(int index, string action, RelicModel relic) => new()
    {
        ["index"] = index,
        ["id"] = relic.Id.Entry,
        ["action"] = action,
        ["label"] = relic.Title.GetFormattedText(),
        ["description"] = relic.DynamicDescription.GetFormattedText(),
        ["enabled"] = true,
        ["relic"] = SimStateExtractor.SerializeRelic(relic),
    };
}


/// <summary>Rest site: the agent picks one of the enabled campfire options.</summary>
public class SimRestSiteRoomHandler : IRoomHandler, IHandler
{
    private static TimeSpan ProceedWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));

    public RoomType[] HandledTypes => new[] { RoomType.RestSite };

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        Logger.Log("[SimRestSite] Entering rest site");
        NRestSiteRoom room = await WaitHelper.ForNode<NRestSiteRoom>(
            SimRoomNodes.SceneRoot, SimRoomNodes.RestSiteRoomPath, ct);

        List<NRestSiteButton> buttons;
        int choice;
        do
        {
            // Legality is the option's own IsEnabled: the button stays enabled for
            // an illegal option and just ignores the click. Same filter as the
            // advisor lane (SimScreenAdvice.DescribeRestSite). Read again after a
            // potion (a heal can change what the options say).
            buttons = UiHelper.FindAll<NRestSiteButton>(room)
                .Where(b => b.Option.IsEnabled && room.Options.Contains(b.Option))
                .ToList();
            if (buttons.Count == 0)
            {
                AutoSlayLog.Warn("[SimRestSite] No enabled rest site options");
                return;
            }

            List<Dictionary<string, object>> options = buttons.Select((b, i) => new Dictionary<string, object>
            {
                ["index"] = i,
                ["id"] = b.Option.OptionId,
                ["action"] = NonCombatBridgeProtocol.RestOptionAction,
                ["label"] = b.Option.Title.GetFormattedText(),
                ["description"] = b.Option.Description.GetFormattedText(),
                ["enabled"] = b.Option.IsEnabled,
            }).ToList();

            choice = await SimAgentChoice.AskAsync(
                (_, _) => new Dictionary<string, object>
                {
                    [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.RestSiteState,
                    ["options"] = options,
                },
                options.Count, NonCombatBridgeProtocol.RestSiteState, "SimRestSite", ct, potions: true);
        } while (choice == SimAgentChoice.PotionHandled);

        NRestSiteButton chosen = buttons[choice];
        Logger.Log($"[SimRestSite] Agent chose {chosen.Option.OptionId}");
        await UiHelper.Click(chosen);

        // Some options (smith, ...) open a card screen; the overlay drain then
        // owns the rest of the room, including proceed.
        await WaitHelper.Until(() => room.ProceedButton.IsEnabled || SimRoomNodes.OverlayUp(),
            ct, ProceedWait, "Rest site proceed button never enabled");
        if (SimRoomNodes.OverlayUp())
        {
            Logger.Log("[SimRestSite] Overlay opened by the option; leaving proceed to the screen drain");
            return;
        }
        Logger.Log("[SimRestSite] Clicking proceed");
        await UiHelper.Click(room.ProceedButton);
    }
}

/// <summary>Treasure room: open the chest (automatic), agent picks the relic.</summary>
public class SimTreasureRoomHandler : IRoomHandler, IHandler
{
    private const int ChestOpenSettleMs = 1000;
    private const int RelicPickSettleMs = 500;
    private static TimeSpan ProceedWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(5));

    public RoomType[] HandledTypes => new[] { RoomType.Treasure };

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        NTreasureRoom room = await WaitHelper.ForNode<NTreasureRoom>(
            SimRoomNodes.SceneRoot, SimRoomNodes.TreasureRoomPath, ct);

        NClickableControl chest = room.GetNodeOrNull<NClickableControl>("Chest")
            ?? throw new InvalidOperationException("Treasure room has no Chest node");
        Logger.Log("[SimTreasure] Opening chest");
        await UiHelper.Click(chest);
        await Task.Delay(ChestOpenSettleMs, ct);

        List<NTreasureRoomRelicHolder> holders = UiHelper.FindAll<NTreasureRoomRelicHolder>(room)
            .Where(h => h.IsEnabled && h.Visible)
            .ToList();
        if (holders.Count > 0)
        {
            List<Dictionary<string, object>> options = holders
                .Select((h, i) => SimRelicOptions.Option(i, NonCombatBridgeProtocol.CollectTreasureAction, h.Relic.Model))
                .ToList();

            int choice;
            do
            {
                choice = await SimAgentChoice.AskAsync(
                    (_, _) => new Dictionary<string, object>
                    {
                        [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.TreasureState,
                        ["options"] = options,
                    },
                    options.Count, NonCombatBridgeProtocol.TreasureState, "SimTreasure", ct, potions: true);
            } while (choice == SimAgentChoice.PotionHandled);

            NTreasureRoomRelicHolder chosen = holders[choice];
            Logger.Log($"[SimTreasure] Agent chose relic {chosen.Relic.Model.Id.Entry}");
            await UiHelper.Click(chosen);
            await Task.Delay(RelicPickSettleMs, ct);
        }

        await WaitHelper.Until(() => room.ProceedButton.IsEnabled, ct, ProceedWait,
            "Treasure room proceed button never enabled");
        Logger.Log("[SimTreasure] Clicking proceed");
        await UiHelper.Click(room.ProceedButton);
    }
}

/// <summary>Boss / Ancient "choose a relic" overlay: the agent picks one.</summary>
public class SimChooseARelicScreenHandler : IScreenHandler, IHandler
{
    private const string ScreenName = nameof(NChooseARelicSelection);

    public Type ScreenType => typeof(NChooseARelicSelection);

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        AutoSlayLog.EnterScreen(ScreenName);
        NChooseARelicSelection screen = AutoSlayer.GetCurrentScreen<NChooseARelicSelection>();

        List<NRelicBasicHolder> holders = UiHelper.FindAll<NRelicBasicHolder>(screen)
            .Where(h => h.IsEnabled && h.Visible)
            .ToList();
        if (holders.Count == 0)
        {
            AutoSlayLog.Warn("[SimChooseARelic] No selectable relics on screen");
            return;
        }

        List<Dictionary<string, object>> options = holders
            .Select((h, i) => SimRelicOptions.Option(i, NonCombatBridgeProtocol.PickRelicAction, h.Relic.Model))
            .ToList();

        int choice = await SimAgentChoice.AskAsync(
            (_, _) => new Dictionary<string, object>
            {
                [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.BossRelicState,
                ["options"] = options,
            },
            options.Count, NonCombatBridgeProtocol.BossRelicState, "SimChooseARelic", ct);

        NRelicBasicHolder chosen = holders[choice];
        Logger.Log($"[SimChooseARelic] Agent chose relic {chosen.Relic.Model.Id.Entry}");
        await UiHelper.Click(chosen);
        AutoSlayLog.ExitScreen(ScreenName);
    }
}

public class SimShopRoomHandler : IRoomHandler, IHandler
{
    private const int MaxPurchaseAttempts = 50;
    private const int InventoryOpenSettleMs = 500;
    private const int PurchaseSettleMs = 300;
    private const int BackSettleMs = 300;

    public RoomType[] HandledTypes => new[] { RoomType.Shop };

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        NMerchantRoom room = await WaitHelper.ForNode<NMerchantRoom>(
            SimRoomNodes.SceneRoot, SimRoomNodes.MerchantRoomPath, ct);
        Logger.Log("[SimShop] Opening merchant inventory");
        room.OpenInventory();
        await Task.Delay(InventoryOpenSettleMs, ct);

        bool stoppedEarly = false;
        int attempts = 0;
        while (attempts < MaxPurchaseAttempts)
        {
            ct.ThrowIfCancellationRequested();
            attempts++;

            ShopChoice choice = await ChooseShopOption(room, ct);
            if (choice.Potion)
            {
                // Drunk or discarded: the belt (and what a potion slot sells) changed.
                continue;
            }
            if (choice.Leave)
            {
                stoppedEarly = true;
                break;
            }

            NMerchantSlot slot = choice.Slot!;
            Logger.Log($"[SimShop] Agent chose option {choice.OptionIndex}");
            // A purchase can push an overlay (e.g. a relic pickup) that only
            // completes once answered, so it must be awaited with the drain.
            Task purchase = slot.Entry.OnTryPurchaseWrapper(room.Inventory.Inventory);
            await SimAutoSlayer.AwaitWithScreenDrainAsync(purchase, ct, "shop purchase of " + ShopEntryId(slot));
            await Task.Delay(PurchaseSettleMs, ct);

            // Stay in the shop (the removal's selection, a relic's screen): drain what
            // the purchase left up, then shop on with the inventory open again.
            await SimAutoSlayer.DrainOverlaysAsync(ct, "shop purchase of " + ShopEntryId(slot));
            if (!GodotObject.IsInstanceValid(room) || !room.IsInsideTree())
            {
                Logger.Log("[SimShop] The shop closed after a purchase");
                return;
            }
            if (!room.Inventory.IsOpen)
            {
                Logger.Log("[SimShop] Reopening merchant inventory");
                room.OpenInventory();
                await Task.Delay(InventoryOpenSettleMs, ct);
            }
        }

        if (!stoppedEarly)
        {
            AutoSlayLog.Warn($"[SimShop] Hit the purchase attempt limit ({MaxPurchaseAttempts})");
        }

        // Close the shopfront first: the room keeps Proceed unavailable while
        // the inventory is open.
        NBackButton? back = UiHelper.FindFirst<NBackButton>(room);
        if (back != null)
        {
            Logger.Log("[SimShop] Closing inventory");
            await UiHelper.Click(back);
            await Task.Delay(BackSettleMs, ct);
        }

        Logger.Log("[SimShop] Leaving shop");
        await UiHelper.Click(room.ProceedButton);
    }

    /// <summary>
    /// Why the game's purchase would refuse this slot now, or null if it would go
    /// through (MerchantEntry.OnTryPurchaseWrapper: stock, gold; the removal: unused, a
    /// removable card; a potion: PotionCmd's ShouldProcurePotion hooks, then a free belt
    /// slot). The reasons are the Headless host's (RunSimulator.Shop.cs, ShopEntryReason).
    /// </summary>
    internal static string? ShopEntryReason(NMerchantSlot slot, Player? me)
    {
        MerchantEntry? entry = slot.Entry;
        if (entry == null)
            return "sold_out";
        if (entry is MerchantCardRemovalEntry removal)
        {
            if (removal.Used) return "removal_used";
        }
        else if (!entry.IsStocked) return "sold_out";
        if (!entry.EnoughGold) return "not_enough_gold";
        if (me == null) return null;
        if (entry is MerchantCardRemovalEntry && !me.Deck.Cards.Any(c => c != null && c.IsRemovable))
            return "no_removable_cards";
        if (entry is MerchantPotionEntry potion)
        {
            if (potion.Model != null && !Hook.ShouldProcurePotion(me.RunState, me.Creature.CombatState, potion.Model, me))
                return "potion_forbidden";
            if (!me.HasOpenPotionSlots) return "belt_full";
        }
        return null;
    }

    /// <summary>
    /// Option 0 leaves; option i (1..n) buys the i-th purchasable slot in inventory order
    /// (card removal included). The message also carries the whole shopfront
    /// (`shopfront`: every slot, sold or unaffordable, with why it can't be bought), and
    /// takes the potion verb outside combat (SimOutOfCombatPotions). Asked even with
    /// nothing to buy, as the Headless host shows the shop then.
    /// </summary>
    private static async Task<ShopChoice> ChooseShopOption(NMerchantRoom room, CancellationToken ct)
    {
        Player? me = LocalContext.GetMe(RunManager.Instance.DebugOnlyGetState());
        List<NMerchantSlot> all = room.Inventory.GetAllSlots().ToList();
        List<string?> reasons = all.Select(s => ShopEntryReason(s, me)).ToList();
        List<NMerchantSlot> purchasable = all.Where((_, i) => reasons[i] == null).ToList();

        var options = new List<Dictionary<string, object>>
        {
            new()
            {
                ["index"] = 0,
                ["id"] = NonCombatBridgeProtocol.LeaveShopAction,
                ["action"] = NonCombatBridgeProtocol.LeaveShopAction,
                ["label"] = "Leave shop",
                ["enabled"] = true,
            },
        };
        options.AddRange(purchasable.Select((slot, i) => ShopOption(slot, i + 1)));
        int? purgeCost = SafePurgeCost(purchasable);
        int next = 1;
        List<Dictionary<string, object>> shopfront = all.Select((slot, i) =>
        {
            Dictionary<string, object> row = ShopStockRow(slot);
            row["slot"] = i;
            row["reason"] = reasons[i]!;
            row["option_index"] = reasons[i] == null ? next++ : null!;
            row["on_sale"] = slot.Entry is MerchantCardEntry { IsOnSale: true };
            return row;
        }).ToList();

        int index = await SimAgentChoice.AskAsync(
            (_, player) =>
            {
                var body = new Dictionary<string, object>
                {
                    [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.ShopState,
                    ["options"] = options,
                    ["player_gold"] = player?.Gold ?? 0,
                    ["shopfront"] = shopfront,
                };
                if (purgeCost.HasValue)
                {
                    body["purge_cost"] = purgeCost.Value;
                }
                return body;
            },
            options.Count, NonCombatBridgeProtocol.ShopState, "SimShop", ct, negativeMeansFirst: true, potions: true);

        if (index == SimAgentChoice.PotionHandled)
            return ShopChoice.PotionActed;
        return index == 0 ? ShopChoice.LeaveShop : new ShopChoice(false, purchasable[index - 1], index);
    }

    internal static Dictionary<string, object> ShopOption(NMerchantSlot slot, int index)
    {
        string action = ShopAction(slot);
        var option = new Dictionary<string, object>
        {
            ["index"] = index,
            ["id"] = action,
            ["action"] = action,
            ["label"] = ShopEntryId(slot),
            ["description"] = $"Cost: {slot.Entry.Cost}",
            ["enabled"] = slot.Entry.IsStocked && slot.Entry.EnoughGold,
            ["price"] = slot.Entry.Cost,
        };
        Dictionary<string, object?>? detail = ShopEntryDetail(slot);
        if (detail != null)
        {
            option["item"] = detail;
        }
        return option;
    }

    /// <summary>
    /// Display row for one merchant slot, stocked or not, affordable or not.
    /// The workbench mirror renders the whole shopfront from these; the
    /// advice ``options`` stay purchasable-only (see ShopOption callers).
    /// </summary>
    internal static Dictionary<string, object> ShopStockRow(NMerchantSlot slot)
    {
        var row = new Dictionary<string, object>
        {
            ["kind"] = ShopAction(slot),
            ["label"] = ShopEntryId(slot),
            ["price"] = slot.Entry.Cost,
            ["stocked"] = slot.Entry.IsStocked,
            ["affordable"] = slot.Entry.EnoughGold,
        };
        Dictionary<string, object?>? detail = ShopEntryDetail(slot);
        if (detail != null)
            row["item"] = detail;
        return row;
    }

    private static Dictionary<string, object?>? ShopEntryDetail(NMerchantSlot slot)
    {
        try
        {
            return slot.Entry switch
            {
                MerchantCardEntry cardEntry when cardEntry.CreationResult?.Card != null =>
                    SimStateExtractor.SerializeCard(cardEntry.CreationResult.Card),
                MerchantRelicEntry relicEntry when relicEntry.Model != null =>
                    SimStateExtractor.SerializeRelic(relicEntry.Model),
                MerchantPotionEntry potionEntry when potionEntry.Model != null =>
                    SimStateExtractor.SerializePotionSlot(potionEntry.Model, -1),
                _ => null,
            };
        }
        catch (Exception ex)
        {
            AutoSlayLog.Warn($"[SimShop] Error building shop entry detail: {ex.Message}");
            return null;
        }
    }

    internal static int? SafePurgeCost(List<NMerchantSlot> purchasableSlots)
    {
        try
        {
            NMerchantCardRemoval? removal = purchasableSlots.OfType<NMerchantCardRemoval>().FirstOrDefault();
            return removal?.Entry.Cost;
        }
        catch
        {
            return null;
        }
    }

    /// <summary>Descriptive wire action, keyed by the slot NODE type.</summary>
    private static string ShopAction(NMerchantSlot slot) => slot switch
    {
        NMerchantCard => NonCombatBridgeProtocol.BuyCardAction,
        NMerchantRelic => NonCombatBridgeProtocol.BuyRelicAction,
        NMerchantPotion => NonCombatBridgeProtocol.BuyPotionAction,
        NMerchantCardRemoval => NonCombatBridgeProtocol.RemoveCardAction,
        _ => NonCombatBridgeProtocol.BuyItemAction,
    };

    /// <summary>Human/wire label of what a slot sells, keyed by the ENTRY type.</summary>
    internal static string ShopEntryId(NMerchantSlot slot) => slot.Entry switch
    {
        MerchantCardEntry card => card.CreationResult?.Card?.Id.Entry ?? "card",
        MerchantRelicEntry relic => relic.Model?.Id.Entry ?? "relic",
        MerchantPotionEntry potion => potion.Model?.Id.Entry ?? "potion",
        MerchantCardRemovalEntry => NonCombatBridgeProtocol.RemoveCardAction,
        _ => slot.Entry?.GetType().Name ?? "unknown",
    };

    /// <summary>Outcome of one shop decision: leave, or buy <see cref="Slot"/>.</summary>
    private readonly record struct ShopChoice(bool Leave, NMerchantSlot? Slot, int OptionIndex, bool Potion = false)
    {
        public static ShopChoice LeaveShop => new(true, null, 0);

        /// <summary>A potion was drunk or discarded: ask again.</summary>
        public static ShopChoice PotionActed => new(false, null, -1, true);
    }

}

public class SimEventRoomHandler : IRoomHandler, IHandler
{
    private const int MaxDecisionIterations = 50;
    private const int PostCombatSettleMs = 500;
    private static TimeSpan ProceedCloseWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(5));
    private static TimeSpan ChoiceResultWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(5));
    private static TimeSpan OptionsLoadWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(30));
    private const int OptionsWaitReportEveryPolls = 50;
    private static TimeSpan FakeMerchantProceedWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));

    private const string AncientDialogueHitbox = "%DialogueHitbox";
    private const int AncientMaxClicks = 50;
    private const int AncientHitboxPollMs = 100;
    private const int AncientClickSettleMs = 500;
    private static TimeSpan AncientOptionsWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));

    public RoomType[] HandledTypes => new[] { RoomType.Event };

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        Logger.Log("[SimEvent] Entering event room");
        Node room = await WaitHelper.ForNode<Node>(SimRoomNodes.SceneRoot, SimRoomNodes.EventRoomPath, ct);

        NAncientEventLayout? ancient = UiHelper.FindFirst<NAncientEventLayout>(room);
        if (ancient != null)
        {
            await ClickThroughAncientDialogueAsync(ancient, ct);
        }
        else
        {
            NFakeMerchant? fakeMerchant = UiHelper.FindFirst<NFakeMerchant>(room);
            if (fakeMerchant != null)
            {
                // A shop-shaped event with nothing to decide: just leave.
                await LeaveFakeMerchantAsync(fakeMerchant, ct);
                return;
            }
            await WaitForAnyOptionAsync(room, ct);
        }

        int iterations = await RunDecisionLoopAsync(room, random, ct);
        if (iterations >= MaxDecisionIterations)
        {
            AutoSlayLog.Warn($"[SimEvent] Hit the decision iteration cap ({MaxDecisionIterations})");
        }
        Logger.Log("[SimEvent] Event finished");
    }

    /// <summary>
    /// One agent decision per page until the event leaves the room, opens an
    /// overlay, offers nothing more, or the cap is reached. Returns the number
    /// of counted iterations.
    /// </summary>
    private async Task<int> RunDecisionLoopAsync(Node room, Rng random, CancellationToken ct)
    {
        int iterations = 0;
        while (iterations < MaxDecisionIterations)
        {
            ct.ThrowIfCancellationRequested();

            if (!SimRoomNodes.IsLive(room))
            {
                Node? resumed = await ResumeAfterEventCombatAsync(random, ct);
                if (resumed == null)
                {
                    break;
                }
                room = resumed;
            }

            // Lethal options stay in: the agent decides, informed by will_kill_player.
            List<NEventOptionButton> options = SimRoomNodes.UnlockedEventOptions(room);
            if (options.Count == 0)
            {
                break;
            }

            NEventOptionButton? picked = await ChooseEventOption(options, ct);
            if (picked == null)
            {
                // A potion was drunk or discarded: read the page again (not an iteration).
                continue;
            }
            NEventOptionButton chosen = picked;
            string textKey = chosen.Option.TextKey ?? "";
            bool isProceed = chosen.Option.IsProceed;
            Logger.Log($"[SimEvent] Agent chose option {textKey}");

            // The agent may think for a long time; the page can be gone by now.
            if (NEventRoom.Instance == null)
            {
                Logger.Log("[SimEvent] Event room is transitioning; re-checking");
                iterations++;
                continue;
            }

            try
            {
                await UiHelper.Click(chosen);
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                AutoSlayLog.Warn($"[SimEvent] Clicking option {textKey} failed " +
                                 $"({ex.GetType().Name}: {ex.Message}); re-checking room");
                iterations++;
                continue;
            }

            if (isProceed)
            {
                Logger.Log("[SimEvent] Proceed option clicked; waiting for the room to close");
                // CLEANROOM2-Q5: a timeout here throws, like every WaitHelper.Until wait.
                Node proceededRoom = room;
                await WaitHelper.Until(() => !SimRoomNodes.IsLive(proceededRoom) || SimRoomNodes.MapOpen(),
                    ct, ProceedCloseWait, "Event room did not close after proceed");
                break;
            }

            Node currentRoom = room;
            await WaitHelper.Until(() =>
                    SimRoomNodes.OverlayUp() ||
                    SimRoomNodes.MapOpen() ||
                    !SimRoomNodes.IsLive(currentRoom) ||
                    SimRoomNodes.UnlockedEventOptions(currentRoom).Count > 0,
                ct, ChoiceResultWait, "Event showed no result after the chosen option");

            if (SimRoomNodes.OverlayUp())
            {
                Logger.Log("[SimEvent] Option opened an overlay; the overlay pump takes over");
                break;
            }

            iterations++;
        }
        return iterations;
    }

    /// <summary>
    /// An event that started a fight leaves the event node behind; play the
    /// combat, then re-attach to the event room if it is still there.
    /// </summary>
    private static async Task<Node?> ResumeAfterEventCombatAsync(Rng random, CancellationToken ct)
    {
        RunState? runState = RunManager.Instance.DebugOnlyGetState();
        bool eventCombat = runState != null &&
                           runState.CurrentRoomCount > 1 &&
                           runState.BaseRoom?.RoomType == RoomType.Event;
        if (!eventCombat)
        {
            Logger.Log("[SimEvent] Event room is gone and no event combat is running");
            return null;
        }

        Logger.Log("[SimEvent] Event started a combat; handling it");
        await new SimCombatHandler().HandleAsync(random, ct);
        Logger.Log("[SimEvent] Event combat finished");

        Node? room = SimRoomNodes.SceneRoot.GetNodeOrNull<Node>(SimRoomNodes.EventRoomPath);
        if (!SimRoomNodes.IsLive(room))
        {
            Logger.Log("[SimEvent] Event ended after its combat");
            return null;
        }

        await Task.Delay(PostCombatSettleMs, ct);
        return room;
    }

    private static async Task WaitForAnyOptionAsync(Node room, CancellationToken ct)
    {
        int polls = 0;
        await WaitHelper.Until(() =>
        {
            if (++polls % OptionsWaitReportEveryPolls == 0)
            {
                AutoSlayLog.Info($"[SimEvent] Still waiting for event options (room children: {room.GetChildCount()})");
                SimAutoSlayer.CurrentWatchdog?.Reset("Waiting for event options to load");
            }
            return UiHelper.FindAll<NEventOptionButton>(room).Count > 0;
        }, ct, OptionsLoadWait, "Event options never appeared");
    }

    private static async Task LeaveFakeMerchantAsync(NFakeMerchant merchant, CancellationToken ct)
    {
        Logger.Log("[SimEvent] FakeMerchant event detected; leaving");
        NProceedButton? proceed = null;
        await WaitHelper.Until(() =>
        {
            proceed = UiHelper.FindAll<NProceedButton>(merchant).FirstOrDefault(b => b.IsEnabled && b.Visible);
            return proceed != null;
        }, ct, FakeMerchantProceedWait, "FakeMerchant proceed button never became available");
        await UiHelper.Click(proceed!);
        Logger.Log("[SimEvent] Event finished (FakeMerchant)");
    }

    private static bool HasReadyOption(Node layout) =>
        UiHelper.FindAll<NEventOptionButton>(layout).Any(b => b.IsEnabled && !b.Option.IsLocked);

    /// <summary>
    /// Advances an Ancient's dialogue until its options are offered. Not a
    /// decision: every line has to be clicked through anyway.
    /// </summary>
    private static async Task ClickThroughAncientDialogueAsync(NAncientEventLayout layout, CancellationToken ct)
    {
        int clicks = 0;
        while (clicks < AncientMaxClicks)
        {
            if (!GodotObject.IsInstanceValid(layout))
            {
                break;
            }

            int ready = UiHelper.FindAll<NEventOptionButton>(layout).Count(b => b.IsEnabled && !b.Option.IsLocked);
            if (ready > 0)
            {
                Logger.Log($"[SimEvent] Ancient dialogue done; {ready} option(s) ready");
                break;
            }

            NButton? hitbox = layout.GetNodeOrNull<NButton>(AncientDialogueHitbox);
            if (hitbox == null || !hitbox.Visible || !hitbox.IsEnabled)
            {
                // Not clickable yet (line still animating): not a click. Bounded
                // by the run watchdog like WaitHelper's waits: a hitbox that never
                // becomes clickable (and no options) dies on the stall limit
                // (AutoSlayTimeoutException) instead of polling forever.
                ct.ThrowIfCancellationRequested();
                SimAutoSlayer.CurrentWatchdog?.Check();
                await Task.Delay(AncientHitboxPollMs, ct);
                continue;
            }

            clicks++;
            Logger.Log($"[SimEvent] Advancing Ancient dialogue (click {clicks})");
            SimAutoSlayer.CurrentWatchdog?.Reset("Clicking through Ancient dialogue");
            hitbox.EmitSignal(NClickableControl.SignalName.Released, hitbox);
            await Task.Delay(AncientClickSettleMs, ct);
        }

        await WaitHelper.Until(() => GodotObject.IsInstanceValid(layout) && HasReadyOption(layout),
            ct, AncientOptionsWait, "Ancient event options never became ready");
    }

    /// <summary>
    /// The game's own <c>EventOption.WillKillPlayer</c> verdict for one option,
    /// or <c>null</c> when the predicate threw.
    /// </summary>
    /// <remarks>
    /// Deliberately tri-state rather than a plain <c>bool</c>. A missing predicate
    /// or a missing Owner really is "not flagged" — <c>EventRoomHandler.cs:97</c>
    /// keeps exactly those options — so <c>false</c> is faithful there. A probe that
    /// THREW is different: emitting <c>false</c> would tell the Python guard that the
    /// world said SAFE, when in fact nothing was computed. <c>null</c> makes it fall
    /// back to its HP-cost parse instead. Same contract as the headless arm's probe
    /// in <c>RunSimulator</c>, so both bridges speak one dialect.
    /// </remarks>
    private static bool? SafeWillKillPlayer(NEventOptionButton option)
    {
        var predicate = option.Option.WillKillPlayer;
        var owner = option.Event.Owner;
        if (predicate == null || owner == null) return false;
        try { return predicate(owner); }
        catch (Exception ex)
        {
            AutoSlayLog.Warn(
                $"WillKillPlayer probe ({option.Option.TextKey}) failed, reporting unknown: "
                + $"{ex.GetType().Name}: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// Asks the agent to pick an event option. An out-of-range index, or an
    /// option that is locked by the time the reply arrives, gets invalid_index
    /// and a re-ask. Null after a potion reply (the page is read again).
    /// </summary>
    private static async Task<NEventOptionButton?> ChooseEventOption(List<NEventOptionButton> options, CancellationToken ct)
    {
        List<Dictionary<string, object?>> payload = EventOptions(options);
        int index = await SimAgentChoice.AskAsync(
            (_, _) => new Dictionary<string, object>
            {
                [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.EventState,
                ["options"] = payload,
            },
            options.Count, NonCombatBridgeProtocol.EventState, "SimEvent", ct,
            rejectAtReply: i => GodotObject.IsInstanceValid(options[i]) && options[i].Option.IsLocked
                ? $"index {i} is not enabled (option locked)"
                : null,
            potions: true);
        return index == SimAgentChoice.PotionHandled ? null : options[index];
    }

    /// <summary>
    /// The event options as sent on the wire. <c>text_key</c> is the option's
    /// loc-key prefix; its tail is the enum id the run policy was trained on
    /// (e.g. Neow's options). <c>relic_id</c> marks relic-granting options.
    /// <c>will_kill_player</c> is the game's own lethality verdict, where
    /// null means "could not be computed", never "safe".
    /// </summary>
    internal static List<Dictionary<string, object?>> EventOptions(List<NEventOptionButton> options) =>
        options.Select((button, i) => new Dictionary<string, object?>
        {
            ["index"] = i,
            ["id"] = NonCombatBridgeProtocol.EventChoiceAction,
            ["action"] = NonCombatBridgeProtocol.EventChoiceAction,
            ["label"] = button.Option.Title.GetFormattedText(),
            ["description"] = button.Option.Description.GetFormattedText(),
            ["enabled"] = !button.Option.IsLocked,
            ["event_id"] = button.Event.Id.Entry,
            ["text_key"] = button.Option.TextKey ?? "",
            ["relic_id"] = button.Option.Relic?.Id.Entry ?? "",
            ["will_kill_player"] = SafeWillKillPlayer(button),
        }).ToList();
}

/// <summary>
/// Whether the player can take a potion right now. A full belt is not the
/// only refusal: PotionCmd.TryToProcure first asks every hook listener's
/// ShouldProcurePotion (v0.107.1), and the Sozu relic says no for its owner.
/// Offering such a potion makes the click fail silently -- the shop keeps the
/// item and the gold, the reward keeps its button -- so the same choice comes
/// round again until the loop guard stops autoplay.
/// </summary>
internal static class SimPotionRules
{
    /// <summary>
    /// True when <paramref name="me"/> would get <paramref name="potion"/>:
    /// a free belt slot and no hook refusing it. A null potion is judged on
    /// the belt alone.
    /// </summary>
    public static bool CanProcure(Player me, PotionModel? potion)
    {
        if (!me.HasOpenPotionSlots)
            return false;
        return potion == null
            || Hook.ShouldProcurePotion(me.RunState, me.Creature.CombatState, potion, me);
    }
}
