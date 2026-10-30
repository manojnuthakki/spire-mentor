// SimStateExtractor.cs -- shared, full-fidelity state extraction helpers.
//
// New file, not present in the donor. Phase 1a of the bridge plan: every
// outbound decision message previously hand-rolled its own subset of card/
// creature/relic/potion/run fields (see phase1-current.md's audit, section
// 3, "Serialization helpers -- inventory of duplication/inconsistency").
// This file centralizes that into one set of static, JSON-serializable
// extractors so every handler emits the same rich schema instead of N
// slightly-different ones.
//
// Design constraints (see docs/plans/2026-07-10-real-game-bridge.md phase 1a):
//   - "C# stays dumb": raw extraction only, no auto-resolution/decision logic.
//   - Direct typed member access preferred over reflection everywhere --
//     every field this file reads turned out to be public at v0.107.1
//     (verified against the decomp), with ONE exception: the cost model's
//     CardEnergyCost._localModifiers list has no public accessor, and
//     SafePersistentCost needs the per-modifier LIFETIME that no public
//     method exposes. That single field read is guarded and falls back to
//     "unknown" (null) rather than to a wrong number.
//   - Null-safe: any game subsystem may be absent (no orbs for Ironclad, no
//     pets, empty potion slots, no active combat for deck/master_deck
//     cards). Prefer ?. chains + defaults; a top-level try/catch per
//     "category" logs and emits {"error": "..."} for that category only,
//     never lets one bad field take down the whole message.
//   - Cheap per-decision: combat_action is sent ~4x/s. Nothing here is
//     quadratic over the map graph -- SerializeMapGraph is never called from
//     combat_action, only from run-level (per-screen) messages (map_select's
//     handler plus ApplyEnvelope's MapCarryingTypes).
//
// Card cost fields (see phase1-target.md section 4, "clever things worth
// stealing" #2): `effective_cost` calls the engine's own
// CardEnergyCost.GetAmountToSpend() rather than re-deriving discount/X-cost
// math ourselves.

using System;
using System.Collections.Generic;
using System.Linq;
using MegaCrit.Sts2.Core.Combat;
using MegaCrit.Sts2.Core.Combat.History;
using MegaCrit.Sts2.Core.Combat.History.Entries;
using MegaCrit.Sts2.Core.Entities.Cards;
using MegaCrit.Sts2.Core.Entities.Creatures;
using MegaCrit.Sts2.Core.Entities.Orbs;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Entities.Potions;
using MegaCrit.Sts2.Core.Map;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.MonsterMoves.Intents;
using MegaCrit.Sts2.Core.Runs;
using MegaCrit.Sts2.Core.Unlocks;

namespace SpireMentor;

public static class SimStateExtractor
{
    // ----------------------------------------------------------------
    // Envelope: seed/act/floor/mod_version/run_state on every outbound
    // decision message. Fixes phase1-current.md gap #6 (inconsistent
    // presence across message types) and gap #2/#3/#4 (no relics/gold/deck
    // anywhere).
    // ----------------------------------------------------------------

    /// <summary>
    /// Run-level (non-combat) decision message types that carry the act map
    /// graph in their envelope. 2026-08-02 run-level dialect audit: the map
    /// was only ever attached to map_select, so every OFF-map run-policy
    /// decision on the live arm (Neow, the act-start Ancient relic offer =
    /// the boss-relic surface, any event/shop/rest before an act's first map
    /// screen, and the card_selects those open) was decided MAP-BLIND --
    /// obs.map/map_coord/act_floor all degenerate -- while headless_eval
    /// seeds the identical client cache via get_map() for any off-map
    /// decision (headless_eval.py:1510-1519). combat_action is deliberately
    /// absent (sent ~4x/s; SerializeMapGraph walks the whole act graph),
    /// card_select is included (a handful per floor, and the Neow-opened
    /// transform/remove picks are exactly the map-blind ones).
    /// </summary>
    private static readonly HashSet<string> MapCarryingTypes = new()
    {
        NonCombatBridgeProtocol.EventState,
        NonCombatBridgeProtocol.RestSiteState,
        NonCombatBridgeProtocol.ShopState,
        NonCombatBridgeProtocol.CardRewardState,
        NonCombatBridgeProtocol.RewardScreenState,
        NonCombatBridgeProtocol.TreasureState,
        NonCombatBridgeProtocol.BossRelicState,
        NonCombatBridgeProtocol.CardSelectState,
        NonCombatBridgeProtocol.CardBundleState,
        NonCombatBridgeProtocol.CrystalSphereState,
    };

    /// <summary>
    /// Mutates <paramref name="msg"/> in place, adding the common envelope
    /// fields. Safe to call with a null runState/player (e.g. between runs);
    /// everything degrades to defaults/empty collections.
    /// </summary>
    public static void ApplyEnvelope(Dictionary<string, object> msg, RunState? runState, Player? player)
    {
        // Act map graph on every RUN-LEVEL decision message (see
        // MapCarryingTypes). Handlers set msg["type"] before calling this
        // (verified for every call site); map_select keeps its own
        // handler-supplied copy via the ContainsKey guard.
        if (runState != null
            && msg.TryGetValue(NonCombatBridgeProtocol.TypeField, out object? typeVal)
            && typeVal is string typeStr
            && MapCarryingTypes.Contains(typeStr)
            && !msg.ContainsKey("map"))
        {
            try { msg["map"] = SerializeMapGraph(runState); }
            catch { /* map must never take down the envelope */ }
        }
        msg["seed"] = runState?.Rng.StringSeed ?? "";
        msg["act"] = (runState?.CurrentActIndex ?? 0) + 1;
        // WHICH act, not just its index. Act 1 ships two models (Overgrowth /
        // Underdocks, GH #544), so the index alone cannot name the room, and
        // every painted region downstream is keyed by this name — the combat
        // backdrop, the card-reward scene, the rest-site clearing, the map
        // parchment. Without it the workbench painted Overgrowth for all three
        // acts of every live run it replayed.
        //
        // `Id.Entry` is the act model's class name ("Overgrowth", "Underdocks",
        // "Hive", "Glory"), which is also its asset-directory name and matches
        // what the headless oracle reports through `act_name`. ActModel's own
        // `FilePathIdentifier` says the same thing but is protected.
        msg["act_name"] = SafeString(() => runState?.Act?.Id.Entry) ?? "";
        msg["floor"] = runState?.TotalFloor ?? 0;
        msg["mod_version"] = MainFile.ModVersion;
        msg["run_state"] = SerializeRunState(runState, player);
        // The run's RNG position: the once-hashed master seed every per-stream
        // salt mixes into (`RunRngSet.Seed`, which is what sts2-rs's
        // `RunRngSet::new` takes) plus each stream's read count. `seed` above
        // is the human-facing STRING seed and is not a substitute.
        //
        // Without these a state reconstruction has no RNG lineage at all — its
        // monster_ai and shuffle streams come from a default seed — so every
        // enemy intent and reshuffle order it predicts is a guess, and a guess
        // that misses is not evidence of anything. Same block, same names, as
        // the headless runner emits; the mirror's restore path is shared.
        msg["rng"] = RngState(runState);
        // Phase 1b stale-state guard (see SimProtocolHelpers.cs / PROTOCOL.md
        // "state_id"): every outbound message gets a fresh, monotonically
        // increasing id here so a handler can compare it against an optional
        // "state_id" the client echoes back on its reply.
        msg["state_id"] = SimProtocolHelpers.NextStateId();
    }

