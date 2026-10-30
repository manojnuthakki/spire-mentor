// SimRewardsScreenHandler.cs -- post-combat/event rewards screen, decided by the agent.
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
using MegaCrit.Sts2.Core.Hooks;
using MegaCrit.Sts2.Core.Nodes.CommonUi;
using MegaCrit.Sts2.Core.Nodes.Rewards;
using MegaCrit.Sts2.Core.Nodes.Screens;
using MegaCrit.Sts2.Core.Nodes.Screens.Map;
using MegaCrit.Sts2.Core.Nodes.Screens.Overlays;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Rewards;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

public class SimRewardsScreenHandler : IScreenHandler, IHandler
{
    private const string LogName = nameof(NRewardsScreen);
    private const string UnknownRewardLabel = "unknown";
    private const string GoldLabelPrefix = "gold:";

    private const int RewardClickSettleMs = 500;
    private static TimeSpan ProceedCloseWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));

    public Type ScreenType => typeof(NRewardsScreen);

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        AutoSlayLog.EnterScreen(LogName);
        NRewardsScreen screen = AutoSlayer.GetCurrentScreen<NRewardsScreen>();
        var claimed = new HashSet<NRewardButton>(ReferenceEqualityComparer.Instance);

        while (true)
        {
            ct.ThrowIfCancellationRequested();

            List<NRewardButton> claimable = ClaimableRewards(screen, claimed);
            NProceedButton? proceed = UiHelper.FindFirst<NProceedButton>(screen);
            // Potions with no room on the belt: shown unavailable while a potion can be drunk
            // or discarded to make room (SimOutOfCombatPotions).
            List<NRewardButton> waiting = PotionsWithoutRoom(screen, claimed);
            Player? me = LocalContext.GetMe(RunManager.Instance.DebugOnlyGetState());
            if (!SimOutOfCombatPotions.AnyActionNow(me))
            {
                waiting.Clear();
            }

            // Nothing left to take: leaving is the only move, so it is not asked.
            if (claimable.Count == 0 && waiting.Count == 0)
            {
                if (proceed != null)
                    await ProceedAndCloseAsync(screen, proceed, ct);
                AutoSlayLog.ExitScreen(LogName);
                return;
            }

            var shown = new HashSet<NRewardButton>(claimable.Concat(waiting), ReferenceEqualityComparer.Instance);
            List<NRewardButton> buttons = UiHelper.FindAll<NRewardButton>(screen).Where(shown.Contains).ToList();
            RewardScreenChoice choice = await ChooseRewardScreenOption(
                buttons, new HashSet<NRewardButton>(claimable, ReferenceEqualityComparer.Instance), proceed, ct);

            if (choice.PotionActed)
            {
                continue;
            }

            if (choice.ShouldProceed)
            {
                if (proceed != null)
                    await ProceedAndCloseAsync(screen, proceed, ct);
                AutoSlayLog.ExitScreen(LogName);
                return;
            }

            NRewardButton button = choice.Button!;
            claimed.Add(button);
            AutoSlayLog.Action($"Clicking reward button: {RewardLabel(button)}");
            await UiHelper.Click(button);
            await Task.Delay(RewardClickSettleMs, ct);

            if (!IsTopScreen(screen))
            {
                AutoSlayLog.Info("Child screen opened, returning to drain loop");
                AutoSlayLog.ExitScreen(LogName);
                return;
            }
        }
    }

    /// <summary>
    /// Enabled reward buttons not yet clicked by this handler. Potions are
    /// only claimable when the player can take them (free slot, no Sozu).
    /// </summary>
    private static List<NRewardButton> ClaimableRewards(
        NRewardsScreen screen, HashSet<NRewardButton> claimed)
    {
        Player? me = LocalContext.GetMe(RunManager.Instance.DebugOnlyGetState());
        return UiHelper.FindAll<NRewardButton>(screen)
            .Where(b => b.IsEnabled && !claimed.Contains(b))
            .Where(b => b.Reward is not PotionReward pr || (me != null && SimPotionRules.CanProcure(me, pr.Potion)))
            .ToList();
    }

    /// <summary>
    /// Enabled potion rewards not yet clicked that only a full belt keeps from the player
    /// (`belt_full`: a drink or a discard makes room). One the run refuses (Sozu) is not
    /// among them: no discard would make it claimable, and it is not shown.
    /// </summary>
    private static List<NRewardButton> PotionsWithoutRoom(NRewardsScreen screen, HashSet<NRewardButton> claimed)
    {
        Player? me = LocalContext.GetMe(RunManager.Instance.DebugOnlyGetState());
        return UiHelper.FindAll<NRewardButton>(screen)
            .Where(b => b.IsEnabled && !claimed.Contains(b))
            .Where(b => b.Reward is PotionReward pr && me != null && !me.HasOpenPotionSlots
                        && (pr.Potion == null || Hook.ShouldProcurePotion(me.RunState, me.Creature.CombatState, pr.Potion, me)))
            .ToList();
    }

    private static bool IsTopScreen(NRewardsScreen screen)
    {
        NOverlayStack? stack = NOverlayStack.Instance;
        return stack != null && stack.ScreenCount > 0 && ReferenceEquals(stack.Peek(), screen);
    }

    private static bool MapOpen() => NMapScreen.Instance?.IsOpen ?? false;

    private static async Task ProceedAndCloseAsync(
        NRewardsScreen screen, NProceedButton proceed, CancellationToken ct)
    {
        AutoSlayLog.Action("Rewards: clicking proceed");
        await UiHelper.Click(proceed);
        await WaitHelper.Until(
            () => !GodotObject.IsInstanceValid(screen) || !IsTopScreen(screen) || MapOpen(),
            ct, ProceedCloseWait, "Rewards screen did not close after proceed");
    }

    /// <summary>
    /// Asks the agent which reward to take. Index i &lt; count picks reward i;
    /// index == count picks the trailing "proceed" option, which is only
    /// offered while the proceed button is enabled. Proceed is re-checked for
    /// every ask and again when the reply arrives.
    /// </summary>
    private static async Task<RewardScreenChoice> ChooseRewardScreenOption(
        List<NRewardButton> buttons, HashSet<NRewardButton> claimable, NProceedButton? proceed, CancellationToken ct)
    {
        bool proceedOffered = false;

        int index = await SimAgentChoice.AskAsync(
            (_, _) =>
            {
                List<Dictionary<string, object>> options = buttons.Select((b, i) => RewardOption(b, i)).ToList();
                foreach ((Dictionary<string, object> option, NRewardButton b) in options.Zip(buttons))
                {
                    option["enabled"] = claimable.Contains(b);
                    if (!claimable.Contains(b))
                    {
                        option["reason"] = "belt_full";  // PotionsWithoutRoom's only kind
                    }
                }
                proceedOffered = ProceedEnabled(proceed);
                if (proceedOffered)
                {
                    options.Add(new Dictionary<string, object>
                    {
                        ["index"] = buttons.Count,
                        ["id"] = NonCombatBridgeProtocol.ProceedAction,
                        ["action"] = NonCombatBridgeProtocol.ProceedAction,
                        ["label"] = NonCombatBridgeProtocol.ProceedAction,
                        ["enabled"] = true,
                    });
                }
                return new Dictionary<string, object>
                {
                    [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.RewardScreenState,
                    ["options"] = options,
                };
            },
            proceed != null ? buttons.Count + 1 : buttons.Count,
            NonCombatBridgeProtocol.RewardScreenState, "SimRewards", ct,
            rejectAtReply: i => i == buttons.Count && !(proceedOffered && ProceedEnabled(proceed))
                ? $"index {i} (proceed) is not enabled"
                : i < buttons.Count && !claimable.Contains(buttons[i])
                    ? $"index {i} is not enabled (no room for the potion)"
                    : null,
            potions: true);

        if (index == SimAgentChoice.PotionHandled)
            return RewardScreenChoice.Potion;
        return index < buttons.Count ? RewardScreenChoice.Pick(buttons[index]) : RewardScreenChoice.Proceed;
    }

    private static bool ProceedEnabled(NProceedButton? proceed) =>
        proceed != null && GodotObject.IsInstanceValid(proceed) && proceed.IsEnabled;

    private static Dictionary<string, object> RewardOption(NRewardButton button, int index)
    {
        string label = RewardLabel(button);
        var option = new Dictionary<string, object>
        {
            ["index"] = index,
            ["id"] = label,
            ["action"] = NonCombatBridgeProtocol.PickRewardAction,
            ["label"] = label,
            ["description"] = RewardDescription(button),
            ["enabled"] = button.IsEnabled,
        };
        object? detail = RewardDetail(button);
        if (detail != null)
        {
            option["reward"] = detail;
        }
        return option;
    }

    private static object? RewardDetail(NRewardButton button)
    {
        try
        {
            return button.Reward switch
            {
                CardReward cardReward => cardReward.Cards.Select(c => SimStateExtractor.SerializeCard(c)).ToList(),
                PotionReward { Potion: not null } potionReward => SimStateExtractor.SerializePotionSlot(potionReward.Potion, -1),
                RelicReward { Relic: not null } relicReward => SimStateExtractor.SerializeRelic(relicReward.Relic),
                GoldReward goldReward => new Dictionary<string, object?> { ["amount"] = goldReward.Amount },
                _ => null,
            };
        }
        catch (Exception ex)
        {
            AutoSlayLog.Warn($"[SimRewards] Error building reward detail: {ex.Message}");
            return null;
        }
    }


    /// <summary>
    /// Wire id of a reward: "gold:N" (no space) for gold, otherwise the
    /// reward's runtime type name (CardReward, PotionReward, RelicReward, ...).
    /// The Python driver matches these literally.
    /// </summary>
    private static string RewardLabel(NRewardButton button) => button.Reward switch
    {
        null => UnknownRewardLabel,
        GoldReward gold => GoldLabelPrefix + gold.Amount,
        Reward reward => reward.GetType().Name,
    };

    private static string RewardDescription(NRewardButton button)
    {
        try
        {
            return button.Reward?.Description.GetFormattedText() ?? "";
        }
        catch (Exception)
        {
            return "";
        }
    }

    /// <summary>Either "click proceed" or "claim this reward button".</summary>
    private readonly record struct RewardScreenChoice(bool ShouldProceed, NRewardButton? Button, bool PotionActed = false)
    {
        public static RewardScreenChoice Proceed => new(true, null);

        public static RewardScreenChoice Pick(NRewardButton button) => new(false, button);

        /// <summary>A potion was drunk or discarded: read the screen again.</summary>
        public static RewardScreenChoice Potion => new(false, null, true);
    }
}
