// sts2-agent fork: the rewards screen after combat, in agent mode.
//
// Upstream auto-claims gold, potions, relics and special cards after combat (a potion
// that doesn't fit is lost), shows only the first card reward, and drops card-removal
// and linked rewards. In agent mode the host instead returns a `rewards` decision that
// lists every reward the game generated, and the client claims them one at a time:
//
//   rewards:     claim_reward{reward_index}, proceed (leave the rest), use_potion,
//                discard_potion
//   card_reward: select_card_reward{card_index}, skip_card_reward (the Skip
//                alternative), choose_card_reward_alternative{option_id}
//
// Claims go through the game's own path (RewardsSetSynchronizer.SelectLocalReward ->
// Reward.SelectUnsynchronized), on the thread pool like an event option, so a card
// reward reaches the client through ICardSelector.GetSelectedCardReward with its
// alternatives (Skip, REROLL, SACRIFICE), and a card-removal reward through the
// ordinary card selection. A potion reward with a full belt is listed as unavailable
// (`belt_full`) and stays on the screen, as in the game; discarding a potion makes it
// claimable. `proceed` skips whatever is left (Reward.OnSkipped), as leaving the
// screen does. With agent mode off none of this runs.

using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Rewards;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Runs;

namespace Sts2Headless;

public partial class RunSimulator
{
    /// <summary>The rewards set of the combat just won (agent mode), until proceed.</summary>
    private RewardsSet? _agentRewardsSet;

    /// <summary>The rewards listed in the last `rewards` decision, by reward_index.</summary>
    private List<Reward> _agentRewardsShown = new();

    /// <summary>
    /// A claim suspended at the selection it opened (a card reward, a card removal).
    /// Driven forward by the commands that resolve that selection.
    /// </summary>
    private Task? _pendingRewardClaimTask;

    /// <summary>Forget the previous room's rewards (a new room is being entered).</summary>
    private void ResetAgentRewards()
    {
        _agentRewardsSet = null;
        _agentRewardsShown = new();
        _pendingRewardClaimTask = null;
    }

    /// <summary>
    /// After combat in agent mode: generate the room's rewards once, the way
    /// CombatRoom.OfferRoomEndRewards does, register them with the game's rewards
    /// synchronizer (as RewardsSet.Offer does), and show the screen.
    /// </summary>
    private Dictionary<string, object?> AgentRewardsState(Player player, CombatRoom combatRoom)
    {
        if (_agentRewardsSet == null)
        {
            _goldBeforeCombat = player.Gold;
            var set = new RewardsSet(player).WithRewardsFromRoom(combatRoom);
            try
            {
                set.GenerateWithoutOffering().GetAwaiter().GetResult();
                _syncCtx.Pump();
            }
            catch (Exception ex) { LogException("Generate rewards", ex); }
            _ = RunManager.Instance.RewardsSetSynchronizer.BeginRewardsSet(set);
            _agentRewardsSet = set;
        }
        return RewardsState(player);
    }