    /// <summary>
    /// `{"run_seed": uint, "counters": {stream: readCount}}` — the run's RNG
    /// position, keyed by the same snake_case stream names sts2-rs uses so the
    /// two sides line up field-for-field. Empty when no run is active.
    /// </summary>
    private static Dictionary<string, object?> RngState(RunState? runState)
    {
        var d = new Dictionary<string, object?>();
        var rng = runState?.Rng;
        if (rng == null) return d;
        try
        {
            // Built LOCALLY and published as a pair. A partial block here is
            // worse than no block: the consumer
            // (`combat_mirror.reconcile`) reads the presence of `run_seed`
            // alone as "the world owns the dice", so it stops carrying its own
            // valid cursors — and a `state_from_dict` with a seed and no
            // counters rebuilds every stream at count 0. Seed-without-counters
            // therefore does not degrade to the pre-field behaviour, it
            // actively destroys RNG lineage that was previously intact.
            var seed = rng.Seed;
            var c = new Dictionary<string, object?>
            {
                ["up_front"] = rng.UpFront.Counter,
                ["shuffle"] = rng.Shuffle.Counter,
                ["combat_card_generation"] = rng.CombatCardGeneration.Counter,
                ["combat_potion_generation"] = rng.CombatPotionGeneration.Counter,
                ["combat_card_selection"] = rng.CombatCardSelection.Counter,
                ["combat_energy_costs"] = rng.CombatEnergyCosts.Counter,
                ["combat_targets"] = rng.CombatTargets.Counter,
                ["monster_ai"] = rng.MonsterAi.Counter,
                ["niche"] = rng.Niche.Counter,
                ["combat_orbs"] = rng.CombatOrbGeneration.Counter,
            };
            var prng = (runState?.Players != null && runState.Players.Count > 0)
                ? runState.Players[0].PlayerRng
                : null;
            if (prng != null)
            {
                c["rewards"] = prng.Rewards.Counter;
                c["shops"] = prng.Shops.Counter;
                c["transformations"] = prng.Transformations.Counter;
            }
            d["run_seed"] = seed;
            d["counters"] = c;
        }
        catch
        {
            // All-or-nothing: hand back an EMPTY block, which every consumer
            // already treats as "this producer does not carry the run's dice".
            d.Clear();
        }
        return d;
    }

    public static Dictionary<string, object?> SerializeRunState(RunState? runState, Player? player)
    {
        try
        {
            var potions = new List<Dictionary<string, object?>>();
            if (player != null)
            {
                int slot = 0;
                foreach (PotionModel? potion in player.PotionSlots)
                {
                    potions.Add(SerializePotionSlot(potion, slot));
                    slot++;
                }
            }

            var relics = new List<Dictionary<string, object?>>();
            if (player != null)
            {
                foreach (RelicModel relic in player.Relics)
                    relics.Add(SerializeRelic(relic));
            }

            var deck = new List<Dictionary<string, object?>>();
            if (player != null)
            {
                foreach (CardModel card in player.Deck.Cards)
                    deck.Add(SerializeCard(card));
            }

            return new Dictionary<string, object?>
            {
                ["gold"] = player?.Gold ?? 0,
                ["hp"] = player?.Creature?.CurrentHp ?? 0,
                ["max_hp"] = player?.Creature?.MaxHp ?? 0,
                ["potions"] = potions,
                ["potion_slot_count"] = player?.MaxPotionCount ?? 0,
                ["relics"] = relics,
                ["master_deck"] = deck,
                ["ascension"] = runState?.AscensionLevel ?? 0,
                ["character"] = player?.Character?.Id.Entry,
                ["unlocked_epochs"] = SerializeUnlockedEpochs(runState, player),
            };
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing run_state: {ex.Message}");
            return new Dictionary<string, object?> { ["error"] = "run_state_serialization_failed" };
        }
    }

