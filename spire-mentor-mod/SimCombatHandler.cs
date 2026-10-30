// SimCombatHandler.cs -- agent-driven combat room handler and the combat_action board builder.
// Originally ported from zhiyue/sts2-rl-agent (STS2BridgeMod); rewritten clean-room.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using MegaCrit.Sts2.Core.AutoSlay;
using MegaCrit.Sts2.Core.AutoSlay.Handlers;
using MegaCrit.Sts2.Core.AutoSlay.Helpers;
using MegaCrit.Sts2.Core.Combat;
using MegaCrit.Sts2.Core.Commands;
using MegaCrit.Sts2.Core.Context;
using MegaCrit.Sts2.Core.Entities.Actions;
using MegaCrit.Sts2.Core.Entities.Cards;
using MegaCrit.Sts2.Core.Entities.Creatures;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.GameActions;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Models.Afflictions;
using MegaCrit.Sts2.Core.Models.Powers;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

/// <summary>
/// Room handler for Monster / Elite / Boss rooms. Every combat decision is
/// delegated to the bridge agent: the handler serializes the board, waits
/// (without limit) for a play / end_turn / potion reply, applies it, lets the
/// engine settle, and repeats until the fight is over.
/// </summary>
public class SimCombatHandler : IRoomHandler, IHandler
{
    // Turn-loop bounds and cadence (tuned values; see SPEC 12.4).
    private const int TurnCap = 200;
    private const int ActionCapPerTurn = 50;
    private const int WatchdogRefreshEveryActions = 10;
    private const int FrameSettleMs = 16;
    private static TimeSpan PlayPhaseWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(30));
    private static TimeSpan CombatEndWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(30));

    private static readonly RoomType[] CombatRoomTypes =
    {
        RoomType.Monster,
        RoomType.Elite,
        RoomType.Boss,
    };

    public RoomType[] HandledTypes => CombatRoomTypes;

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        Logger.Log("[SimCombat] Waiting for combat to start");
        await WaitHelper.Until(() => CombatManager.Instance.IsInProgress, ct,
            SimConfig.Instance.Scaled(AutoSlayConfig.nodeWaitTimeout), "Combat never started");
        Logger.Log("[SimCombat] Combat started");
        _lastAction = null;

        Player player = LocalContext.GetMe(RunManager.Instance.DebugOnlyGetState())
            ?? throw new InvalidOperationException("SimCombat: no local player in the run state");

        for (int turn = 1; turn <= TurnCap && CombatManager.Instance.IsInProgress; turn++)
        {
            ct.ThrowIfCancellationRequested();

            await WaitHelper.Until(
                () => IsPlayerInPlayPhase(player) || !CombatManager.Instance.IsInProgress,
                ct, PlayPhaseWait, $"Player turn {turn} never reached the play phase");
            if (IsOver(player))
                break;

            SimAutoSlayer.CurrentWatchdog?.Reset($"Combat turn {turn}");
            Logger.Log($"[SimCombat] Turn {turn}: awaiting agent decision");

            bool agentEndedTurn = await RunTurnDecisionsAsync(player, turn, ct);

            // Only reachable at the per-turn action cap: the agent never said
            // end_turn, but the turn has to finish somehow.
            if (!agentEndedTurn && MayAsk(player))
            {
                Logger.Log($"[SimCombat] Turn {turn}: {ActionCapPerTurn}-action cap reached, forcing end turn");
                PlayerCmd.EndTurn(player, canBackOut: false);
                await WaitForTurnEndAsync(player, ct);
            }
        }

        await WaitHelper.Until(() => !CombatManager.Instance.IsInProgress, ct,
            CombatEndWait, "Combat did not end");
        Logger.Log("[SimCombat] Combat finished");
    }

    /// <summary>
    /// Asks the agent for actions until it ends the turn, the action cap is
    /// hit, the player drops out of the play phase, or the fight is over (ended,
    /// ending, the player dead: a death mid-play leaves the phase at Play), checked
    /// again after each settle wait (<see cref="SimCombatTurn"/>). Returns whether the
    /// agent itself ended the turn. Rejected / errored replies still count as
    /// an action so a misbehaving agent cannot spin the turn forever.
    /// </summary>
    private Task<bool> RunTurnDecisionsAsync(Player player, int turn, CancellationToken ct) =>
        SimCombatTurn.RunAsync(
            () => MayAsk(player),
            () => AwaitSettledAsync(player, ct),
            async () =>
            {
                (string stateJson, long stateId) = SerializeCombatState(player);
                string reply = await SimAgentGate.RequestActionAsync(stateJson, ct, "combat_action");
                bool endedTurn = await ExecuteAgentAction(reply, player, stateId, ct);
                await Task.Delay(FrameSettleMs, ct);
                return endedTurn;
            },
            taken =>
            {
                if (taken > 0 && taken % WatchdogRefreshEveryActions == 0)
                    SimAutoSlayer.CurrentWatchdog?.Reset($"Combat turn {turn}, played {taken} cards");
            },
            ActionCapPerTurn, ct);

    /// <summary>Whether the agent may be asked for a play (<see cref="SimCombatTurn"/>): the play phase, the fight going on.</summary>
    private static bool MayAsk(Player player) =>
        SimCombatTurn.MayAsk(IsPlayerInPlayPhase(player), CombatManager.Instance.IsInProgress,
            CombatManager.Instance.IsEnding, player.Creature.IsDead);

    /// <summary>Whether the fight is over for the handler: ended, ending, or the player dead (<see cref="SimCombatTurn"/>).</summary>
    private static bool IsOver(Player player) =>
        SimCombatTurn.Over(CombatManager.Instance.IsInProgress, CombatManager.Instance.IsEnding, player.Creature.IsDead);

    private static bool IsPlayerInPlayPhase(Player player) =>
        player.PlayerCombatState?.Phase == PlayerTurnPhase.Play;

    /// <summary>
    /// Target for a card play. Only AnyEnemy cards take a target. A
    /// non-negative index addresses the full enemy list (so indices match the
    /// serialized <c>enemies</c> array) and must land on a hittable enemy; a
    /// negative index means "first hittable enemy".
    /// </summary>
    internal static Creature? ResolveTarget(CardModel card, int targetIndex)
    {
        if (card.TargetType != TargetType.AnyEnemy)
            return null;

        ICombatState? combat = card.CombatState;
        if (combat == null || combat.Enemies.Count == 0)
            return null;

        if (targetIndex < 0)
            return combat.HittableEnemies.FirstOrDefault();

        if (targetIndex >= combat.Enemies.Count)
            return null;

        Creature candidate = combat.Enemies[targetIndex];
        return candidate.IsHittable ? candidate : null;
    }

    /// <summary>
    /// Applies one agent reply (PROTOCOL.md section 4, combat verbs). Returns
    /// true only when the reply ended the turn. Every rejection sends a
    /// structured error and returns false, so the turn loop re-serializes and
    /// asks again.
    /// </summary>
    private async Task<bool> ExecuteAgentAction(string json, Player player, long stateId, CancellationToken ct)
    {
        RunState? runState = RunManager.Instance.DebugOnlyGetState();
        if (!SimProtocolHelpers.TryParse(json, runState, player, out JsonElement root) ||
            !SimProtocolHelpers.CheckStateId(root, stateId, runState, player))
        {
            return false;
        }

        try
        {
            string action = root.ValueKind == JsonValueKind.Object &&
                            root.TryGetProperty("action", out JsonElement actionEl) &&
                            actionEl.ValueKind == JsonValueKind.String
                ? actionEl.GetString() ?? ""
                : "";

            if (action.Equals("play", StringComparison.OrdinalIgnoreCase))
            {
                await ExecutePlayAsync(root, player, runState, ct);
                return false;
            }
            if (action.Equals("end_turn", StringComparison.OrdinalIgnoreCase))
            {
                Logger.Log("[SimCombat] Agent ended the turn");
                PlayerCmd.EndTurn(player, canBackOut: false);
                await WaitForTurnEndAsync(player, ct);
                return true;
            }
            if (action.Equals("potion", StringComparison.OrdinalIgnoreCase))
            {
                await ExecutePotionAsync(root, player, runState, ct);
                return false;
            }

            SimProtocolHelpers.SendError("unknown_action", $"Unknown combat action \"{action}\"", runState, player);
            return false;
        }
        // CLEANROOM2-Q6: cancellation (run teardown) propagates instead of
        // being reported to the agent as internal_error.
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            Logger.Log($"[SimCombat] Error applying agent action: {ex}");
            SimProtocolHelpers.SendError("internal_error", ex.Message, runState, player);
            return false;
        }
    }

    /// <summary>The <c>play</c> verb: card_index into the hand, optional target_index.</summary>
    private static async Task ExecutePlayAsync(JsonElement root, Player player, RunState? runState, CancellationToken ct)
    {
        if (!TryReadInt(root, "card_index", out int cardIndex))
        {
            SimProtocolHelpers.SendError("malformed_action", "\"play\" requires a numeric \"card_index\"", runState, player);
            return;
        }
        int targetIndex = TryReadInt(root, "target_index", out int t) ? t : -1;

        IReadOnlyList<CardModel> hand = PileType.Hand.GetPile(player).Cards;
        if (cardIndex < 0 || cardIndex >= hand.Count)
        {
            SimProtocolHelpers.SendError("invalid_index",
                $"card_index {cardIndex} out of range (hand size {hand.Count})", runState, player);
            return;
        }

        CardModel card = hand[cardIndex];
        if (!card.CanPlay(out UnplayableReason reason, out AbstractModel? _))
        {
            SimProtocolHelpers.SendError("unplayable_card", $"{card.Id.Entry}: {reason}", runState, player);
            return;
        }

        Creature? target = ResolveTarget(card, targetIndex);
        if (card.TargetType == TargetType.AnyEnemy && target == null)
        {
            SendMissingTarget(targetIndex, card.Id.Entry, runState, player);
            return;
        }

        Logger.Log($"[SimCombat] Playing {card.Id.Entry} (target_index={targetIndex})");
        await PlayCardAndWaitAsync(player, card, target, ct);
    }

    /// <summary>The <c>potion</c> verb: use or discard the potion in a slot.</summary>
    private static async Task ExecutePotionAsync(JsonElement root, Player player, RunState? runState, CancellationToken ct)
    {
        if (!TryReadInt(root, "slot", out int slot))
        {
            SimProtocolHelpers.SendError("malformed_action", "\"potion\" requires a numeric \"slot\"", runState, player);
            return;
        }
        int targetIndex = TryReadInt(root, "target_index", out int t) ? t : -1;
        bool discard = root.TryGetProperty("discard", out JsonElement discardEl) &&
                       discardEl.ValueKind == JsonValueKind.True;

        PotionModel? potion = SafeGetPotionAtSlot(player, slot);
        if (potion == null)
        {
            SimProtocolHelpers.SendError("empty_potion_slot", $"potion slot {slot} is empty", runState, player);
            return;
        }

        if (discard)
        {
            // PotionCmd.Discard has no in-use guard; the game UI disables
            // Discard for a queued potion, so the bridge refuses it too.
            if (potion.IsQueued)
            {
                SimProtocolHelpers.SendError("potion_busy",
                    $"potion {potion.Id.Entry} in slot {slot} is already queued for use", runState, player);
                return;
            }
            Logger.Log($"[SimCombat] Discarding potion in slot {slot}");
            await DiscardPotionAndWaitAsync(player, slot, ct);
            return;
        }

        Creature? target = ResolvePotionTarget(player, potion, targetIndex);
        if (potion.TargetType == TargetType.AnyEnemy && target == null)
        {
            SendMissingTarget(targetIndex, potion.Id.Entry, runState, player);
            return;
        }

        Logger.Log($"[SimCombat] Using potion in slot {slot} (target_index={targetIndex})");
        await UsePotionAndWaitAsync(player, potion, slot, target, ct);
    }

    /// <summary>An explicit index that resolved to nothing is invalid_target; no index at all is target_required.</summary>
    private static void SendMissingTarget(int targetIndex, string what, RunState? runState, Player? player)
    {
        if (targetIndex >= 0)
        {
            SimProtocolHelpers.SendError("invalid_target",
                $"target_index {targetIndex} is not a hittable enemy for {what}", runState, player);
        }
        else
        {
            SimProtocolHelpers.SendError("target_required",
                $"{what} needs an enemy target and none is hittable", runState, player);
        }
    }

    /// <summary>Reads an int32 property that must be a JSON number.</summary>
    private static bool TryReadInt(JsonElement root, string name, out int value)
    {
        value = 0;
        return root.TryGetProperty(name, out JsonElement el) &&
               el.ValueKind == JsonValueKind.Number &&
               el.TryGetInt32(out value);
    }

    /// <summary>The potion in <paramref name="slot"/>, or null (empty, negative or unreadable slot).</summary>
    internal static PotionModel? SafeGetPotionAtSlot(Player player, int slot)
    {
        if (slot < 0)
            return null;
        try
        {
            return player.GetPotionAtSlotIndex(slot);
        }
        catch (Exception)
        {
            return null;
        }
    }

    /// <summary>
    /// Target for a potion use. AnyEnemy follows the same addressing as
    /// <see cref="ResolveTarget"/>; self/ally potions target the player;
    /// everything else auto-targets (null).
    /// </summary>
    internal static Creature? ResolvePotionTarget(Player player, PotionModel? potion, int targetIndex)
    {
        if (potion == null)
            return null;

        switch (potion.TargetType)
        {
            case TargetType.AnyEnemy:
            {
                ICombatState? combat = player.Creature?.CombatState;
                if (combat == null)
                    return null;
                if (targetIndex < 0)
                    return combat.HittableEnemies.FirstOrDefault();
                if (targetIndex >= combat.Enemies.Count)
                    return null;
                Creature candidate = combat.Enemies[targetIndex];
                return candidate.IsHittable ? candidate : null;
            }
            case TargetType.Self:
            case TargetType.AnyPlayer:
            case TargetType.AnyAlly:
                return player.Creature;
            default:
                return null;
        }
    }

    /// <summary>
    /// Settle-wait after requesting EndTurn -- the same Instant fast-mode
    /// async fix PlayCardAndWaitAsync got. PlayerCmd.EndTurn only REQUESTS
    /// the end of turn; under a busy action queue (multi-enemy fights, boss
    /// trigger storms) the player stays in the Play phase for many frames.
    /// Without this wait the turn loop's Until(IsPlayerInPlayPhase) gate is
    /// already true, the handler re-serializes the same Play-phase state, the
    /// agent (deterministically) answers end_turn again, and the duplicate
    /// EndTurn command lands in the NEXT turn's play phase -- ending it with
    /// zero cards played. Measured on the 2026-07-13 sweep: 209 duplicate
    /// re-sends across 9/12 runs; one death took 41 unblocked damage from a
    /// single eaten turn (seed 1005, floor 4).
    /// </summary>
    private static async Task WaitForTurnEndAsync(Player player, CancellationToken ct)
    {
        int waitMs = 0;
        while (waitMs < 3000)
        {
            if (!IsPlayerInPlayPhase(player) || !CombatManager.Instance.IsInProgress)
                return;
            await Task.Delay(16, ct);
            waitMs += 16;
        }
        Logger.Log("[SimCombat] WARNING: end turn still unprocessed after 3s; the agent will see a re-sent state");
    }

    private const int PlayStartWaitMs = 1000;
    private const int ResolutionSettleMs = 2000;

    /// <summary>
    /// Enqueues a card play and waits until it has really resolved.
    ///
    /// The start signal (energy paid / card left the hand) fires long before
    /// the card's effects, its move to discard/exhaust, cost-modifier cleanup
    /// and after-exhaust hooks (e.g. Feel No Pain's block) land; serializing at
    /// that point was the largest source of live mirror divergences. The
    /// action's State reaching Finished is strictly after all of that.
    /// Canceled is terminal but never completes the action's completion task,
    /// hence polling the state rather than awaiting it. GatheringPlayerChoice
    /// (or an open card selection) means the play is parked on a choice the
    /// agent answers on another path.
    /// </summary>
    private static async Task PlayCardAndWaitAsync(Player player, CardModel card, Creature? target, CancellationToken ct)
    {
        // Snapshot BEFORE enqueueing: under Instant fast mode the play can
        // resolve within the same frame.
        int energyBefore = player.PlayerCombatState?.Energy ?? -1;
        int handBefore = PileType.Hand.GetPile(player).Cards.Count;

        var action = new PlayCardAction(card, target);
        _lastAction = new SimActionWatch(action);
        RunManager.Instance.ActionQueueSynchronizer.RequestEnqueue(action);

        // A free card that draws its own replacement changes neither signal,
        // hence the cap.
        await PollUntilAsync(() =>
            (player.PlayerCombatState?.Energy ?? -1) != energyBefore ||
            PileType.Hand.GetPile(player).Cards.Count != handBefore ||
            !IsPlayerInPlayPhase(player) ||
            !CombatManager.Instance.IsInProgress, PlayStartWaitMs, ct);

        if (!await WaitForResolutionAsync(action, player, ct))
        {
            Logger.Log($"[SimCombat] WARNING: {card.Id.Entry} still resolving after {ResolutionSettleMs}ms; " +
                       "the next state may be mid-resolution");
        }
    }

    /// <summary>The last card play or potion use the agent asked for in this fight (<see cref="SimCombatSettle"/>).</summary>
    private static SimActionWatch? _lastAction;

    /// <summary>Whether the board is settled now (combat_action's <c>settled</c>; <see cref="SimCombatSettle"/>).</summary>
    private static bool IsSettled()
    {
        try
        {
            return SimCombatSettle.Settled(_lastAction, SimCardSelector.SelectionOpen,
                RunManager.Instance.ActionExecutor.IsRunning);
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimCombat] settled probe: {ex.Message}");
            return false;
        }
    }

    /// <summary>
    /// Waits, before the board is serialized, until it is settled (or the turn or the fight is over).
    /// A play or potion that opened a selection returns from its own settle wait as the selection
    /// opens; the agent answers it on the card selector's path, and only then does the action resume
    /// and end. The time a selection stays open is the agent's and doesn't count; otherwise the wait
    /// gives up after <see cref="ResolutionSettleMs"/> and the board goes out with <c>settled</c>
    /// false (the agent reads it again).
    /// </summary>
    private static async Task AwaitSettledAsync(Player player, CancellationToken ct)
    {
        int waited = 0;
        while (!IsSettled() && IsPlayerInPlayPhase(player) && CombatManager.Instance.IsInProgress)
        {
            if (SimCardSelector.SelectionOpen)
                waited = 0;
            else if (waited >= ResolutionSettleMs)
            {
                Logger.Log($"[SimCombat] WARNING: the board is still not settled after {ResolutionSettleMs}ms " +
                           $"(last action {_lastAction?.Action.State}); sent with settled false");
                return;
            }
            else
                waited += FrameSettleMs;
            await Task.Delay(FrameSettleMs, ct);
        }
    }

    /// <summary>Second settle phase shared by card plays and potion uses.</summary>
    private static Task<bool> WaitForResolutionAsync(GameAction action, Player player, CancellationToken ct) =>
        PollUntilAsync(() =>
            IsActionQuiescent(action) ||
            SimCardSelector.SelectionOpen ||
            !IsPlayerInPlayPhase(player) ||
            !CombatManager.Instance.IsInProgress, ResolutionSettleMs, ct);

    /// <summary>Polls <paramref name="condition"/> once per frame; false if it never held within <paramref name="maxMs"/>.</summary>
    private static async Task<bool> PollUntilAsync(Func<bool> condition, int maxMs, CancellationToken ct)
    {
        for (int waited = 0; ; waited += FrameSettleMs)
        {
            if (condition())
                return true;
            if (waited >= maxMs)
                return false;
            await Task.Delay(FrameSettleMs, ct);
        }
    }

    /// <summary>
    /// True once <paramref name="action"/> can no longer mutate game state on
    /// its own: it ran to completion, was cancelled, or suspended waiting on a
    /// player choice.
    ///
    /// GameAction.State is public (GameAction.cs:44) and is the engine's own
    /// end-of-action signal:
    ///   - Finished (GameAction.cs:139-146) is set in the `finally` of
    ///     GameAction.Execute only after _executionTask completed, i.e. after
    ///     the derived ExecuteAction's final `await` returned.
    ///   - Canceled (GameAction.cs:191) is terminal but never completes
    ///     _completionSource, so CompletionTask would never resolve there —
    ///     hence polling State instead of awaiting CompletionTask.
    ///   - GatheringPlayerChoice (GameAction.cs:183) means the action parked
    ///     itself on a player decision; the agent has to answer on another
    ///     channel before it can move again, so waiting here would deadlock
    ///     until the settle timeout.
    /// Every caller polls this under its own bounded timeout, so this can
    /// never turn into an unbounded wait.
    /// </summary>
    private static bool IsActionQuiescent(GameAction action)
    {
        return action.State == GameActionState.Finished
            || action.State == GameActionState.Canceled
            || action.State == GameActionState.GatheringPlayerChoice;
    }

    /// <summary>
    /// Enqueues a potion use and waits for it to resolve, with the same
    /// second settle phase as card plays (without it a draw potion was
    /// serialized with the drawn card still in the draw pile).
    /// </summary>
    private static async Task UsePotionAndWaitAsync(
        Player player, PotionModel potion, int slot, Creature? target, CancellationToken ct)
    {
        var action = new UsePotionAction(potion, target, CombatManager.Instance.IsInProgress);
        _lastAction = new SimActionWatch(action);
        RunManager.Instance.ActionQueueSynchronizer.RequestEnqueue(action);

        // The slot empties at the very start of the use.
        await PollUntilAsync(() =>
            SafeGetPotionAtSlot(player, slot) == null ||
            !IsPlayerInPlayPhase(player) ||
            !CombatManager.Instance.IsInProgress, PlayStartWaitMs, ct);

        if (!await WaitForResolutionAsync(action, player, ct))
        {
            Logger.Log($"[SimCombat] WARNING: potion {potion.Id.Entry} still resolving after {ResolutionSettleMs}ms; " +
                       "the next state may be mid-resolution");
        }
    }

    /// <summary>
    /// Discards <paramref name="potion"/> via PotionCmd.Discard (v0.107.1
    /// decomp section 3 -- distinct from "using" it: this mutates the slot
    /// synchronously, so a settle-poll on the slot emptying is enough; no
    /// RequestEnqueue/ActionQueueSynchronizer round-trip is involved).
    /// </summary>
    private static async Task DiscardPotionAndWaitAsync(
        Player player, int slot, CancellationToken ct)
    {
        PotionModel? potion = SafeGetPotionAtSlot(player, slot);
        if (potion == null)
            return;

        await PotionCmd.Discard(potion);

        int waitMs = 0;
        while (waitMs < 1000)
        {
            if (SafeGetPotionAtSlot(player, slot) == null)
                break;
            await Task.Delay(16, ct);
            waitMs += 16;
        }
    }

    // ----------------------------------------------------------------
    // State serialization
    //
    // Phase 1a: delegates to SimStateExtractor for everything shared
    // (envelope, cards, piles, creatures/intents, potions). This handler
    // now only assembles the combat-specific shape around that.
    // ----------------------------------------------------------------

    /// <summary>
    /// Player card plays started this turn, bucketed by card type in the
    /// Rust engine's canonical order (Attack=0, Skill=1, Power=2, Curse=3,
    /// Status=4 — sts2-core cards.rs CardType::index). Mirrors the scan
    /// RingingPower.ShouldPlay does over CombatManager history.
    /// </summary>
    private static int[] CountCardsPlayedThisTurn(CombatState? combatState, Creature? playerCreature)
    {
        var buckets = new int[5];
        if (combatState == null || playerCreature == null)
            return buckets;
        foreach (var entry in CombatManager.Instance.History.CardPlaysStarted)
        {
            if (!entry.HappenedThisTurn(combatState))
                continue;
            if (entry.CardPlay.Card.Owner.Creature != playerCreature)
                continue;
            int idx = entry.CardPlay.Card.Type switch
            {
                CardType.Attack => 0,
                CardType.Skill => 1,
                CardType.Power => 2,
                CardType.Curse => 3,
                CardType.Status => 4,
                _ => -1,
            };
            if (idx >= 0)
                buckets[idx]++;
        }
        return buckets;
    }

    /// <summary>
    /// ChainsOfBinding's once-per-turn gate, read from the POWER'S OWN FLAG.
    ///
    /// This field decides whether a second Bound card may be played
    /// (`ChainsOfBindingPower.ShouldPlay` returns `!boundCardPlayed`), and it
    /// is the single largest source of disagreement between the search and the
    /// live game: 22 of the 41 plays the game refused across the first 51
    /// ascension-climb runs were `BlockedByHook / CHAINS_OF_BINDING_POWER`,
    /// every one of them in a Queen fight. Each cost three refusals and then a
    /// thrown-away turn.
    ///
    /// It used to be RE-DERIVED here by walking `History.CardPlaysStarted` and
    /// re-testing each played card's affliction, on the reasoning that
    /// afflictions are only cleared at turn end so the scan reproduces the
    /// power exactly. Reproducing another object's private bookkeeping is a
    /// standing bet that every one of its edge cases was noticed --
    /// `BeforeCardPlayed`'s dupe/owner filters, `BeforeSideTurnEnd`'s reset
    /// point, and whatever the history collection does or does not record --
    /// and it is a bet with no upside, because the flag itself is reachable.
    /// Read the authority; do not model it. (Same lesson as every other
    /// SpireMentor-synthesized wire field.)
    ///
    /// The history scan survives as the fallback for a build whose internals
    /// moved, so a renamed field degrades to the old behavior instead of
    /// silently reporting `false` -- which is the answer that causes the
    /// refusals.
    /// </summary>
    private static bool BoundCardPlayedThisTurn(CombatState? combatState, Creature? playerCreature)
    {
        if (combatState == null || playerCreature == null)
            return false;
        bool? own = ReadChainsBoundFlag(playerCreature);
        if (own.HasValue)
            return own.Value;
        foreach (var entry in CombatManager.Instance.History.CardPlaysStarted)
        {
            if (!entry.HappenedThisTurn(combatState))
                continue;
            if (entry.CardPlay.Card.Owner.Creature != playerCreature)
                continue;
            if (entry.CardPlay.Card.IsDupe)
                continue;
            if (entry.CardPlay.Card.Affliction is Bound)
                return true;
        }
        return false;
    }

    /// <summary>Logged once: a reflection failure repeats every poll.</summary>
    private static bool _chainsFlagUnreadable;

    /// <summary>
    /// `ChainsOfBindingPower`'s private `boundCardPlayed`, or null when it
    /// cannot be read (caller falls back to the history scan).
    ///
    /// `PowerModel._internalData` holds an instance of the power's private
    /// nested `Data` class (`PowerModel.cs:45`, v0.107.1); the power itself
    /// reaches it through `GetInternalData&lt;T&gt;()`, which is `protected`.
    /// Both hops are by name so a build that renames either one falls back
    /// rather than throwing.
    ///
    /// No power on the creature means no card is Bound and nothing is gated,
    /// which is `false` -- not "unreadable".
    /// </summary>
    private static bool? ReadChainsBoundFlag(Creature playerCreature)
    {
        try
        {
            PowerModel? chains = playerCreature.Powers
                .FirstOrDefault(p => p is ChainsOfBindingPower);
            if (chains == null)
                return false;
            object? data = typeof(PowerModel)
                .GetField("_internalData", BindingFlags.NonPublic | BindingFlags.Instance)
                ?.GetValue(chains);
            object? flag = data?.GetType()
                .GetField("boundCardPlayed",
                          BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
                ?.GetValue(data);
            if (flag is bool value)
                return value;
        }
        catch (Exception ex)
        {
            if (!_chainsFlagUnreadable)
            {
                _chainsFlagUnreadable = true;
                Logger.Log(
                    "[SimCombat] Could not read ChainsOfBindingPower.boundCardPlayed " +
                    $"({ex.Message}); falling back to the history scan.");
            }
            return null;
        }
        if (!_chainsFlagUnreadable)
        {
            _chainsFlagUnreadable = true;
            Logger.Log(
                "[SimCombat] ChainsOfBindingPower.boundCardPlayed not found on this build; " +
                "falling back to the history scan.");
        }
        return null;
    }

    private const string SerializationFailedJson = "{\"type\":\"combat_action\",\"error\":\"serialization_failed\"}";

    /// <summary>The combat_action message for the agent, and its state_id (-1 on failure).</summary>
    private (string json, long stateId) SerializeCombatState(Player player)
    {
        try
        {
            // Read before and after the board: settled true means nothing was moving while it was built.
            bool settledBefore = IsSettled();
            Dictionary<string, object>? state = BuildCombatState(player);
            if (state == null)
                return (SerializationFailedJson, -1);
            state["settled"] = settledBefore && IsSettled();
            return (JsonSerializer.Serialize(state), (long)state["state_id"]);
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimCombat] Could not serialize combat state: {ex}");
            return (SerializationFailedJson, -1);
        }
    }

    /// <summary>
    /// The one builder of the combat board (<c>combat_action</c>), envelope
    /// applied. The advisor and the board embedded in card_select reuse it,
    /// so there is a single shape. Null (logged) on failure.
    /// </summary>
    internal static Dictionary<string, object>? BuildCombatState(Player player)
    {
        try
        {
            CombatState? combatState = CombatManager.Instance.DebugOnlyGetState();
            Creature? creature = player.Creature;
            PlayerCombatState? pcs = player.PlayerCombatState;
            RunState? runState = RunManager.Instance.DebugOnlyGetState();

            var playerInfo = new Dictionary<string, object>
            {
                ["hp"] = creature?.CurrentHp ?? 0,
                ["max_hp"] = creature?.MaxHp ?? 0,
                ["block"] = creature?.Block ?? 0,
                ["energy"] = pcs?.Energy ?? 0,
                ["max_energy"] = pcs?.MaxEnergy ?? 3,
                ["powers"] = creature?.Powers.Select(SimStateExtractor.SerializePower).ToList()
                             ?? new List<Dictionary<string, object?>>(),
                ["cards_played_this_turn"] = CountCardsPlayedThisTurn(combatState, creature),
                ["bound_card_played_this_turn"] = BoundCardPlayedThisTurn(combatState, creature),
            };
            try
            {
                if (pcs != null && pcs.Stars >= 0 && player.Character?.Id.Entry == "REGENT")
                    playerInfo["stars"] = pcs.Stars;
            }
            catch (Exception ex)
            {
                Logger.Log($"[SimCombat] Could not read stars: {ex.Message}");
            }

            List<Dictionary<string, object?>> hand = combatState != null && pcs != null
                ? PileType.Hand.GetPile(player).Cards
                    .Select(c => SimStateExtractor.SerializeCard(c, includeLegality: true))
                    .ToList()
                : new List<Dictionary<string, object?>>();

            List<Dictionary<string, object?>> enemies = combatState?.Enemies
                .Select(e => SimStateExtractor.SerializeCreature(e))
                .ToList() ?? new List<Dictionary<string, object?>>();

            List<Dictionary<string, object?>> orbs = SimStateExtractor.SerializeOrbs(player, out int orbCapacity);

            List<Dictionary<string, object?>> potions = player.PotionSlots
                .Select((potion, slot) => SimStateExtractor.SerializePotionSlot(potion, slot))
                .ToList();

            var state = new Dictionary<string, object>
            {
                ["type"] = "combat_action",
                ["player"] = playerInfo,
                ["hand"] = hand,
                ["draw_pile"] = SimStateExtractor.SerializePile(pcs?.DrawPile),
                ["discard_pile"] = SimStateExtractor.SerializePile(pcs?.DiscardPile),
                ["exhaust_pile"] = SimStateExtractor.SerializePile(pcs?.ExhaustPile),
                // A card whose OnPlay awaits a selection sits in the play pile
                // until the pick resolves; a board serialized during that
                // screen must still show it.
                ["play_pile"] = SimStateExtractor.SerializePile(pcs?.PlayPile),
                ["enemies"] = enemies,
                ["orbs"] = orbs,
                ["orb_capacity"] = orbCapacity,
                ["pets"] = SimStateExtractor.SerializePets(player),
                ["potions"] = potions,
                ["available_actions"] = GetAvailableActions(potions),
                ["draw_pile_count"] = pcs?.DrawPile?.Cards.Count ?? 0,
                ["discard_pile_count"] = pcs?.DiscardPile?.Cards.Count ?? 0,
                ["exhaust_pile_count"] = pcs?.ExhaustPile?.Cards.Count ?? 0,
                ["round"] = combatState?.RoundNumber ?? 0,
                ["room_type"] = runState?.CurrentRoom?.RoomType.ToString() ?? "Combat",
            };

            SimStateExtractor.ApplyEnvelope(state, runState, player);
            return state;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimCombat] Could not build combat state: {ex}");
            return null;
        }
    }

    /// <summary>
    /// <see cref="BuildCombatState"/> for EMBEDDING inside another decision
    /// message, or null when no fight is in progress.
    ///
    /// Drops <c>state_id</c>: the agent gate keys a response to the top-level
    /// decision it answers, and a second id nested inside it is a trap for
    /// anyone who reaches for the wrong one. A failure is logged and reported
    /// as "no board" rather than thrown — the consumer decides how serious
    /// that is, and the combat mirror treats a missing board on an in-combat
    /// selection as a divergence rather than guessing.
    /// </summary>
    internal static Dictionary<string, object>? BuildEmbeddedCombatState(Player? player)
    {
        try
        {
            if (player == null || !CombatManager.Instance.IsInProgress)
                return null;
            Dictionary<string, object>? state = BuildCombatState(player);
            state?.Remove("state_id");
            return state;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimCombat] Could not embed the combat board: {ex.Message}");
            return null;
        }
    }

    /// <summary>PLAY and END_TURN always; POTION when any slot reports can_use=true.</summary>
    private static List<string> GetAvailableActions(IEnumerable<Dictionary<string, object?>> potions)
    {
        var actions = new List<string> { "PLAY", "END_TURN" };
        if (potions.Any(p => p.TryGetValue("can_use", out object? canUse) && canUse is true))
            actions.Add("POTION");
        return actions;
    }
}
