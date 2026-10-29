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

            // Nothing left to take: leaving is the only move, so it is not asked.
            if (claimable.Count == 0)
            {
                if (proceed != null)
                    await ProceedAndCloseAsync(screen, proceed, ct);
                AutoSlayLog.ExitScreen(LogName);
                return;
            }

            RewardScreenChoice choice = await ChooseRewardScreenOption(claimable, proceed, ct);

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
        List<NRewardButton> buttons, NProceedButton? proceed, CancellationToken ct)
    {
        bool proceedOffered = false;

        int index = await SimAgentChoice.AskAsync(
            (_, _) =>
            {
                List<Dictionary<string, object>> options = buttons.Select((b, i) => RewardOption(b, i)).ToList();
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
                : null);

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
    private readonly record struct RewardScreenChoice(bool ShouldProceed, NRewardButton? Button)
    {
        public static RewardScreenChoice Proceed => new(true, null);

        public static RewardScreenChoice Pick(NRewardButton button) => new(false, button);
    }
}