    /// <summary>
    /// The epoch ids this run's UnlockState has REVEALED (GH #374).
    ///
    /// Every character card pool overrides
    /// <c>CardPoolModel.FilterThroughEpochs(UnlockState, cards)</c> to prune
    /// cards behind unrevealed epochs, and <c>GetUnlockedCards</c>
    /// (CardPoolModel.cs:83-96) runs that filter before ANY consumer sees the
    /// pool -- combat generation, card rewards and shop stock alike. The
    /// engine had no UnlockState concept, so its pools ran too large and its
    /// combat_card_generation cursor drifted by exactly the missing epoch's
    /// size (GH #367).
    ///
    /// Stage 1 inferred the save's unlock state from a corpus census and
    /// hardcoded it in the Python driver. That is a snapshot of one save at
    /// one moment: it goes stale silently the instant the timeline advances,
    /// and it is wrong for anyone else's save. This makes the producer say it.
    ///
    /// Source choice matters. <c>UnlockState</c>'s own doc-comment warns that
    /// using <c>SaveManager.Instance.Progress</c> during a run can include
    /// epochs unlocked between a save and a load, which would diverge from
    /// what the run itself is generating against. So this reads the RUN's
    /// UnlockState (RunState.cs:499 -- the superset over all players, built at
    /// RunState.cs:616 from each Player.UnlockState), falling back to the
    /// single player's when there is no run yet.
    ///
    /// Emits the raw <c>ToSerializable().UnlockedEpochs</c> list, i.e. what C#
    /// considers REVEALED, and lets the consumer decide what "locked" means
    /// for the epochs it models -- C# carries many epochs the engine does not
    /// (Relic1-5, Silent1/3/4/6, Underdocks, Darv...), so inverting here would
    /// bake this engine's coverage into the wire.
    ///
    /// Returns null (NOT an empty list) when the unlock state is unavailable.
    /// The distinction is load-bearing: an empty list is a legitimate
    /// UnlockState.none -- a save that has revealed NOTHING -- whereas null
    /// means "this producer could not tell you", and a consumer that treated
    /// the two alike would lock all 60 gated cards on a serialization slip.
    /// </summary>
    private static List<string>? SerializeUnlockedEpochs(RunState? runState, Player? player)
    {
        try
        {
            UnlockState? unlocks = runState?.UnlockState ?? player?.UnlockState;
            return unlocks?.ToSerializable()?.UnlockedEpochs;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing unlocked_epochs: {ex.Message}");
            return null;
        }
    }

    // ----------------------------------------------------------------
    // Cards -- shared serializer for hand/piles/deck/rewards/selects.
    // ----------------------------------------------------------------

    /// <summary>
    /// Serializes a single card. <paramref name="includeLegality"/> adds
    /// per-choice playability fields (playable/unplayable_reason/
    /// valid_target_indices) -- only meaningful when the card is a live
    /// hand-play choice; leave false for deck/pile/reward listings.
    /// </summary>
    public static Dictionary<string, object?> SerializeCard(CardModel card, bool includeLegality = false)
    {
        var data = new Dictionary<string, object?>
        {
            ["id"] = card.Id.Entry,
            ["name"] = card.Title,
            ["description"] = SafeDescription(card),
            ["cost"] = SafeCost(card),
            ["base_cost"] = SafeBaseCost(card),
            // What `base_cost` becomes once this turn's cost modifiers wear
            // off. Only present when it can differ — see SafePersistentCost.
            ["persistent_cost"] = SafePersistentCost(card),
            // What `base_cost` becomes the moment the card is PLAYED — a
            // different boundary from persistent_cost. See SafePlayedCost.
            ["played_cost"] = SafePlayedCost(card),
            ["stable_cost"] = SafeStableCost(card),
            ["effective_cost"] = SafeEffectiveCost(card),
            ["costs_x"] = card.EnergyCost.CostsX,
            ["type"] = card.Type.ToString(),
            ["rarity"] = card.Rarity.ToString(),
            ["target_type"] = card.TargetType.ToString(),
            ["upgrade_level"] = card.CurrentUpgradeLevel,
            ["is_upgradable"] = card.IsUpgradable,
            // GH #592. The game's OWN verdict on whether this card can appear
            // on a removal screen -- `CardModel.IsRemovable`, the single
            // filter `CardSelectCmd.FromDeckForRemoval` applies. The shop's
            // remove_card fan-out used to reconstruct this client-side from
            // the card id plus the `enchantment` field, and got it wrong twice:
            // once offering Eternal curses (Ascender's Bane, Enthralled) the
            // screen withholds, and once on a plain `OUTBREAK` with no
            // enchantment on the wire at all -- 25 of 27 deck cards offered,
            // the two missing ones exactly the two the fan-out over-offered
            // (logs/f7-mirror-check19, Silent A8 floor 42). A reconstruction
            // cannot be audited against a keyword set it cannot see; the
            // producer can just say.
            ["is_removable"] = card.IsRemovable,
            ["exhaust"] = card.Keywords.Contains(CardKeyword.Exhaust),
            ["innate"] = card.Keywords.Contains(CardKeyword.Innate),
            ["retain"] = card.Keywords.Contains(CardKeyword.Retain),
            ["ethereal"] = card.Keywords.Contains(CardKeyword.Ethereal),
            // sts2-agent fork (D1b): the two keywords the flags above left out. Without them a
            // consumer could only infer Unplayable (from unplayable_reason in hand, a missing
            // base cost elsewhere) and never see Eternal (a curse no removal takes: Greed).
            ["unplayable"] = card.Keywords.Contains(CardKeyword.Unplayable),
            ["eternal"] = card.Keywords.Contains(CardKeyword.Eternal),
            ["will_exhaust"] = SafeBool(() => card.ExhaustOnNextPlay),
            ["will_retain"] = SafeBool(() => card.ShouldRetainThisTurn),
            // GH #370. The Sly twins of `retain` / `will_retain` above. `sly`
            // is the keyword set (intrinsic, plus whatever `AddKeyword` put
            // there — MasterPlanner marks a played Skill Sly for the REST OF
            // THE COMBAT that way); `sly_this_turn` additionally folds in the
            // private single-turn `_hasSingleTurnSly` that Hand Trick sets and
            // `EndOfTurnCleanup` clears. The consumer needs both to tell the
            // two lifetimes apart — see CardModel.cs:620.
            ["sly"] = card.Keywords.Contains(CardKeyword.Sly),
            ["sly_this_turn"] = SafeBool(() => card.IsSlyThisTurn),
            ["is_curse"] = card.Type == CardType.Curse,
            ["is_status"] = card.Type == CardType.Status,
            ["pile"] = SafeString(() => card.Pile?.Type.ToString()),
            // The card's DynamicVars as lowercased name -> BaseValue — the
            // instance's CURRENT base stats (post-upgrade, post-enchantment,
            // post-permanent-growth). Twin of the headless
            // RunSimulator.SerializeCombatCard `stats` block; the shared
            // consumer (sts2_hint.adapter) maps `damage`/`block` onto the
            // engine's base_damage/base_block. Without this the live arm
            // rebuilt every Sharp/Nimble-enchanted card and every Rampage-
            // style permanent growth at its registry value — a per-play
            // planning error the DLL arm did not have. Null when empty.
            ["stats"] = CardStats(card),
        };

        // Kept conditional (only present when true) to match the pre-phase-1
        // schema's "upgraded" semantics that existing consumers may already
        // rely on; `upgrade_level`/`is_upgradable` above are the new,
        // always-present replacements.
        if (card.IsUpgraded)
            data["upgraded"] = true;

        // Per-card affliction (Bound from the Queen's ChainsOfBinding,
        // Tainted from Infested Prism, ...). Wire name = the C# class name,
        // which sts2_rs's CardAfflictionKind mapping pins 1:1. Without it a
        // reconstructed search state plans as if every afflicted card were
        // freely playable (live sweep-9 Queen forensics, 2026-07-13).
        AfflictionModel? affliction = null;
        try
        {
            affliction = card.Affliction;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error reading affliction for {SafeCardId(card)}: {ex.Message}");
        }
        if (affliction != null)
        {
            data["affliction"] = affliction.GetType().Name;
            int stacks = 1;
            try { stacks = affliction.Amount; } catch { }
            data["affliction_stacks"] = stacks;
        }

        // Run-persistent enchantment (Nimble/Sharp/Steady/...). Emitted as
        // the canonical Id.Entry ("NIMBLE") — the engine resolves it via
        // `EnchantmentId::from_wire_name`, which normalizes case/punctuation
        // and accepts both this form and the headless localized title.
        // Amount follows the headless convention: only present when nonzero
        // (the engine reads absent as one stack — the scaling enchantments
        // Nimble/Sharp put their whole payload here). Without these the live
        // reconstruction dropped every enchantment outright: missing Retain
        // (Steady), missing +N block/damage (Nimble/Sharp), missing cost
        // edits already folded into the cost fields but re-derivable hooks
        // lost. Mirrors RunSimulator.SerializeCombatCard.
        EnchantmentModel? enchantment = null;
        try
        {
            enchantment = card.Enchantment;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error reading enchantment for {SafeCardId(card)}: {ex.Message}");
        }
        if (enchantment != null)
        {
            data["enchantment"] = SafeString(() => enchantment.Id.Entry);
            int enchAmount = 0;
            try { enchAmount = enchantment.Amount; } catch { }
            if (enchAmount != 0)
                data["enchantment_amount"] = enchAmount;
        }

        if (includeLegality)
        {
            bool playable = false;
            string? unplayableReason = null;
            try
            {
                playable = card.CanPlay(out UnplayableReason reason, out AbstractModel? _);
                unplayableReason = reason == UnplayableReason.None ? null : reason.ToString();
            }
            catch (Exception ex)
            {
                Logger.Log($"[SimStateExtractor] Error computing playability for {SafeCardId(card)}: {ex.Message}");
            }

            data["playable"] = playable;
            data["unplayable_reason"] = unplayableReason;
            data["valid_target_indices"] = ComputeValidTargetIndices(card);
        }

        return data;
    }