    private Dictionary<string, object?> RewardsState(Player player)
    {
        _agentRewardsShown = _agentRewardsSet!.Rewards.Where(r => !r.SuccessfullySelected).ToList();
        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "rewards",
            ["context"] = RunContext(),
            ["rewards"] = _agentRewardsShown.Select((r, i) => RewardDict(r, player, i)).ToList(),
            ["gold_earned"] = player.Gold - _goldBeforeCombat,
            ["player"] = PlayerSummary(player),
        };
    }

    private Dictionary<string, object?> RewardDict(Reward reward, Player player, int? index)
    {
        var d = new Dictionary<string, object?> { ["index"] = index, ["available"] = true, ["reason"] = null };
        switch (reward)
        {
            case GoldReward gold:
                d["type"] = "gold";
                d["amount"] = gold.Amount;
                break;
            case PotionReward potion:
                d["type"] = "potion";
                d["id"] = potion.Potion?.Id.ToString();
                d["name"] = potion.Potion == null ? null : _loc.Potion(potion.Potion.Id.Entry);
                d["description"] = potion.Potion == null ? null : _loc.Bilingual("potions", potion.Potion.Id.Entry + ".description");
                // The game refuses the claim (PotionProcureFailureReason.TooFull) and keeps
                // the reward on the screen.
                if (!player.HasOpenPotionSlots)
                {
                    d["available"] = false;
                    d["reason"] = "belt_full";
                }
                break;
            case RelicReward relic:
                d["type"] = "relic";
                d["id"] = relic.Relic?.Id.ToString();
                d["name"] = relic.Relic == null ? null : _loc.Relic(relic.Relic.Id.Entry);
                d["description"] = relic.Relic == null ? null : _loc.Bilingual("relics", relic.Relic.Id.Entry + ".description");
                break;
            case CardReward:
                // The offered cards are seen only once the reward is opened (claimed).
                d["type"] = "card";
                break;
            case SpecialCardReward special:
                d["type"] = "special_card";
                var card = SpecialCard(special);
                d["id"] = card?.Id.ToString();
                d["name"] = card == null ? null : _loc.Card(card.Id.Entry);
                d["description"] = card == null ? null : _loc.Bilingual("cards", card.Id.Entry + ".description");
                break;
            case CardRemovalReward:
                d["type"] = "card_removal";
                break;
            case LinkedRewardSet linked:
                // Claiming one reward of the set removes the others. v0.107.1 never builds one.
                d["type"] = "linked";
                d["rewards"] = linked.Rewards.Select(r => RewardDict(r, player, null)).ToList();
                break;
            default:
                d["type"] = reward.GetType().Name;
                break;
        }
        return d;
    }

    /// <summary>SpecialCardReward keeps its card private.</summary>
    private static MegaCrit.Sts2.Core.Models.CardModel? SpecialCard(SpecialCardReward reward) =>
        HarmonyLib.AccessTools.Field(typeof(SpecialCardReward), "_card")?.GetValue(reward)
            as MegaCrit.Sts2.Core.Models.CardModel;

    /// <summary>
    /// Claim one reward. A card reward or card removal suspends at its selection;
    /// the claim is stashed and finished by the command that answers it.
    /// </summary>
    private Dictionary<string, object?> DoClaimReward(Player player, Dictionary<string, object?>? args)
    {
        TryGetInt(args, "reward_index", out var index);  // checked by ValidateClaimReward
        var reward = _agentRewardsShown[index];
        Log($"Claiming reward {index}: {reward.GetType().Name}");
        var sync = RunManager.Instance.RewardsSetSynchronizer;
        var task = Task.Run(() => sync.SelectLocalReward(reward));
        WaitUntilOptionTaskSettles(task, "reward claim or the selection it opens");
        if (!task.IsCompleted && AnySelectionPending())
        {
            _pendingRewardClaimTask = task;
            WaitForActionExecutor();
            return DetectDecisionPoint();
        }
        FinishRewardClaim(task);
        return DetectDecisionPoint();
    }

    /// <summary>
    /// Drive a stashed claim after its selection was answered: it either completes or
    /// suspends again (a REROLL shows new cards; some relics allow more than one pick).
    /// </summary>
    private void DriveRewardClaimTask()
    {
        var task = _pendingRewardClaimTask;
        _pendingRewardClaimTask = null;
        if (task == null) return;
        WaitUntilOptionTaskSettles(task, "reward claim (until next selection or completion)");
        if (!task.IsCompleted && AnySelectionPending())
        {
            _pendingRewardClaimTask = task;
            WaitForActionExecutor();
            return;
        }
        FinishRewardClaim(task);
    }

    private void FinishRewardClaim(Task task)
    {
        // Not settling was already recorded as an engine error by WaitUntilOptionTaskSettles.
        if (task.IsFaulted) LogException("Claim reward", task.Exception!);
        WaitForActionExecutor();
        _syncCtx.Pump();
        WaitForActionExecutor();
    }

    /// <summary>A stashed claim with no selection left to resume it can never finish.</summary>
    private void ReleaseStrandedRewardClaimTask(string why)
    {
        if (_pendingRewardClaimTask == null || _pendingRewardClaimTask.IsCompleted) return;
        Log($"[WARN] Abandoning stashed reward claim ({why})");
        EngineErrors.Record("host", $"abandoned a stashed reward claim ({why}); its remaining effects will not run");
        _pendingRewardClaimTask = null;
    }

    /// <summary>Leave the rewards screen: skip what is left, then continue as upstream does.</summary>
    private Dictionary<string, object?> ProceedFromAgentRewards()
    {
        var sync = RunManager.Instance.RewardsSetSynchronizer;
        if (_agentRewardsSet != null && !sync.IsRewardsSetCompleted(_agentRewardsSet))
            sync.SkipLocalRewardsSet();
        _agentRewardsSet = null;
        _agentRewardsShown = new();
        _rewardsProcessed = true;
        return DetectDecisionPoint();
    }

    /// <summary>Card-reward alternatives as the client sees them (agent mode).</summary>
    private List<Dictionary<string, object?>> CardRewardAlternativesDict() =>
        (_cardSelector.PendingRewardAlternatives ?? new()).Select((a, i) => new Dictionary<string, object?>
        {
            ["index"] = i,
            ["id"] = a.OptionId,
            // What choosing it does: the reward stays claimable (Skip), is used up
            // (SACRIFICE), or the screen stays open (REROLL).
            ["after_selected"] = a.AfterSelected switch
            {
                MegaCrit.Sts2.Core.Entities.Rewards.PostAlternateCardRewardAction.EndSelectionAndDoNotCompleteReward => "keep_reward",
                MegaCrit.Sts2.Core.Entities.Rewards.PostAlternateCardRewardAction.EndSelectionAndCompleteReward => "complete_reward",
                _ => "stay",
            },
        }).ToList();

    private Dictionary<string, object?> DoChooseCardRewardAlternative(Dictionary<string, object?>? args)
    {
        var id = args!["option_id"]!.ToString()!;  // checked by ValidateCardRewardAlternative
        Log($"Choosing card reward alternative {id}");
        _cardSelector.ResolveAlternative(id);
        _syncCtx.Pump();
        WaitForActionExecutor();
        DrainEventOptionTaskIfDue();
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?>? ValidateClaimReward(Dictionary<string, object?>? args)
    {
        if (!TryGetInt(args, "reward_index", out var index))
            return Reject("missing_argument", "claim_reward requires 'reward_index'");
        if (_lastDecision!.GetValueOrDefault("rewards") is not List<Dictionary<string, object?>> rewards
            || _agentRewardsSet == null)
            return Reject("no_pending_rewards", "no rewards screen is open");
        if (index < 0 || index >= rewards.Count)
            return Reject("invalid_reward_index", $"reward index {index} is outside 0..{rewards.Count - 1}");
        if (rewards[index].GetValueOrDefault("available") is false)
            return Reject("reward_unavailable", $"reward {index} cannot be claimed: {rewards[index].GetValueOrDefault("reason")}");
        return null;
    }

    private Dictionary<string, object?>? ValidateCardRewardAlternative(Dictionary<string, object?>? args)
    {
        if (!_cardSelector.HasPendingReward)
            return Reject("no_pending_card_reward", "no card reward is pending");
        if (args == null || !args.TryGetValue("option_id", out var raw) || raw == null)
            return Reject("missing_argument", "choose_card_reward_alternative requires 'option_id'");
        var offered = _cardSelector.PendingRewardAlternatives ?? new();
        if (!offered.Any(a => a.OptionId == raw.ToString()))
            return Reject("invalid_alternative",
                $"'{raw}' is not offered here (offered: {string.Join(", ", offered.Select(a => a.OptionId))})");
        return null;
    }
}
