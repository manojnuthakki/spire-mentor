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
//
// Rewards offered by events and relics (RewardsCmd.OfferCustom, and sets like Neow's
// Bones' that call RewardsSet.Offer themselves) come to the same screen when the offer
// is made by an event option or a reward claim: RewardsSet.testSelector hands the set
// to the client instead of taking everything, and the offering code waits until the
// set is completed (every reward claimed) or skipped (`proceed`), as it waits on the
// game's screen. A set that disallows skipping (Neow's Bones) refuses `proceed`
// (`skip_not_allowed`) and says so with `can_proceed: false`. Sets nest: a relic
// claimed from a set can offer its own (Small Capsule), and the innermost is shown.
// So do offers made by a rest-site option or in a treasure room (RunSimulator.RestTreasure.cs);
// offers made by a shop purchase are still auto-taken.

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
    /// Claims suspended at something they opened (a card reward, a card removal, a set
    /// of rewards offered by the claimed relic), innermost last, each with the number of
    /// custom sets open when it started. Driven forward by the commands that answer.
    /// </summary>
    private readonly List<(Task Task, int Depth)> _pendingRewardClaims = new();

    /// <summary>
    /// Sets offered by events and relics that the client is claiming from, innermost
    /// last. The offering code awaits `Done` (inside RewardsSet.testSelector).
    /// </summary>
    private readonly List<(RewardsSet Set, TaskCompletionSource Done)> _customRewardSets = new();
    private readonly object _customRewardSetsLock = new();

    /// <summary>
    /// Set inside the thread-pool tasks that run an event option or a reward claim:
    /// rewards offered within them are shown to the client (agent mode).
    /// </summary>
    internal static readonly AsyncLocal<bool> SurfaceCustomRewards = new();

    /// <summary>Hands an offered set to the client; installed by start_run in agent mode.</summary>
    internal static Func<RewardsSet, Task>? AgentCustomRewardsOffer;

    private bool CustomRewardsPending
    {
        get { lock (_customRewardSetsLock) return _customRewardSets.Count > 0; }
    }

    private int CustomRewardsDepth
    {
        get { lock (_customRewardSetsLock) return _customRewardSets.Count; }
    }

    /// <summary>The innermost set offered by an event or relic, if any.</summary>
    private RewardsSet? CurrentCustomRewardsSet
    {
        get { lock (_customRewardSetsLock) return _customRewardSets.Count > 0 ? _customRewardSets[^1].Set : null; }
    }

    /// <summary>The set the client claims from: the innermost custom set, else the room's.</summary>
    private RewardsSet? ActiveAgentRewardsSet => CurrentCustomRewardsSet ?? _agentRewardsSet;

    /// <summary>Forget the previous room's rewards (a new room is being entered).</summary>
    private void ResetAgentRewards()
    {
        _agentRewardsSet = null;
        _agentRewardsShown = new();
        _pendingRewardClaims.Clear();
    }

    /// <summary>
    /// RewardsSet.testSelector in agent mode, on the offering thread: register the set
    /// and wait until the client has completed or skipped it.
    /// </summary>
    private Task OfferCustomRewardsToClient(RewardsSet set)
    {
        var done = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        lock (_customRewardSetsLock) _customRewardSets.Add((set, done));
        Log($"Rewards offered to the client: {string.Join(", ", set.Rewards.Select(r => r.GetType().Name))}"
            + (set.DisallowSkipping ? " (skipping disallowed)" : ""));
        return done.Task;
    }

    /// <summary>
    /// Wake the code that offered each innermost set the client has completed or
    /// skipped. Returns whether any was released.
    /// </summary>
    private bool ReleaseCompletedCustomRewardSets()
    {
        var sync = RunManager.Instance.RewardsSetSynchronizer;
        bool released = false;
        while (true)
        {
            TaskCompletionSource done;
            lock (_customRewardSetsLock)
            {
                if (_customRewardSets.Count == 0 || !sync.IsRewardsSetCompleted(_customRewardSets[^1].Set)) break;
                done = _customRewardSets[^1].Done;
                _customRewardSets.RemoveAt(_customRewardSets.Count - 1);
            }
            done.TrySetResult();
            _syncCtx.Pump();
            released = true;
        }
        return released;
    }

    /// <summary>
    /// A prompt the client must answer that was opened after `depth` custom sets were
    /// open: a card selection, a card reward, a bundle or a further set.
    /// </summary>
    private bool PromptPendingAbove(int depth) =>
        _cardSelector.HasPending || _cardSelector.HasPendingReward || _pendingBundles != null
        || CustomRewardsDepth > depth;

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
        return RewardsState(player, set: _agentRewardsSet, custom: false);
    }

    /// <summary>
    /// The rewards screen. `source` is `room` (after combat) or `custom` (offered by an
    /// event or relic); `can_proceed` is false when the set disallows skipping.
    /// </summary>
    private Dictionary<string, object?> RewardsState(Player player, RewardsSet set, bool custom)
    {
        _agentRewardsShown = set.Rewards.Where(r => !r.SuccessfullySelected).ToList();
        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "rewards",
            ["context"] = RunContext(),
            ["source"] = custom ? "custom" : "room",
            ["rewards"] = _agentRewardsShown.Select((r, i) => RewardDict(r, player, i)).ToList(),
            ["can_proceed"] = !set.DisallowSkipping,
            ["gold_earned"] = custom ? null : player.Gold - _goldBeforeCombat,
            ["player"] = PlayerSummary(player),
        };
    }

    /// <summary>The innermost set offered by an event or relic, as a rewards decision.</summary>
    private Dictionary<string, object?>? CustomRewardsState(Player player) =>
        CurrentCustomRewardsSet is { } set ? RewardsState(player, set, custom: true) : null;

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
    /// Claim one reward. A card reward, a card removal or a relic that offers its own
    /// rewards suspends the claim; it is stashed and finished by the command that
    /// answers what it opened. Claiming the last reward of an offered set completes the
    /// set, and the code that offered it continues.
    /// </summary>
    private Dictionary<string, object?> DoClaimReward(Player player, Dictionary<string, object?>? args)
    {
        TryGetInt(args, "reward_index", out var index);  // checked by ValidateClaimReward
        var reward = _agentRewardsShown[index];
        Log($"Claiming reward {index}: {reward.GetType().Name}");
        var sync = RunManager.Instance.RewardsSetSynchronizer;
        var depth = CustomRewardsDepth;
        var task = Task.Run(() =>
        {
            SurfaceCustomRewards.Value = true;
            return sync.SelectLocalReward(reward);
        });
        WaitUntilOptionTaskSettles(task, "reward claim or what it opens", () => PromptPendingAbove(depth));
        if (!task.IsCompleted && PromptPendingAbove(depth))
        {
            _pendingRewardClaims.Add((task, depth));
            WaitForActionExecutor();
            return DetectDecisionPoint();
        }
        FinishRewardClaim(task);
        DrainEventOptionTaskIfDue();
        return DetectDecisionPoint();
    }

    /// <summary>
    /// Drive stashed claims, innermost first, after what they opened was answered: each
    /// either completes or suspends again (a REROLL shows new cards; a set offered by
    /// the claimed relic still has rewards). A completed offered set wakes the code that
    /// offered it, which may be an outer claim.
    /// </summary>
    private void DriveRewardClaimTask()
    {
        while (true)
        {
            ReleaseCompletedCustomRewardSets();
            if (_pendingRewardClaims.Count == 0) return;
            var (task, depth) = _pendingRewardClaims[^1];
            WaitUntilOptionTaskSettles(task, "reward claim (until next prompt or completion)", () => PromptPendingAbove(depth));
            if (!task.IsCompleted && PromptPendingAbove(depth))
            {
                WaitForActionExecutor();
                return;
            }
            _pendingRewardClaims.RemoveAt(_pendingRewardClaims.Count - 1);
            FinishRewardClaim(task);
        }
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
        if (_pendingRewardClaims.All(c => c.Task.IsCompleted))
        {
            _pendingRewardClaims.Clear();
            return;
        }
        Log($"[WARN] Abandoning stashed reward claim ({why})");
        EngineErrors.Record("host", $"abandoned a stashed reward claim ({why}); its remaining effects will not run");
        _pendingRewardClaims.Clear();
    }

    /// <summary>
    /// Leave the rewards screen: skip what is left. An offered set is skipped and the
    /// code that offered it continues; the room's set continues as upstream does.
    /// </summary>
    private Dictionary<string, object?> ProceedFromAgentRewards()
    {
        var sync = RunManager.Instance.RewardsSetSynchronizer;
        if (CustomRewardsPending)
        {
            sync.SkipLocalRewardsSet();  // the innermost set: the one on screen
            _agentRewardsShown = new();
            DrainEventOptionTaskIfDue();
            return DetectDecisionPoint();
        }
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
            || ActiveAgentRewardsSet == null)
            return Reject("no_pending_rewards", "no rewards screen is open");
        if (index < 0 || index >= rewards.Count)
            return Reject("invalid_reward_index", $"reward index {index} is outside 0..{rewards.Count - 1}");
        if (rewards[index].GetValueOrDefault("available") is false)
            return Reject("reward_unavailable", $"reward {index} cannot be claimed: {rewards[index].GetValueOrDefault("reason")}");
        return null;
    }

    /// <summary>`proceed` would skip a set that disallows skipping (Neow's Bones).</summary>
    private Dictionary<string, object?>? ValidateProceedFromRewards()
    {
        if (_lastDecision!.GetValueOrDefault("can_proceed") is false)
            return Reject("skip_not_allowed", "these rewards cannot be skipped; claim them");
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