    private static List<int> ComputeValidTargetIndices(CardModel card)
    {
        var indices = new List<int>();
        try
        {
            if (card.TargetType != TargetType.AnyEnemy)
                return indices;

            ICombatState? cs = card.CombatState;
            if (cs == null)
                return indices;

            IReadOnlyList<Creature> enemies = cs.Enemies;
            for (int i = 0; i < enemies.Count; i++)
            {
                if (card.IsValidTarget(enemies[i]))
                    indices.Add(i);
            }
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error computing valid_target_indices for {SafeCardId(card)}: {ex.Message}");
        }
        return indices;
    }

    private static string SafeCardId(CardModel card)
    {
        try { return card.Id.Entry; } catch { return "UNKNOWN"; }
    }

    /// <summary>
    /// The card's DynamicVar bag as lowercased name -> (int)BaseValue.
    /// Kept identical on purpose to the headless
    /// <c>RunSimulator.SerializeCombatCard</c> stats block (and to this
    /// file's <see cref="PowerVars"/> for powers) so both producers hand the
    /// shared consumer the same shape. Null (field present, value null)
    /// rather than an empty dict when there is nothing to say; any failure
    /// is a lost fidelity nicety, never a reason to drop the card.
    /// </summary>
    private static Dictionary<string, object?>? CardStats(CardModel card)
    {
        try
        {
            var vars = card.DynamicVars?.Values;
            if (vars == null) return null;
            var stats = new Dictionary<string, object?>();
            foreach (var dv in vars)
                stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue;
            return stats.Count > 0 ? stats : null;
        }
        catch { return null; }
    }

    private static string SafeDescription(CardModel card)
    {
        try { return card.GetDescriptionForPile(PileType.None); }
        catch { return ""; }
    }

    private static int SafeCost(CardModel card)
    {
        try { return card.EnergyCost.GetWithModifiers(CostModifiers.All); }
        catch { return card.EnergyCost.Canonical; }
    }

    /// <summary>
    /// The card instance's OWN cost: upgrade-adjusted base with its LOCAL
    /// modifiers applied (Stomp's per-attack shave, Eidolon's until-played
    /// zero) and the GLOBAL power hooks (FreeAttack, Tangled, Enthralled)
    /// excluded — exactly what the engine keeps in `Card.cost`, so a state
    /// reconstruction can rebuild from it and re-derive the globals from the
    /// powers instead of baking a transient discount in permanently.
    ///
    /// NOT <c>Canonical</c>, which is the *printed* cost and therefore misses
    /// permanent upgrade cost changes as well (decomp: <c>CostModifiers.None</c>
    /// reflects an upgrade, <c>Canonical</c> does not).
    ///
    /// -1 is the engine's single "not a real cost" sentinel, covering both
    /// X-cost cards and unplayable ones (its Dazed/Burn specs use -1 the same
    /// way its Whirlwind/Volley specs do); the game reports 0 for X and clamps
    /// unplayables to 0, either of which is indistinguishable from a genuinely
    /// free card.
    ///
    /// Null when the cost model cannot be read at all — NOT -1, which is a
    /// meaningful value the consumer decodes as X-cost/unplayable. The old
    /// fallback (<c>card.EnergyCost.Canonical</c>) sat outside any guard and
    /// would itself throw on the most likely cause of getting here, a null or
    /// throwing <c>EnergyCost</c>. Same nullable shape as the headless
    /// <c>RunSimulator.BaseEnergyCost</c>; kept identical on purpose.
    /// </summary>
    private static int? SafeBaseCost(CardModel card)
    {
        try
        {
            var ec = card.EnergyCost;
            if (ec == null) return null;
            if (ec.CostsX) return -1;
            var v = ec.GetWithModifiers(CostModifiers.Local);
            return v < 0 ? -1 : v;
        }
        catch { return null; }
    }

    // CardEnergyCost._localModifiers — the one piece of the cost model with no
    // public accessor. LocalCostModifier and every member read off it are
    // public, so this is a single field read, not a reimplementation.
    private static readonly System.Reflection.FieldInfo? LocalModifiersField =
        typeof(CardEnergyCost).GetField(
            "_localModifiers",
            System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);

    /// <summary>
    /// What <see cref="SafeBaseCost"/> becomes once this turn's cost modifiers
    /// wear off — the card's cost after C# <c>EndOfTurnCleanup()</c>.
    ///
    /// A local modifier carries a LIFETIME (EndOfCombat / EndOfTurn /
    /// WhenPlayed) that one number cannot express. Stomp's "1 less per Attack
    /// played THIS TURN" and Enlightenment's "costs 1 for the rest of COMBAT"
    /// both arrive as a cheaper card, but only the first springs back at turn
    /// start — so a state rebuilt from <c>base_cost</c> alone keeps every
    /// transient discount forever. Same field, same reasoning, as the headless
    /// runner's <c>PersistentEnergyCost</c>; kept identical on purpose.
    ///
    /// Null when it cannot differ from <c>base_cost</c> (no local modifiers,
    /// X-cost, canonical/negative base) or when the read fails.
    /// </summary>
    private static int? SafePersistentCost(CardModel card) =>
        SafeFoldLocalModifiers(card, LocalCostModifierExpiration.EndOfTurn);

    /// <summary>
    /// What this card's cost becomes once it has been PLAYED — the local
    /// modifiers re-folded without the WhenPlayed ones, mirroring
    /// CardEnergyCost.AfterCardPlayedCleanup(). The game's "costs N this turn"
    /// helpers are really SetThisTurnOrUntilPlayed (EndOfTurn | WhenPlayed,
    /// whichever comes first), so this is NOT the same number as
    /// SafePersistentCost, and a consumer that only expires discounts at the
    /// turn boundary keeps a spent one alive on the played card.
    /// </summary>
    private static int? SafePlayedCost(CardModel card) =>
        SafeFoldLocalModifiers(card, LocalCostModifierExpiration.WhenPlayed);

    /// <summary>
    /// The cost once BOTH transient edges have passed. Load-bearing, though not
    /// because the other two collide: one combined `EndOfTurn | WhenPlayed`
    /// modifier folds to `(persistent, played) = (base, base)` while two
    /// separate modifiers fold to `(M2(base), M1(base))`, which differ unless
    /// both are no-ops. What the pair cannot say is how much of the shave
    /// SURVIVES the first edge — combined, the whole shave goes at whichever
    /// fires first; split, each edge takes only its own half. This third number
    /// is the fixed point that separates those. Kept identical on purpose to
    /// the headless `RunSimulator.StableEnergyCost`.
    /// </summary>
    private static int? SafeStableCost(CardModel card) =>
        SafeFoldLocalModifiers(
            card, LocalCostModifierExpiration.EndOfTurn | LocalCostModifierExpiration.WhenPlayed);

    private static int? SafeFoldLocalModifiers(CardModel card, LocalCostModifierExpiration expiring)
    {
        try
        {
            var ec = card.EnergyCost;
            if (ec == null || ec.CostsX || !ec.HasLocalModifiers || card.IsCanonical) return null;
            // GetWithModifiers(None) is the private `_base`: the permanent,
            // upgrade-adjusted cost to re-fold from.
            var cost = ec.GetWithModifiers(CostModifiers.None);
            if (cost < 0) return null;
            if (LocalModifiersField?.GetValue(ec) is not IEnumerable<LocalCostModifier> mods) return null;
            foreach (var m in mods)
            {
                if ((m.Expiration & expiring) != 0) continue;
                cost = m.Modify(cost);
            }
            return Math.Max(0, cost);
        }
        catch { return null; }
    }

    private static int SafeEffectiveCost(CardModel card)
    {
        try { return card.EnergyCost.GetAmountToSpend(); }
        catch { return SafeCost(card); }
    }

    private static bool SafeBool(Func<bool> f)
    {
        try { return f(); } catch { return false; }
    }

    private static string? SafeString(Func<string?> f)
    {
        try { return f(); } catch { return null; }
    }

    /// <summary>
    /// Full pile contents in engine order. For the draw pile, index 0 is the
    /// next card that will be drawn (verified against CardPile.cs/
    /// CardPileCmd.Draw in the v0.107.1 decomp -- FirstOrDefault() is
    /// explicitly the "Top"/next-drawn card).
    /// </summary>
    public static Dictionary<string, object?> SerializePile(CardPile? pile, bool includeLegality = false)
    {
        var cards = new List<Dictionary<string, object?>>();
        try
        {
            if (pile != null)
            {
                foreach (CardModel card in pile.Cards)
                    cards.Add(SerializeCard(card, includeLegality));
            }
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing pile: {ex.Message}");
        }

        return new Dictionary<string, object?>
        {
            ["type"] = pile?.Type.ToString(),
            ["count"] = pile?.Cards.Count ?? 0,
            ["cards"] = cards,
        };
    }

    // ----------------------------------------------------------------
    // Relics / potions
    // ----------------------------------------------------------------

    /// The once-per-combat latches no public accessor exposes, keyed by relic
    /// id. Kept identical on purpose to <c>RunSimulator.RelicLatchFields</c>.
    ///
    /// These relics declare no counter — <c>ShowCounter</c> is false and there
    /// is no UI badge — but their spent/unspent bit IS the state a
    /// reconstruction must not lose: a mirror that rebuilds Vambrace unspent
    /// grants its block a second time in the same fight. Polarity is
    /// 1 = already spent, matching the engine's <c>VAMBRACE_USED = 1</c>.
    private static readonly System.Reflection.FieldInfo? VambraceLatch =
        LatchField("Vambrace", "_blockGainedThisCombat");

    private static readonly System.Reflection.FieldInfo? RuinedHelmetLatch =
        LatchField("RuinedHelmet", "_usedThisCombat");

    // First-Power-card-this-combat 7-block grant (v27wr sweep: 51 re-armed
    // re-grants at exactly -7). Same allowlist entry as RunSimulator's.
    private static readonly System.Reflection.FieldInfo? PermafrostLatch =
        LatchField("Permafrost", "_activatedThisCombat");

    private static System.Reflection.FieldInfo? LatchField(string typeName, string fieldName)
    {
        try
        {
            var t = typeof(RelicModel).Assembly.GetType(
                "MegaCrit.Sts2.Core.Models.Relics." + typeName);
            return t?.GetField(
                fieldName,
                System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);
        }
        catch { return null; }
    }

    private static int? SafeRelicCounter(RelicModel relic)
    {
        try
        {
            if (relic.ShowCounter) return relic.DisplayAmount;
            var latch = relic.Id.Entry switch
            {
                "VAMBRACE" => VambraceLatch,
                "RUINED_HELMET" => RuinedHelmetLatch,
                "PERMAFROST" => PermafrostLatch,
                _ => null,
            };
            if (latch == null) return null;
            return (latch.GetValue(relic) is bool b && b) ? 1 : 0;
        }
        catch { return null; }
    }

    /// <summary>
    /// Relic counter: uses the generic public RelicModel.ShowCounter/
    /// DisplayAmount pair (the same mechanism the game's own UI badge uses)
    /// rather than casting to ~30 individual relic subclasses for their
    /// bespoke [SavedProperty] counter fields (PenNib.AttacksPlayed,
    /// Girya.TimesLifted, etc -- see phase1-decomp.md section 4). This
    /// covers every relic with a visible counter, not just a hardcoded list --
    /// plus the two counter-less once-per-combat latches, see
    /// <see cref="SafeRelicCounter"/>.
    /// </summary>
    public static Dictionary<string, object?> SerializeRelic(RelicModel relic)
    {
        try
        {
            return new Dictionary<string, object?>
            {
                ["id"] = relic.Id.Entry,
                ["name"] = relic.Title.GetFormattedText(),
                ["description"] = relic.DynamicDescription.GetFormattedText(),
                ["rarity"] = relic.Rarity.ToString(),
                ["counter"] = SafeRelicCounter(relic),
                ["stack_count"] = relic.StackCount,
                ["is_wax"] = relic.IsWax,
                ["is_melted"] = relic.IsMelted,
            };
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing relic: {ex.Message}");
            return new Dictionary<string, object?> { ["error"] = "relic_serialization_failed" };
        }
    }

    public static Dictionary<string, object?> SerializePotionSlot(PotionModel? potion, int slot)
    {
        if (potion == null)
        {
            return new Dictionary<string, object?> { ["slot"] = slot, ["empty"] = true };
        }

        try
        {
            TargetType targetType = potion.TargetType;
            return new Dictionary<string, object?>
            {
                ["slot"] = slot,
                ["empty"] = false,
                ["id"] = potion.Id.Entry,
                ["name"] = potion.Title.GetFormattedText(),
                ["description"] = potion.DynamicDescription.GetFormattedText(),
                ["rarity"] = potion.Rarity.ToString(),
                ["usage"] = potion.Usage.ToString(),
                ["can_use"] = potion.Usage != PotionUsage.Automatic,
                ["target_type"] = targetType.ToString(),
                ["requires_target"] = targetType == TargetType.AnyEnemy,
            };
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing potion at slot {slot}: {ex.Message}");
            return new Dictionary<string, object?> { ["slot"] = slot, ["error"] = "potion_serialization_failed" };
        }
    }

    // ----------------------------------------------------------------
    // Powers / intents / creatures
    // ----------------------------------------------------------------

    /// <summary>
    /// A power's DynamicVar bag as name -> BaseValue. Kept identical on purpose
    /// to <c>RunSimulator.PowerVars</c> so the two arms hand the shared
    /// <c>sts2_hint.adapter._POWER_VAR_TO_ENGINE_FIELD</c> table the same shape.
    ///
    /// <c>BaseValue</c>, not <c>IntValue</c>: the accumulators the reconstruction
    /// needs (CrimsonMantle/Inferno <c>SelfDamage</c>) are stored there, and
    /// that is the field the headless twin reads.
    ///
    /// Returns null rather than an empty dict when there is nothing to say, and
    /// swallows any per-power failure — a missing var bag is a lost fidelity
    /// nicety, never a reason to drop the whole power.
    /// </summary>
    private static Dictionary<string, object?>? PowerVars(PowerModel power)
    {
        try
        {
            var vars = power.DynamicVars?.Values;
            if (vars == null) return null;
            var d = new Dictionary<string, object?>();
            foreach (var dv in vars)
                d[dv.Name] = (int)dv.BaseValue;
            return d.Count > 0 ? d : null;
        }
        catch { return null; }
    }

    /// <summary>
    /// Monsters whose cross-decision AI state lives in a PRIVATE field on the
    /// mutable <see cref="MonsterModel"/> — kept identical on purpose to
    /// <c>RunSimulator.AppendMonsterHiddenState</c> so both producers disclose
    /// the same allowlist under the same field names.
    ///
    /// - <c>Fabricator._lastSpawned</c> (Fabricator.cs:37): the bot kind
    ///   excluded from the next SpawnBot pool; unrecoverable from the wire
    ///   once that bot died (the max-uid inference the engine falls back to).
    /// - <c>KnowledgeDemon._curseOfKnowledgeCounter</c> (KnowledgeDemon.cs:66):
    ///   indexes the curse card set and the Disintegration 6/7/8 damage table
    ///   and gates CURSE_OF_KNOWLEDGE re-entry — the DISINTEGRATION
    ///   off-by-one when rebuilt at 0.
    /// </summary>
    private static readonly System.Reflection.FieldInfo? FabricatorLastSpawnedField =
        MonsterField("Fabricator", "_lastSpawned");

    private static readonly System.Reflection.FieldInfo? KnowledgeDemonCurseCounterField =
        MonsterField("KnowledgeDemon", "_curseOfKnowledgeCounter");

    private static System.Reflection.FieldInfo? MonsterField(string typeName, string fieldName)
    {
        try
        {
            var t = typeof(MonsterModel).Assembly.GetType(
                "MegaCrit.Sts2.Core.Models.Monsters." + typeName);
            return t?.GetField(
                fieldName,
                System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);
        }
        catch { return null; }
    }

    /// <summary>
    /// Appends the allowlisted private-AI-state fields for this monster to an
    /// already-built creature dict. Quiet for monsters not on the allowlist
    /// and for a Fabricator that has not spawned yet (absent field ==
    /// "nothing to disclose"). Wire shape: <c>fabricator_last_spawned</c> =
    /// the spawned bot's canonical <c>MonsterModel.Id.Entry</c>
    /// (e.g. <c>"ZAPBOT"</c>); <c>curse_of_knowledge_counter</c> = raw int.
    /// </summary>
    private static void AppendMonsterHiddenState(Dictionary<string, object?> d, MonsterModel? m)
    {
        if (m == null) return;
        try
        {
            if (FabricatorLastSpawnedField?.DeclaringType?.IsInstanceOfType(m) == true
                && FabricatorLastSpawnedField.GetValue(m) is MonsterModel last)
                d["fabricator_last_spawned"] = last.Id.Entry;
            if (KnowledgeDemonCurseCounterField?.DeclaringType?.IsInstanceOfType(m) == true
                && KnowledgeDemonCurseCounterField.GetValue(m) is int curse)
                d["curse_of_knowledge_counter"] = curse;
        }
        catch { }
    }

    public static Dictionary<string, object?> SerializePower(PowerModel power)
    {
        try
        {
            string description = "";
            try { description = power.DumbHoverTip.Description; } catch { }

            return new Dictionary<string, object?>
            {
                ["id"] = power.Id.Entry,
                ["name"] = power.Title.GetFormattedText(),
                ["amount"] = power.Amount,
                // `Amount` is not always the number the power runs on.
                // WitheringPresence's `Amount` is the static cycle length while
                // `DisplayAmount` is the live cards-left counter, so a mirror
                // rebuilt from `Amount` alone restarts every countdown at full.
                ["display_amount"] = power.DisplayAmount,
                // ...and neither number sees a per-instance accumulator, which
                // lives in a DynamicVar. CrimsonMantle's `Amount` is the block
                // it grants; the damage it deals you each turn is `SelfDamage`,
                // bumped once per copy played. Without this the engine rebuilds
                // it at zero and skips that damage entirely. `PowerVars` in
                // `RunSimulator` is the twin of this — keep them in step.
                ["vars"] = PowerVars(power),
                ["type"] = power.Type.ToString(),
                ["description"] = description,
            };
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing power: {ex.Message}");
            return new Dictionary<string, object?> { ["error"] = "power_serialization_failed" };
        }
    }

    /// <summary>
    /// Full multi-intent serialization (fixes phase1-current.md gap #7/#8:
    /// the old code only ever looked at Intents[0] and only computed
    /// damage/hits for AttackIntent). Every intent in the move -- attack,
    /// buff, debuff, etc -- is emitted; non-attack intents get the engine's
    /// own rendered label/description text (via the public
    /// AbstractIntent.GetHoverTip) instead of a bare type string.
    /// </summary>
    public static Dictionary<string, object?> SerializeIntent(AbstractIntent intent, Creature owner, IEnumerable<Creature> targets)
    {
        var data = new Dictionary<string, object?>
        {
            ["kind"] = intent.GetType().Name,
            ["intent_type"] = intent.IntentType.ToString(),
        };

        try
        {
            var tip = intent.GetHoverTip(targets, owner);
            data["title"] = tip.Title;
            data["description"] = tip.Description;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error getting intent hover tip: {ex.Message}");
        }

        if (intent is AttackIntent attackIntent)
        {
            try
            {
                int damage = attackIntent.GetSingleDamage(targets, owner);
                int hits = attackIntent.Repeats > 0 ? attackIntent.Repeats : 1;
                data["damage"] = damage;
                data["hits"] = hits;
                data["total_damage"] = damage * hits;
            }
            catch (Exception ex)
            {
                Logger.Log($"[SimStateExtractor] Error computing intent damage: {ex.Message}");
            }
        }

        return data;
    }

    /// <summary>
    /// Serializes a Creature (player, enemy, or ally/pet). Intents/move
    /// history are only emitted for monster-backed creatures (enemies,
    /// Osty-style pets when they have a telegraphed move).
    /// </summary>
    public static Dictionary<string, object?> SerializeCreature(Creature creature, bool includeIntents = true)
    {
        var data = new Dictionary<string, object?>
        {
            ["id"] = SafeCreatureId(creature),
            ["name"] = SafeString(() => creature.Name) ?? "UNKNOWN",
            ["side"] = creature.Side.ToString(),
            ["is_player"] = creature.IsPlayer,
            ["is_monster"] = creature.IsMonster,
            ["is_alive"] = creature.IsAlive,
            ["is_hittable"] = SafeBool(() => creature.IsHittable),
            ["hp"] = creature.CurrentHp,
            ["max_hp"] = creature.MaxHp,
            ["block"] = creature.Block,
            ["slot_name"] = creature.SlotName,
        };

        var powers = new List<Dictionary<string, object?>>();
        try
        {
            foreach (PowerModel power in creature.Powers)
                powers.Add(SerializePower(power));
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing powers: {ex.Message}");
        }
        data["powers"] = powers;

        if (includeIntents && creature.IsMonster && creature.Monster != null)
        {
            var intents = new List<Dictionary<string, object?>>();
            try
            {
                var nextMove = creature.Monster.NextMove;
                data["next_move_id"] = nextMove?.Id;
                if (nextMove?.Intents != null)
                {
                    IEnumerable<Creature> targets = creature.CombatState?.GetOpponentsOf(creature)
                        ?? Array.Empty<Creature>();
                    foreach (AbstractIntent intent in nextMove.Intents)
                        intents.Add(SerializeIntent(intent, creature, targets));
                }
            }
            catch (Exception ex)
            {
                Logger.Log($"[SimStateExtractor] Error serializing intents: {ex.Message}");
            }
            data["intents"] = intents;
            data["move_history"] = SerializeMoveHistory(creature);
        }

        // Allowlisted private AI state (Fabricator._lastSpawned, KnowledgeDemon
        // curse counter) — deliberately OUTSIDE the includeIntents gate: the
        // hidden counters are state, not telegraphing, and must survive any
        // intent-less serialization path. See AppendMonsterHiddenState.
        if (creature.IsMonster)
            AppendMonsterHiddenState(data, creature.Monster);

        return data;
    }

    private static string SafeCreatureId(Creature creature)
    {
        try
        {
            if (creature.IsMonster && creature.Monster != null)
                return creature.Monster.Id.Entry;
            if (creature.IsPlayer && creature.Player != null)
                return creature.Player.Character.Id.Entry;
        }
        catch { }
        return "UNKNOWN";
    }

    /// <summary>
    /// Last few move IDs this monster has actually performed, from the
    /// live combat history (CombatManager.Instance.History) -- cheap
    /// (bounded by one combat's entry count) and answers "has this monster
    /// used this move before" without any reflection (fixes
    /// phase1-current.md gap #17).
    /// </summary>
    private static List<string> SerializeMoveHistory(Creature enemyCreature, int maxEntries = 5)
    {
        var result = new List<string>();
        try
        {
            MonsterModel? monster = enemyCreature.Monster;
            if (monster == null)
                return result;

            var history = CombatManager.Instance?.History;
            if (history == null)
                return result;

            List<string> moveIds = history.Entries
                .OfType<MonsterPerformedMoveEntry>()
                .Where(e => e.Monster == monster)
                .Select(e => e.Move.Id)
                .ToList();

            int start = Math.Max(0, moveIds.Count - maxEntries);
            result = moveIds.Skip(start).ToList();
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing move history: {ex.Message}");
        }
        return result;
    }

    // ----------------------------------------------------------------
    // Orbs (Defect) / pets (Necrobinder Osty and friends)
    // ----------------------------------------------------------------

    public static List<Dictionary<string, object?>> SerializeOrbs(Player player, out int capacity)
    {
        var orbs = new List<Dictionary<string, object?>>();
        capacity = 0;
        try
        {
            OrbQueue? queue = player.PlayerCombatState?.OrbQueue;
            if (queue == null)
                return orbs;

            capacity = queue.Capacity;
            int index = 0;
            foreach (OrbModel orb in queue.Orbs)
            {
                orbs.Add(new Dictionary<string, object?>
                {
                    ["index"] = index,
                    ["id"] = orb.Id.Entry,
                    ["name"] = orb.Title.GetFormattedText(),
                    ["passive_value"] = orb.PassiveVal,
                    ["evoke_value"] = orb.EvokeVal,
                });
                index++;
            }
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing orbs: {ex.Message}");
        }
        return orbs;
    }

    public static List<Dictionary<string, object?>> SerializePets(Player player)
    {
        var pets = new List<Dictionary<string, object?>>();
        try
        {
            IReadOnlyList<Creature>? list = player.PlayerCombatState?.Pets;
            if (list == null)
                return pets;

            foreach (Creature pet in list)
                pets.Add(SerializeCreature(pet));
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing pets: {ex.Message}");
        }
        return pets;
    }

    // ----------------------------------------------------------------
    // Map graph -- called once per run-level screen (map_select's handler
    // plus ApplyEnvelope's MapCarryingTypes), never per combat decision.
    // ----------------------------------------------------------------

    public static Dictionary<string, object?> SerializeMapGraph(RunState runState)
    {
        var data = new Dictionary<string, object?>();
        try
        {
            ActMap map = runState.Map;

            var nodes = new List<Dictionary<string, object?>>();
            foreach (MapPoint mp in map.GetAllMapPoints())
            {
                var children = mp.Children
                    .Select(c => (object?)new Dictionary<string, object?> { ["col"] = c.coord.col, ["row"] = c.coord.row })
                    .ToList();
                nodes.Add(new Dictionary<string, object?>
                {
                    ["col"] = mp.coord.col,
                    ["row"] = mp.coord.row,
                    ["type"] = mp.PointType.ToString(),
                    ["children"] = children,
                });
            }

            MapCoord? current = runState.CurrentMapCoord;
            data["row_count"] = map.GetRowCount();
            data["col_count"] = map.GetColumnCount();
            data["current"] = current.HasValue
                ? new Dictionary<string, object?> { ["col"] = current.Value.col, ["row"] = current.Value.row }
                : null;

            try
            {
                MapPoint boss = map.BossMapPoint;
                var bossNode = new Dictionary<string, object?> { ["col"] = boss.coord.col, ["row"] = boss.coord.row };
                // The boss's IDENTITY, not just where its node sits. The PPO
                // encoder reads this into a categorical (`ppo/mapfeat.rs::
                // boss_id_of` -> the state token's boss slot) so deck-shaping
                // advice can condition on the fight it is preparing for.
                // Without it the sim lane sees the boss and the live lane does
                // not, and the feature is dead in the only place it matters.
                // Resolved separately so a failure here still ships the coord.
                try
                {
                    string? bossIdEntry = runState.Act?.BossEncounter?.Id?.Entry;
                    if (!string.IsNullOrEmpty(bossIdEntry))
                        bossNode["id"] = bossIdEntry;
                }
                catch (Exception ex)
                {
                    Logger.Log($"[SimStateExtractor] Error resolving boss encounter id: {ex.Message}");
                }
                data["boss"] = bossNode;
            }
            catch (Exception ex)
            {
                Logger.Log($"[SimStateExtractor] Error resolving boss map point: {ex.Message}");
                data["boss"] = null;
            }

            data["visited"] = runState.VisitedMapCoords
                .Select(c => (object?)new Dictionary<string, object?> { ["col"] = c.col, ["row"] = c.row })
                .ToList();
            data["nodes"] = nodes;
        }
        catch (Exception ex)
        {
            Logger.Log($"[SimStateExtractor] Error serializing map graph: {ex.Message}");
            data["error"] = "map_graph_serialization_failed";
        }
        return data;
    }
}
