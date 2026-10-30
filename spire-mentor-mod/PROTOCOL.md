# Spire Mentor wire protocol (v3 / advisor mode; v2 = phase 1b)

Newline-delimited JSON over a single-client TCP socket on `127.0.0.1:<port>`
(default `9002`, see `SpireMentor.conf`). One JSON object per line.

- **Game -> Agent**: state messages (`combat_action`, `map_select`, ...).
  Every state message that expects a reply carries a `request_id`
  (monotonically increasing string) injected by the server; the reply must
  echo it back as `request_id` for the server to accept it.
- **Agent -> Game**: action messages (`play`, `end_turn`, `choose`, ...).
- Only one request is in flight at a time. A reply whose `request_id`
  doesn't match the currently pending one is dropped.
- `game_over` and `run_complete` are **fire-and-forget notifications** --
  no `request_id`, no reply expected/accepted.
- `error` (new, phase 1b) is also a **fire-and-forget notification** -- see
  section 3a. It is always followed by a fresh re-send of the current
  decision message (a normal, request_id-bearing message) on the next
  iteration of whatever loop rejected the action.
- `ping` (`{"action":"ping"}`, case-insensitive, or legacy
  `{"type":"PING"}`) gets an immediate `{"type":"pong"}` reply, bypassing
  the request/reply correlation entirely.

This document supersedes any reference to `sts2-rl-agent/docs/PROTOCOL.md`
(that file does not exist in this repo).

## Phase 1b changes at a glance

- **Potions**: `potion` verb payload extended to `{slot, target_index?, discard?}`
  -- see section 4.
- **Multi-card select**: `card_select` now declares `min_count`/`max_count`
  (aliases of `min_select`/`max_select`) and `cancelable`; accepts
  `{"action":"choose","indices":[...]}` (new spelling, `indexes` also still
  accepted) and `{"action":"cancel"}` -- see section 3 (`card_select`) and 4.
- **Client-driven run start**: new `run_config`/`start_run` handshake, gated
  by the `client_run_config` config flag (default off) -- see section 3
  (`run_config`) and section 4 (`start_run`).
- **Stale-state guard**: every outbound message now carries a `state_id`
  (monotonically increasing integer); inbound actions may optionally echo it
  back -- see section 1 and section 4.
- **Structured errors everywhere**: a new `error` outbound message type --
  see section 3a.

---

## 1. Common envelope

**Every outbound state message** (including `game_over`/`run_complete`, but
not `pong`) carries these top-level fields in addition to its own
message-specific ones:

| Field | Type | Meaning |
|---|---|---|
| `type` | string | Message type discriminator (see section 3). |
| `seed` | string | `RunState.Rng.StringSeed`, `""` if no active run. |
| `act` | int | 1-based act number (`CurrentActIndex + 1`), `1` if no active run. |
| `floor` | int | `RunState.TotalFloor` (floor across the whole run), `0` if no active run. |
| `mod_version` | string | Spire Mentor mod version (e.g. `"0.1.0"`). |
| `run_state` | object | See below. Always present, degrades to defaults/empty collections if no run/player is active. |
| `state_id` | int | **New, phase 1b.** Monotonically increasing id, assigned fresh every time this envelope is built (`SimStateExtractor.ApplyEnvelope`/`SimProtocolHelpers.NextStateId`). See section 4's "stale-state guard" for how inbound actions may reference it. |
| `request_id` | string | Present on every message that expects a reply (everything except `game_over`/`run_complete`/`error`/`pong`); injected by the transport layer, not by the handler. |
| `map` | object | **New, 2026-08-02 run-level dialect audit.** The full act map graph (same object as `map_select`'s `map` field), attached by `ApplyEnvelope` to every **run-level** decision message (`event`/`rest_site`/`shop`/`card_reward`/`reward_screen`/`treasure`/`boss_relic`/`card_select`/`card_bundle`/`crystal_sphere`) while a run is active. Deliberately absent from `combat_action` (sent ~4x/s; the graph walk is per-screen cost) and from `error`/terminal messages. Before this field, every off-map decision (Neow, the act-start Ancient relic offer, pre-first-map screens) was answered map-blind on the live arm while the headless harness seeded the same view via `get_map`. |

### `run_state` object

| Field | Type | Meaning |
|---|---|---|
| `gold` | int | Player's current gold. |
| `hp` / `max_hp` | int | Player creature HP (also duplicated per-message where historically present, e.g. `combat_action.player.hp`). |
| `potions` | array | One entry per potion slot, in slot order, **including empty slots** -- see "Potion object" below. Fixes the old behavior of silently omitting empty slots. |
| `potion_slot_count` | int | Total number of potion slots (`Player.MaxPotionCount`). |
| `relics` | array | Every owned relic -- see "Relic object" below. |
| `master_deck` | array | The player's full deck (`Player.Deck.Cards`), in engine order -- see "Card object" below. |
| `ascension` | int | Ascension level (0-10). |
| `character` | string\|null | Character `Id.Entry` (e.g. `"IRONCLAD"`), null if no active run. |
| `unlocked_epochs` | array\<string\>\|null | **New (GH #374).** The epoch ids this run's save has REVEALED — `RunState.UnlockState.ToSerializable().UnlockedEpochs`, falling back to `Player.UnlockState` when there is no run. C# prunes cards behind *un*revealed epochs out of every card pool (`CardPoolModel.GetUnlockedCards`), so a consumer that models epoch gating must invert this against its own epoch coverage; the raw revealed list is emitted rather than a locked list because C# carries epochs a consumer may not model (`Relic1-5`, `Silent1/3/4/6`, `Underdocks`, `Darv`, ...). **`null` means "could not determine", NOT "nothing revealed"** — an empty array is a legitimate `UnlockState.none`, so a consumer must not conflate them. Read from the RUN's unlock state, not `SaveManager.Instance.Progress`, per `UnlockState`'s own doc-comment: the latter can include epochs unlocked between a save and a load. |

---

## 2. Shared object schemas

### Card object

Used for `hand`, `draw_pile`/`discard_pile`/`exhaust_pile` contents,
`run_state.master_deck`, `card_reward.cards`, `card_select.cards`,
`card_bundle.bundles[].cards`, `shop` card/potion entries' nested `item`,
and `reward_screen` card-reward `reward` details. One shared serializer
(`SimStateExtractor.SerializeCard`) builds all of these, so the schema is
identical everywhere.

| Field | Type | Meaning |
|---|---|---|
| `id` | string | `CardModel.Id.Entry`. |
| `name` | string | Rendered title (includes the `+N` upgrade suffix the game itself shows). |
| `description` | string | Rendered card text for the current context (`GetDescriptionForPile`). |
| `cost` | int | `CardEnergyCost.GetWithModifiers(CostModifiers.All)` -- current cost with local + global (in-combat) modifiers folded in. For non-X cards this equals the headless wire's `cost` (`GetResolved()`), which the decomp defines as `Max(0, GetWithModifiers(All))`; for X-cost cards it is the base (0), NOT the captured X value. |
| `base_cost` | int\|null | The card instance's OWN cost: upgrade-adjusted base with its LOCAL modifiers applied and the global power hooks excluded -- `CardEnergyCost.GetWithModifiers(CostModifiers.Local)` (decomp `MegaCrit.Sts2.Core.Entities.Cards/CardEnergyCost.cs`). This is exactly what the sim engine stores in `Card.cost`, so state reconstruction prefers it and re-derives the globals from the powers on the same wire. `-1` = X-cost sentinel; null when the cost model is unreadable. Same semantics as headless `RunSimulator.BaseEnergyCost` -- NOT the canonical/printed cost. |
| `persistent_cost` | int\|null | What `base_cost` becomes once this turn's cost modifiers wear off -- the local modifiers re-folded from the permanent base (`GetWithModifiers(CostModifiers.None)`) skipping every `LocalCostModifierExpiration.EndOfTurn` one, mirroring the decomp's `CardEnergyCost.EndOfTurnCleanup()`. Null when it cannot differ from `base_cost` (no local modifiers, X-cost, canonical card). Distinguishes Stomp's "1 less per Attack this turn" (springs back at turn start) from Enlightenment's "costs 1 this combat" (doesn't). |
| `played_cost` | int\|null | What `base_cost` becomes the moment the card is PLAYED -- same re-fold skipping `LocalCostModifierExpiration.WhenPlayed`, mirroring `CardEnergyCost.AfterCardPlayedCleanup()`. The game's "costs N this turn" helpers are really `SetThisTurnOrUntilPlayed` (`EndOfTurn \| WhenPlayed`), so a consumer that only expires discounts at the turn boundary keeps a spent discount alive. Null when it cannot differ. |
| `stable_cost` | int\|null | The cost once BOTH transient edges have passed (re-fold skipping `EndOfTurn \| WhenPlayed`) -- tells a consumer whether a `(persistent, played)` pair came from one combined this-turn-or-until-played modifier or two independent ones. Null when it cannot differ. |
| `effective_cost` | int | **Authoritative for "what will be deducted".** `CardEnergyCost.GetAmountToSpend()` -- what the engine will actually deduct (handles X-cost cards, which report their owner's current energy here). |
| `costs_x` | bool | Whether this is an X-cost card (`CardEnergyCost.CostsX`). |
| `type` | string | `CardType`: `Attack`\|`Skill`\|`Power`\|`Status`\|`Curse`\|`Quest`. |
| `rarity` | string | `CardRarity`: `Basic`\|`Common`\|`Uncommon`\|`Rare`\|`Curse`\|`Status`\|`Event`\|`Quest`\|`Ancient`. |
| `target_type` | string | `TargetType`: `None`\|`Self`\|`AnyEnemy`\|`AllEnemies`\|`RandomEnemy`\|`AnyPlayer`\|`AnyAlly`\|`AllAllies`\|`TargetedNoCreature`\|`Osty`. |
| `upgrade_level` | int | `CurrentUpgradeLevel` (0 = not upgraded). |
| `is_upgradable` | bool | Whether another upgrade is available. |
| `is_removable` | bool | `CardModel.IsRemovable` (`!Keywords.Contains(Eternal)`) -- the game's own verdict on whether the card can appear on a removal screen, which is the ONLY filter `CardSelectCmd.FromDeckForRemoval` applies. Clients building a per-deck-card removal action set must use this rather than reconstructing it from the id plus `enchantment`: the keyword set it reads is not otherwise on the wire (GH #592). |
| `upgraded` | bool | **Only present (as `true`) if upgraded** -- legacy field, kept for backward compatibility; prefer `upgrade_level`. |
| `exhaust` / `innate` / `retain` / `ethereal` | bool | Keyword flags (from `CardModel.Keywords`, which already folds in hook-granted keywords like Hex-applied Ethereal). |
| `will_exhaust` | bool | `ExhaustOnNextPlay` -- a one-off exhaust flag independent of the `exhaust` keyword. |
| `will_retain` | bool | `ShouldRetainThisTurn` -- single-turn retain, independent of the `retain` keyword. |
| `is_curse` / `is_status` | bool | `type == Curse` / `type == Status`. |
| `pile` | string\|null | Which pile currently contains this card instance (`PileType`), null if untracked/not in a pile. |
| `stats` | object\|null | The card's `DynamicVars` as lowercased name -> `(int)BaseValue` (e.g. `{"damage": 9, "block": 5}`) -- the instance's CURRENT base stats (post-upgrade, post-enchantment, post-permanent-growth like Rampage). Twin of the headless `RunSimulator.SerializeCombatCard` `stats` block; the client maps `damage`/`block` onto the engine's `base_damage`/`base_block`. Null when the card has no dynamic vars. |
| `enchantment` | string | **Only present when enchanted.** Canonical `EnchantmentModel.Id.Entry` (e.g. `"NIMBLE"`); the engine resolves it via `EnchantmentId::from_wire_name` (case/punctuation-insensitive). |
| `enchantment_amount` | int | **Only present when nonzero.** `EnchantmentModel.Amount` -- the stack size (Nimble/Sharp's whole payload). Absent means one stack. |
| `affliction` | string | **Only present when afflicted.** The C# affliction class name (`"Bound"`, `"Tainted"`, ...). |
| `affliction_stacks` | int | **Only present with `affliction`.** `AfflictionModel.Amount`. |
| `playable` | bool | **Only present when serialized with legality** (currently: `combat_action.hand` only). `CardModel.CanPlay()`. |
| `unplayable_reason` | string\|null | **Legality-only.** Raw `UnplayableReason` flags as text (e.g. `"EnergyCostTooHigh"`, `"BlockedByHook, HasUnplayableKeyword"`), null if playable. |
| `valid_target_indices` | int[] | **Legality-only.** Indices into `enemies[]` that are legal targets for this card, computed via `CardModel.IsValidTarget`. Only populated for `target_type == "AnyEnemy"` (that's the only target type `play`'s `target_index` is used for); empty array otherwise. |

### Relic object

| Field | Type | Meaning |
|---|---|---|
| `id` | string | `RelicModel.Id.Entry`. |
| `name` | string | Rendered title. |
| `description` | string | Rendered dynamic description. |
| `rarity` | string | `RelicRarity`: `None`\|`Starter`\|`Common`\|`Uncommon`\|`Rare`\|`Shop`\|`Event`\|`Ancient`. |
| `counter` | int\|null | The relic's UI counter badge value (`RelicModel.DisplayAmount`), null if this relic doesn't show one (`ShowCounter == false`). Covers every relic with a visible counter (Pen Nib, Nunchaku, Girya, Pumpkin Candle, ...) generically via the same public API the game's own UI badge uses -- not a hardcoded per-relic list. |
| `stack_count` | int | For stackable relics (e.g. Circlet). |
| `is_wax` | bool | Wax Choker mechanic. |
| `is_melted` | bool | Melt mechanic. |

### Potion object (potion slot)

Empty slots are represented explicitly now (not omitted):

| Field | Type | Meaning |
|---|---|---|
| `slot` | int | Slot index. |
| `empty` | bool | `true` if this slot has no potion. |
| `id` | string | *(non-empty only)* `PotionModel.Id.Entry`. |
| `name` | string | *(non-empty only)* Rendered title. |
| `description` | string | *(non-empty only)* Rendered dynamic description. |
| `rarity` | string | *(non-empty only)* `PotionRarity`: `None`\|`Common`\|`Uncommon`\|`Rare`\|`Event`\|`Token`. |
| `usage` | string | *(non-empty only)* `PotionUsage`: `None`\|`CombatOnly`\|`AnyTime`\|`Automatic`. |
| `can_use` | bool | *(non-empty only)* `usage != "Automatic"`. |
| `target_type` | string | *(non-empty only)* Same `TargetType` enum as cards. |
| `requires_target` | bool | *(non-empty only)* `target_type == "AnyEnemy"`. Clients should key off THIS field, not `target_type`: `AnyEnemy` is the only target type for which the `potion` verb's `target_index` is consumed; every other type (`Self`, `AllEnemies`, `RandomEnemy`, `AnyPlayer`, `AnyAlly`, ...) auto-targets and `target_index` is ignored. |
| `use_now` | bool | *(sts2-agent fork; non-empty only, on a decision outside combat marked `potion_verb`)* The `potion` verb can drink it now: the game's popup would enable Use (`AnyTime`, not queued, `PassesCustomUsabilityCheck`, the owner alive and allowed to remove potions) and it needs no aiming (not `AnyEnemy`, not throwable at an ally), or it is Foul Potion (`TargetedNoCreature`) with a merchant to throw it at: in the shop, or where its own check passes (the fake merchant's event). |
| `discard_now` | bool | *(sts2-agent fork; same messages)* The `potion` verb can discard it now (not queued, the owner alive and allowed to remove potions). |

### Power object

Used for `player.powers` and each creature's `powers`.

| Field | Type | Meaning |
|---|---|---|
| `id` | string | `PowerModel.Id.Entry`. |
| `name` | string | Rendered title. |
| `amount` | int | Current stack amount. |
| `display_amount` | int | `PowerModel.DisplayAmount` -- the player-facing (and, for Counter powers like Slow/WitheringPresence, the actually-scaling) value; `amount` is only the stack count for those. |
| `vars` | object\|null | The power's `DynamicVars` as `{name: (int)BaseValue}` (e.g. CrimsonMantle/Inferno `{"SelfDamage": n}`, Thievery `{"Gold": n}`) -- the per-instance accumulators neither `amount` nor `display_amount` describes. Emitted for EVERY creature's powers (player, enemies, pets), same as the headless twin `RunSimulator.PowerVars` since the 2026-08-02 wire-completeness round (enemy powers were player-only there before). Null when the power has no vars. |
| `type` | string | `PowerType`: `Buff`\|`Debuff` (etc). |
| `description` | string | Rendered hover-tip description text. |

### Creature object (enemies / pets)

Used for each entry in `combat_action.enemies` and `combat_action.pets`.

| Field | Type | Meaning |
|---|---|---|
| `id` | string | Monster `Id.Entry`, or character `Id.Entry` if this creature is a player. |
| `name` | string | Rendered display name. |
| `side` | string | `CombatSide`. |
| `is_player` / `is_monster` | bool | |
| `is_alive` | bool | |
| `is_hittable` | bool | |
| `hp` / `max_hp` / `block` | int | |
| `slot_name` | string\|null | Positional slot identifier. |
| `powers` | Power[] | See above. |
| `next_move_id` | string\|null | **Monster only.** The telegraphed move's `MoveState.Id`. |
| `intents` | Intent[] | **Monster only. Full multi-intent** -- every intent in the telegraphed move, not just the first (fixes the old Intents[0]-only truncation). |
| `move_history` | string[] | **Monster only.** Up to the last 5 move IDs this monster has actually performed this combat (from `CombatManager.Instance.History`), oldest first -- lets an agent detect repeated/telegraphed patterns. |
| `fabricator_last_spawned` | string | **Fabricator only, absent until it has spawned.** Reflection export of the private `Fabricator._lastSpawned` (Fabricator.cs:37) as the bot's canonical `MonsterModel.Id.Entry` (`ZAPBOT`\|`STABBOT`\|`GUARDBOT`\|`NOISEBOT`). The next `SpawnBot` pool excludes this kind; without the field a reconstruction can only infer it from the highest-uid living bot, which is unrecoverable once that bot died. Same field on the headless wire (`RunSimulator.AppendMonsterHiddenState`). |
| `curse_of_knowledge_counter` | int | **Knowledge Demon only.** Reflection export of the private `KnowledgeDemon._curseOfKnowledgeCounter` (KnowledgeDemon.cs:66), 0..3 -- indexes the curse card set and the Disintegration damage table (6/7/8) and gates `CURSE_OF_KNOWLEDGE` re-entry at 3. Same field on the headless wire. |

### Intent object

| Field | Type | Meaning |
|---|---|---|
| `kind` | string | Concrete intent class name (e.g. `SingleAttackIntent`, `MultiAttackIntent`, `BuffIntent`, `DebuffIntent`, `DefendIntent`, `HealIntent`, `EscapeIntent`, `SummonIntent`, `StunIntent`, ...). |
| `intent_type` | string | `IntentType`: `Attack`\|`Buff`\|`Debuff`\|`DebuffStrong`\|`Defend`\|`Escape`\|`Heal`\|`Hidden`\|`Summon`\|`Sleep`\|`Stun`\|`StatusCard`\|`CardDebuff`\|`DeathBlow`\|`Unknown`. |
| `title` | string\|null | Rendered intent title (from the game's own hover-tip). |
| `description` | string\|null | Rendered intent description text (magnitude for buffs/debuffs/heals/etc is baked into this text by the game itself, since only attack intents expose a structured numeric field). |
| `damage` | int | **Attack intents only.** Single-hit damage (`AttackIntent.GetSingleDamage`, already includes Strength/hook modifiers). |
| `hits` | int | **Attack intents only.** Repeat count (`AttackIntent.Repeats`, floored at 1). |
| `total_damage` | int | **Attack intents only.** `damage * hits`. |

### Orb object

Used for `combat_action.orbs` (Defect only; empty array otherwise).

| Field | Type | Meaning |
|---|---|---|
| `index` | int | Position in the orb queue. |
| `id` | string | Orb `Id.Entry` (`LightningOrb`\|`FrostOrb`\|`DarkOrb`\|`PlasmaOrb`\|`GlassOrb`). |
| `name` | string | Rendered title. |
| `passive_value` | decimal | Passive-trigger magnitude. |
| `evoke_value` | decimal | Evoke magnitude. |

---

## 3. Outbound message types

All of these carry the common envelope (section 1) in addition to the
fields listed below.

### `combat_action`

Sent once per card-play decision (~4x/s during combat).

| Field | Type | Meaning |
|---|---|---|
| `player.hp` / `max_hp` / `block` / `energy` / `max_energy` | int | |
| `player.powers` | Power[] | |
| `player.cards_played_this_turn` | int[5] | Player card plays started this turn, bucketed by card type in the Rust engine's `CardType::index` order (`Attack, Skill, Power, Curse, Status`). Feeds mid-turn ShouldPlay gates (Ringing / Normality / Sloth) in the client's rebuilt search state. |
| `hand` | Card[] | **With legality** (`playable`/`unplayable_reason`/`valid_target_indices` populated). |
| `draw_pile` / `discard_pile` / `exhaust_pile` | Pile object `{type, count, cards: Card[]}` | Full contents, engine order. **Draw pile index 0 is the next card that will be drawn** (verified against `CardPile`/`CardPileCmd.Draw` in the v0.107.1 decomp). |
| `draw_pile_count` / `discard_pile_count` / `exhaust_pile_count` | int | Legacy count-only fields, kept alongside the full piles above. |
| `enemies` | Creature[] | |
| `orbs` | Orb[] | Defect only, empty otherwise. |
| `orb_capacity` | int | |
| `pets` | Creature[] | Necrobinder's Osty (and any other pet), empty otherwise. |
| `potions` | Potion[] | One entry per slot, including empty slots (see Potion object). |
| `available_actions` | string[] | `["PLAY","END_TURN"]` plus `"POTION"` iff any potion slot has `can_use=true`. |
| `round` | int | `CombatState.RoundNumber`. |
| `room_type` | string | The live `RunState.CurrentRoom.RoomType` (`Monster`\|`Elite`\|`Boss`\|`Event`\|...), same enum string the headless `RunContext` emits as `context.room_type`. The client maps `"Elite"` onto the engine's `is_elite` flag (an eval-net feature). Defaults to `"Combat"` when no room is known. |
| `player.stars` | int | **Regent only** (same condition as headless `RunSimulator.CombatPlayState`: `pcs.Stars >= 0` and character `REGENT`). The star economy for star-cost cards; the client hoists it to the headless dialect's top-level `stars`. |

### `map_select`

Sent once per map screen.

| Field | Type | Meaning |
|---|---|---|
| `nodes` | array | `{index, type, row, col}` per **immediately reachable** next node (unchanged addressing -- `choose`'s `index` still refers into this array). |
| `map` | object | **New.** Full act map graph: `{row_count, col_count, current:{col,row}|null, boss:{col,row}|null, visited:[{col,row}], nodes:[{col,row,type,children:[{col,row}]}]}` covering every point in the act (not just the next hop), with `children` as the edge list. Computed once per map screen, never per combat decision. |

### `card_select`

Deck upgrade/transform/enchant/hand-selection prompts (via `ICardSelector`).
This is also how **hand-select** modalities (Armaments upgrade-1,
Burning Pact exhaust-1, Gambler's Brew discard-N, etc) reach the wire: the
game's `CardSelectCmd.FromHand`/`FromHandForDiscard`/`FromHandForUpgrade`
all bypass the in-place `NPlayerHand` UI and call
`SimCardSelector.GetSelectedCards` directly whenever a selector is
installed (which it always is, for the run's lifetime), so there is no
separate "hand_select" message type.

| Field | Type | Meaning |
|---|---|---|
| `cards` | Card[] | Each card object plus `index` (position in this selection's option list). No legality fields. |
| `min_select` / `max_select` | int | |
| `min_count` / `max_count` | int | **New, phase 1b.** Aliases of `min_select`/`max_select`, additive. |
| `cancelable` | bool | **New, phase 1b.** `true` iff `min_select <= 0` -- see "cancel" in section 4. **This is a documented approximation, not the game's real cancelable flag**: the automation-facing `MegaCrit.Sts2.Core.TestSupport.ICardSelector.GetSelectedCards(options, minSelect, maxSelect)` signature does not pass the game's own `CardSelectorPrefs.Cancelable` at all (verified against `CardSelectCmd.cs` in the v0.107.1 decomp -- every `FromXxx` call site forwards only `minSelect`/`maxSelect`). A selection that is genuinely cancelable in-game with `min_select > 0` will report `cancelable:false` here; there is no game symbol reachable from this hook to fix that. |

If the option count is `<= min_select`, the mod auto-resolves without asking (unchanged from v1).

### `card_reward`

Post-combat card picks. Two producers (normal `ICardSelector` path and a
defensive `NCardRewardSelectionScreen` overlay fallback) now share the
same schema:

| Field | Type | Meaning |
|---|---|---|
| `cards` | Card[] | Each card object plus `index`. |
| `can_skip` | bool | Always `true`. |

### `card_bundle`

| Field | Type | Meaning |
|---|---|---|
| `bundles` | array | `{index, action:"pick_card_bundle", cards: Card[], enabled:true}` per bundle. |

### `crystal_sphere`

| Field | Type | Meaning |
|---|---|---|
| `options` | array | `{index, action:"divine_cell", x, y, enabled:true}` per hidden cell, plus a trailing `{index, action:"proceed", enabled:true}` if proceed is available. |
| `grid` | object | *(sts2-agent fork)* What the screen shows, as the Headless host's `crystal_sphere` decision: `{width, height, cleared:[[x,y],...], divinations_left, tool:"big"\|"small", items:[{kind, rarity, x, y, width, height, revealed}]}`. `cleared` includes the corners the game clears first; `items` only those with a cleared cell, by position (`kind`: `card_reward`, `curse`, `gold`, `potion`, `relic`; `rarity` a card reward's, once a cell of its top row is clear, else null). Absent if the minigame can't be read. |

If there are no hidden cells and no enabled proceed button, no message is sent (the handler waits for game animation instead).
Once no divination is left (sts2-agent fork) no message is sent either: the
handler waits for the minigame's rewards to come and go and clicks proceed
itself, as the Headless host's minigame ends at its last divination.

A `choose` of a cell may carry `tool: "big"|"small"` (sts2-agent fork): the
screen's tool button is pressed first when it differs; without `tool` the
selected one is kept. Another value, a `tool` on the proceed index, or one
where the minigame can't be read gets `error:"invalid_tool"` and a re-send.

Proceed availability is re-evaluated for every (re-)send. A `choose` of the
proceed index when proceed is not enabled at the time the reply arrives (or
was not offered in the message being answered) gets `error:"invalid_index"`
and a fresh re-send; the disabled button is never clicked.

### `rest_site`

| Field | Type | Meaning |
|---|---|---|
| `options` | array | `{index, id, action:"rest_option", label, description, enabled}` per option. Only options the game currently allows (`RestSiteOption.IsEnabled`) are offered, so `enabled` is always `true`; advisor mode offers the same set. |

### `shop`

| Field | Type | Meaning |
|---|---|---|
| `options` | array | Index 0 is always `{index:0, id:"leave_shop", action:"leave_shop", ...}`; remaining entries are `{index, id:<action>, action:<action>, label, description, enabled, price, item?}` where `<action>` is `buy_card`\|`buy_relic`\|`buy_potion`\|`remove_card`\|`buy_item`. `item` (new, optional) is the full Card/Relic/Potion object for that slot when resolvable. |
| `player_gold` | int | **New.** Also available via `run_state.gold`, duplicated here for convenience. |
| `purge_cost` | int | **New, optional.** Present iff a card-removal slot is stocked; its current price. |
| `shopfront` | array | *(sts2-agent fork)* Every merchant slot in inventory order (character cards, colorless cards, relics, potions, the card removal), sold or unaffordable included: `{slot, kind, label, price, stocked, affordable, item?, on_sale, reason, option_index}`. `kind` is the option action above; `item` the Card/Relic/Potion object while stocked; `on_sale` a card's sale; `reason` why the game would refuse the purchase now (`sold_out`, `removal_used`, `not_enough_gold`, `no_removable_cards`, `potion_forbidden`, `belt_full`; the Headless host's reasons) or null; `option_index` the slot's index in `options` (null when `reason` isn't). |

`options` lists exactly the slots whose `reason` is null. The shop is asked
even with nothing to buy (leave, and potions: `potion_verb`, section 4). After
a purchase the handler drains any screen the purchase left up (the card
removal's selection is a `card_select`), reopens the inventory if it closed,
and asks again: the visit ends only on `leave_shop` (or `skip`).

### `event`

| Field | Type | Meaning |
|---|---|---|
| `options` | array | `{index, id:"event_choice", action:"event_choice", label, description, enabled, event_id, text_key, relic_id}` per option. `text_key` is the option's loc-key prefix (`EventOption.TextKey`, e.g. `"NEOW.pages.INITIAL.options.WINGED_BOOTS"`; its tail is the option's enum id). `relic_id` is `EventOption.Relic?.Id.Entry` for relic-granting options, `""` otherwise. |

A `choose` whose `index` is out of range, or names an option that has become
locked by the time the reply arrives, gets `error:"invalid_index"` and a
fresh re-send (like every other `choose` handler).

### `treasure`

| Field | Type | Meaning |
|---|---|---|
| `options` | array | `{index, id, action:"collect", label, description, enabled:true, relic}` per relic choice -- `relic` (new) is the full Relic object. |

### `boss_relic`

Same shape as `treasure`, `action:"pick_relic"`.

### `reward_screen`

| Field | Type | Meaning |
|---|---|---|
| `options` | array | `{index, id, action:"pick_reward", label, description, enabled, reward?}` per reward, plus an optional trailing `{index, id:"proceed", action:"proceed", enabled:true}`. `reward` (new, optional): a Card[] for `CardReward` (one entry per offered card), a Potion object for `PotionReward`, a Relic object for `RelicReward`, or `{amount}` for `GoldReward`. |

Proceed availability is re-evaluated for every (re-)send. A `choose` of the
proceed index when proceed is not enabled at the time the reply arrives (or
was not offered in the message being answered) gets `error:"invalid_index"`
and a fresh re-send; the disabled button is never clicked.

A potion reward only a full belt keeps from the player is listed with
`enabled:false, reason:"belt_full"` while a potion can be drunk or discarded
(sts2-agent fork: the room is made with the `potion` verb); with no such
potion it is left out as before, and a screen with nothing else to take
proceeds without asking. One the run refuses (Sozu: `ShouldProcurePotion`)
is never listed, since no discard would make it claimable.

`RewardLabel` unchanged: `"gold:{amount}"` for gold, else the reward's C# type name (`CardReward`/`PotionReward`/`RelicReward`).

### `game_over`

Fire-and-forget, no `request_id`.

| Field | Type | Meaning |
|---|---|---|
| `result` | string | `"victory"` \| `"defeat"`. |
| `message` | string | Literal `"Game over"`. |

### `run_complete`

Fire-and-forget, no `request_id`.

| Field | Type | Meaning |
|---|---|---|
| `result` | string | `"victory"` \| `"defeat"` \| `"terminated"`. |

### `run_config` -- new, phase 1b

Only ever sent when `SpireMentor.conf`'s `client_run_config` is `true`, once
per run (at character-select time, in place of the old auto-selected
character/ascension/seed flow), and the mod **blocks indefinitely** for the
matching `start_run` action (section 4) -- see README "Client-driven run
start".

| Field | Type | Meaning |
|---|---|---|
| `characters` | array | `{id, name}` per available (unlocked) character button on the live character-select screen -- `id` is the exact `CharacterModel.Id.Entry` to echo back in `start_run` (e.g. `"IRONCLAD"`, `"SILENT"`, `"DEFECT"`, `"NECROBINDER"`, `"REGENT"`). |
| `max_ascension` | int | `10` (v0.107.1 decomp-verified: `AscensionManager.maxAscensionAllowed`). |
| `defaults` | object | `{character, ascension, seed}` straight from `SpireMentor.conf` -- purely informational, the client is not required to use them. |

Note: `run_state`/`seed`/`act`/`floor` in the common envelope all degrade to
their no-active-run defaults on this message (`""`/`1`/`0`/empty
collections), since no run exists yet at this point.

### `pong`

Literal `{"type":"pong"}`. No envelope, no `request_id` -- purely a transport-level ping reply.

---

## 3a. `error` -- structured error notifications, new phase 1b

Fire-and-forget, no `request_id` (same as `game_over`/`run_complete`) --
sent whenever an inbound action is rejected for any reason. **The rejecting
handler never silently drops the action, never crashes, and never
substitutes a random/default choice** (never-guess policy). After sending
`error`, the handler's existing retry loop runs again, which re-serializes
and re-sends the current decision message (a fresh `request_id`/`state_id`,
via a new `SimAgentGate.RequestActionAsync` call) -- so an `error` is always
immediately followed by a fresh copy of whatever decision message the
rejected action was replying to.

| Field | Type | Meaning |
|---|---|---|
| `error` | string | Machine-readable error code, see table below. |
| `detail` | string | Human-readable detail (exact reason, offending value(s)). |

`error` messages carry the common envelope of section 1 (`state_id`,
`seed`, `act`, `floor`, `run_state`, ...) whenever a run is active —
`SimProtocolHelpers.SendError` applies `SimStateExtractor.ApplyEnvelope`
with whatever `RunState` the rejecting handler had. Outside a run (or if
the handler had no `RunState`), the envelope fields are simply absent.
Clients must key off field *presence*, not message type.

Error codes in use:

| Code | Meaning |
|---|---|
| `stale_state` | Inbound `state_id` didn't match the state_id of the decision currently being answered. |
| `malformed_action` | Missing/wrong-typed required field (e.g. `play` without `card_index`, `potion` without `slot`, `choose` without `index`). |
| `unknown_action` | The `action` string isn't recognized for this decision point. |
| `invalid_index` | `index`/`card_index`/`indices` value(s) out of range for the current options, or naming an option that is not enabled when the reply arrives (event choice, reward-screen / Crystal Sphere proceed). |
| `invalid_count` | Multi-select `indices`/`indexes` count outside `[min_select, max_select]`. |
| `not_cancelable` | `{"action":"cancel"}` sent for a `card_select` prompt where `cancelable:false`. |
| `unplayable_card` | `play`'s `card_index` resolves to a card that fails `CardModel.CanPlay()`; `detail` includes the raw `UnplayableReason`. |
| `target_required` | A card/potion needs an `AnyEnemy` target and no `target_index` was given (and auto-pick found no hittable enemy). |
| `invalid_target` | An explicit `target_index` was given but doesn't resolve to a legal/hittable target. |
| `empty_potion_slot` | `potion`'s `slot` has no potion. |
| `potion_busy` | `potion` with `discard:true` targeted a potion that is already queued for use (`PotionModel.IsQueued`). |
| `invalid_character` | `start_run`'s `character` isn't one of `run_config`'s `characters`. |
| `invalid_ascension` | `start_run`'s `ascension` outside `[0, max_ascension]`. |
| `internal_error` | Unexpected exception while handling the action; `detail` is the exception message. |

---

## 4. Inbound actions

### `state_id` stale-state guard (phase 1b, applies to every verb below)

Every inbound action **may** carry a top-level `"state_id"` field (an
integer). If present and it doesn't equal the `state_id` on the decision
message currently being answered, the action is **not executed** -- the
handler replies `{"type":"error","error":"stale_state",...}` and re-sends a
fresh copy of the current decision message. If `state_id` is absent, the
action executes exactly as it did before phase 1b (fully backward
compatible). This lets a client assert "the choice I'm sending was computed
against exactly this state" and get a clear rejection instead of the game
silently acting on stale information -- port of HermesBridge's
`expectedRevision`/`expectedScreen`/etc guard idiom (see
phase1-target.md section 2.1/2.2), generalized to one field.

### Combat verbs (`SimCombatHandler.ExecuteAgentAction`, case-insensitive)

| Verb | Payload | Behavior |
|---|---|---|
| `play` | `{action:"play", card_index:int, target_index:int?, state_id:int?}` | `card_index` **always** addresses `hand[card_index]` (0-based). **Phase 1a change**: the old `card_index >= 10` -> "treat as potion slot `card_index - 10`" reinterpretation has been removed -- it silently made hand cards at index >= 10 unreachable for hands larger than 10 cards. Use the `potion` verb to use a potion. `target_index` (optional, default -1 = auto-pick first hittable enemy) is only consulted for `AnyEnemy`-targeted cards. |
| `end_turn` | `{action:"end_turn", state_id:int?}` | Ends the turn. |
| `potion` | `{action:"potion", slot:int, target_index:int?, discard:bool?, state_id:int?}` | **Phase 1b: payload extended.** Uses (or, if `discard:true`, discards via `PotionCmd.Discard`) the potion at `slot` (0-based index into `potions[]`/`run_state.potions[]`). For a use (non-discard) with `TargetType.AnyEnemy`: same auto-pick-first-hittable-if-omitted resolution as `play`'s `target_index`; an explicit-but-invalid index sends `error:"invalid_target"`, an omitted index with no hittable enemy sends `error:"target_required"`. `Self`/`AnyPlayer`/`AnyAlly` potions target the player automatically, no `target_index` needed. `discard:true` ignores `target_index` entirely; discarding an empty slot sends `error:"empty_potion_slot"`, discarding a potion already mid-use (`PotionModel.IsQueued`) sends `error:"potion_busy"` (the engine's own `PotionCmd.Discard` has no such guard -- verified against the v0.107.1 decomp -- so the mod adds it, mirroring the game's own UI, which disables the Discard button in that state). |

**Phase 1b**: every rejection above (bad `card_index`/`slot`, unplayable
card, missing/invalid target, unknown action, malformed JSON, stale
`state_id`) now sends a structured `error` message (section 3a) before the
existing loop re-serializes and re-asks -- never a silent no-op.

### `potion` outside combat (sts2-agent fork)

`map_select`, `event`, `rest_site`, `treasure`, `shop`, `reward_screen` and
`card_reward` (its screen; the cards stay on offer) sent outside a fight carry `potion_verb: true`, and each held slot of their
`run_state.potions` says `use_now` and `discard_now` (section 2). There the
bridge also takes `{action:"potion", slot:int, discard:bool?, state_id:int?}`:
it drinks the potion (a `UsePotionAction`, as the belt's popup enqueues for a
potion that needs no aiming; Foul Potion thrown at the merchant, the shopfront
closed first as the game needs and opened again after) or discards it
(`discard:true`, `DiscardPotionGameAction`), waits until the game's action has
finished (`GameAction.State`; screens it opens are drained, the decision's own
left alone), and sends the decision again (a fresh `state_id`). An action the
game cancels (nothing changed; caught as it is cancelled, since `State` ends
`Finished` either way) is answered with the refusal's error; one that throws
sends `error:"potion_faulted"` and fails the run (`run_complete`
`"terminated"`: what it changed is unknown); one that never finishes ends the
run on the watchdog's stall limit (`SimActionOutcome.cs`; its lifecycle test:
`tests/potion-lifecycle/run.sh`). Refused with
`error:"potion_not_usable"` or `"potion_not_discardable"` where `use_now` /
`discard_now` is false,
`"empty_potion_slot"` for an empty slot, `"malformed_action"` without a
numeric `slot`, `"stale_state"` as any verb. Any other decision (a card
selection, a bundle, the Crystal Sphere, a relic screen; a card reward in test
mode, through `SimCardSelector`) has
no `potion_verb` and refuses `potion` as an unknown action.

### Transport-level

| Verb | Payload | Behavior |
|---|---|---|
| `ping` (action, case-insensitive) or `PING` (legacy `type`) | | Immediate `{"type":"pong"}`, bypasses request/reply correlation. |

### Generic choice verbs (`choose` / `skip`)

Used by every non-combat handler (rest site, treasure, boss relic, shop,
event, card bundle, crystal sphere, reward screen):

- `{"action":"choose","index":N,"state_id":int?}` -- positional index into
  that message's `options`/`bundles`/`nodes` array.
- `{"action":"skip"}` -- only meaningful for `shop` (treated as "leave
  shop", index 0); anywhere else it's an unrecognized action (`error:
  "unknown_action"`, re-asks).
- The `action` string embedded in each *outbound* option (`rest_option`,
  `buy_card`, `event_choice`, `collect`, `pick_relic`, `pick_reward`,
  `pick_card_bundle`, `divine_cell`, `proceed`, `leave_shop`) is purely
  descriptive -- the client must always reply with the literal verb
  `{"action":"choose","index":N}` regardless of which one is shown.
- **Phase 1b**: an out-of-range `index`, a malformed/unparsable response, or
  an unrecognized `action` now sends a structured `error` message (section
  3a) before the handler's existing loop re-asks -- previously this was
  logged server-side only, with no wire-visible signal.

`map_select` is the one outlier: it reads `index` directly off the
response with no `action` check (unchanged in phase 1b; still gets the
`state_id` guard and structured errors for a missing/malformed/out-of-range
`index`).

### `annotations` -- optional, on any inbound action (new, phase 4, overlay)

**Any inbound action** (every verb below, not just combat's `play`) may
carry an additional top-level `"annotations"` object. It has **zero effect
on how the action itself is executed** -- it is purely a hint for
Spire Mentor's optional in-game HUD (`SimHud.cs`), gated by the
`overlay` config flag (default `true`, see README "Config reference").
In addition to the two legacy fields below, `annotations` may carry any of
the rich `advice`-object fields from section 5 (`p_win`, `options`, `pv`,
`low_confidence`) -- the HUD renders whichever are present.
Malformed, absent, or unparsable `annotations` are silently ignored (logged
server-side only, never surfaced to the client, never rejected via `error`);
a bad annotations payload can **never** cause an action to fail or change
its outcome.

```json
{"action":"play", "card_index":2, "target_index":0,
 "annotations": {"note": "MCTS p_win 0.93, 300 sims",
                  "cards": [{"index":0,"score":0.41,"label":"41%"},
                            {"index":2,"score":0.93,"label":"93%"}]}}
```

| Field | Type | Meaning |
|---|---|---|
| `note` | string | Free-text status line shown in the overlay's persistent corner panel, alongside the mod name and a short summary of the action it rode in on (e.g. `play card_index=2 target_index=0`). Updated in place on every annotated action -- no history, no fade timer. |
| `cards` | array | `{index, score, label}` per hand card the agent wants to annotate with a small on-screen label. `index` uses the **same hand indexing as `play`'s `card_index`** (0-based, Nth card of `PileType.Hand`) -- not a UI holder position. `score` (number, clamped to `[0,1]`) drives a red (0) -> green (1) label color; `label` (string) is the text shown, falling back to `score` formatted as a percentage if omitted. Only rendered while combat is in progress; silently ignored otherwise (e.g. annotations on a `choose` reply to a `rest_site` prompt only affect the status panel, never card labels). |

Annotations are picked off on the same call that parses the action
(`SimProtocolHelpers.TryParse` -- the single choke point every inbound
action's JSON passes through, either directly or via `ReadChoiceIndex`).
No new socket concurrency, no extra round trip. (Since phase 3/HUD v2 the
actual rendering is marshalled through `CallDeferred`, so the thread the
parse happens on no longer matters.)

### Card-selection verbs

`card_select`:
- `{"action":"skip"}` or `{"action":"cancel"}` (**`cancel` new in phase
  1b**, an alias of `skip` -- see the `cancelable` field, section 3) --
  valid only if `min_select<=0`/`cancelable`; otherwise `error:
  "not_cancelable"`.
- `{"action":"choose","index":N}` (single-select).
- `{"action":"choose","indices":[...]}` (**`indices` new in phase 1b**,
  preferred spelling) or `{"action":"choose","indexes":[...]}` (pre-existing
  spelling, still accepted) -- multi-select, validated against
  `min_select`/`max_select` (`error:"invalid_count"` if out of range,
  `error:"invalid_index"` if any element is out of range).

`card_reward`: `{"action":"skip"}` or `{"action":"choose","index":N}`
(`N >= cards.length` is treated as an implicit skip). Both `card_reward`
producers (selector and overlay fallback) apply the `state_id` guard and
reply to an unusable action with `malformed_action` (no integer `index`),
`unknown_action`, `invalid_index` (negative `index`) or `stale_state`, then
re-send.

### Run negotiation verb -- new, phase 1b

`start_run`, the only valid reply to `run_config` (section 3), only sent/
accepted when `client_run_config` is enabled and no run is active yet:

`{"action":"start_run", "character":"<id>", "ascension":N, "seed":<int|null>}`

| Field | Type | Meaning |
|---|---|---|
| `character` | string | Must match one of `run_config.characters[].id` (case-insensitive exact match preferred, falls back to substring match); otherwise `error:"invalid_character"`. |
| `ascension` | int | `0`-`max_ascension` (10); otherwise `error:"invalid_ascension"`. |
| `seed` | int \| null | If an integer, applied via `NGame.Instance.DebugSeedOverride` (formatted as an invariant-culture decimal string) before the character-select confirm click -- the same verified v0.107.1 mechanism the mod already used for `SpireMentor.conf`'s `seed`. If `null`/omitted, a fresh random seed is generated exactly as the old config-driven flow did. The resolved seed is always echoed back in the envelope's `seed` field once the run actually starts (`run_state`/every subsequent message). |

Rejected `start_run` actions get a structured `error` and a fresh
`run_config` re-send; the mod blocks indefinitely for a valid one
(never-guess policy -- it will not fall back to `SpireMentor.conf`'s
character/ascension/seed once `client_run_config` is on).

---

## 5. Advisor mode (protocol v3) -- advice push, additive only

New in the advisor-mode work (2026-08). **Every v2 verb above is
untouched**; a v2-only client (all `realgame_*` drivers) keeps working
against a v3 mod and vice versa. The advisor lane only activates while the
mod's runtime mode is `Advisor` (a human is playing; see `SimMode.cs`) --
in `Autopilot` mode none of these messages are ever sent.

### Transport: the advice lane

Autopilot correlation uses a single pending slot (one blocking
request/response at a time). Advice requests are **non-blocking and may
overlap**, so they live in a separate pending map keyed by their own
`request_id` namespace: advice request ids look like `"a<N>"` (autopilot
ids stay bare integers). An inbound `{"action":"advice"}` reply is routed
by its `action` field to the advice map **before** the pending-action
delivery runs, so advisor traffic can never consume -- or be consumed by --
an in-flight autopilot request (`BridgeServer.SendAdviceRequest` /
`DeliverAdvice`).

### `advice_request` -- outbound, new

Sent when the human reaches a decision point (a screen opens, a combat
turn starts, the hand changes). Same envelope as every outbound message
(`type`, `seed`, `act`, `floor`, `state_id`, `request_id`, `run_state`,
`map`), plus the full payload the corresponding v2 decision message would
carry, plus:

| Field | Type | Meaning |
|---|---|---|
| `advice_for` | string | The v2 message type this request wraps (`combat_action`, `card_reward`, `map_select`, ...). The payload fields are those of that message type. |
| `combat_log` | array | The game's own combat-log entries recorded SINCE THE PREVIOUS request (`SimCombatLog.Drain()`): one row per `CombatHistoryEntry`, in the same shape the Rust engine emits (`sts2_core::event_log`), so the workbench renders a live game through the identical formatter it uses for a simulated one. Display only — the advisor never reads it. Empty outside combat. |

Each `combat_log` row carries `kind` (`damage` / `block` / `power` /
`monster_move` / `card_played` / …, or `other` for an entry kind the workbench
has no renderer for), `round`, `side`, the game's own `text`
(`CombatHistoryEntry.Description`), a `source` naming the card or monster move
that caused it, and the kind's own fields (`dealer` / `receiver` / `unblocked` /
`blocked` / `amount` / …). Creatures are `{kind, id, uid}` with the id C# prints
(`IRONCLAD`, `Nibbit`) — never a display name: localization belongs to the
workbench and it needs a stable key.

Rows are DRAINED, not resent: each request carries only what happened since the
last one, and the drain resets itself when the history shrinks (the only way
that happens is a new combat). A fight's final rows — the killing blow, the
end-of-combat powers — land on the first non-combat request that follows it,
which is why the screen-advice path drains as well.

The mod does **not** block on an `advice_request`. If the human acts
before the reply arrives, the reply is dropped on `state_id` mismatch
(same stale-state guard as v2, applied HUD-side) or cancelled outright.

Implemented `advice_for` values:

| Value | Payload | Answered by |
|---|---|---|
| `combat_action` | the v2 combat board | MCTS honest search (`compute_combat_advice`) |
| `card_reward` | `cards[]` + `can_skip`, scraped off the live `NCardRewardSelectionScreen` | the PPO run-policy champion (`compute_screen_advice`) |
| `card_select` | `cards[]`, `min/max_select`, `cancelable`, `prompt_key`, `source`, `screen`, embedded `combat` | same, via `realgame_ppo._build_card_select`'s prompt-keyed surface |
| `map_select` | `nodes[]` (the clickable children) + the full act `map` graph | same; the reply also carries `path[]` (see below) |
| `shop` | `options[]` scraped off `NMerchantRoom` | same |
| `event` | `options[]` scraped off the live `NEventOptionButton`s (Neow included) | same |
| `rest_site` | `options[]` scraped off `NRestSiteButton`s | same |
| `boss_relic` | `options[]` from `NChooseARelicSelection`'s relic holders | same |
| `treasure` | `options[]` from the chest's relic holders, `surface: "treasure"` | the **boss-relic** head — the same question, cheaper; the note says so |
| `card_bundle` | `options[]`, each with its `cards[]`, from `NChooseABundleSelectionScreen` | the **card-pick** head, applied card-by-card; a composition, not a trained surface, and the note says so |

`card_reward` also carries `surface: "choose_a_card"` when it comes from the
in-combat generator (`NChooseACardSelectionScreen` — Skill Potion and friends).
Same payload, different meaning: that card is generated INTO the fight and then
gone, so its deck value is the wrong question and the note flags it.

Answered **mod-side**, no round trip, because they are not judgement calls:
the loot screen (`NRewardsScreen`: claim everything, then proceed), the chest's
open/proceed steps, the rest site's proceed, and the crystal sphere (every
remaining cell is hidden — there is nothing to rank). The game-over screen is
described but never clicked: it only exists so autoplay can stop cleanly.

Every one of these goes through `realgame_ppo._PPO_NONCOMBAT_BUILDERS` — the
driver's own builder table — so a surface the autopilot can decide is a surface
the advisor can explain, with no second dispatch to keep in sync.

Screen payloads are built from the **live screen nodes** (`SimScreenAdvice.cs`),
not from an `ICardSelector` callback: in advisor mode no selector is installed,
so the human sees the real screen and there is no callback to intercept.
`prompt_key` still comes from `CardSelectContextPatch`, which is peeked rather
than consumed there.

The post-combat loot screen (`NRewardsScreen`) is answered **mod-side** without
a round trip — "claim everything, then proceed" is not a judgement call — so it
never appears as an `advice_request`.

### `advice` -- inbound, new; rendered, never executed

```json
{"action":"advice", "request_id":"a77", "state_id":412,
 "advice":{
   "note":"MCTS 3000 sims · beliefdraw",
   "p_win":0.73,
   "low_confidence":false,
   "best":{"stable_idx":2,"target_index":0,"label":"Bash → Cultist"},
   "options":[{"stable_idx":2,"target_index":0,"card_index":2,
               "label":"Bash → Cultist","p_win":0.73,"q":0.31,
               "visits":1420,"visit_share":0.47,
               "wins":900,"losses":420,"ties":100,
               "is_best":true,"low_confidence":false}],
   "pv":["Bash → Cultist","Strike → Cultist","end turn"]}}
```

| Field | Type | Meaning |
|---|---|---|
| `request_id` | string | Must echo the `advice_request`'s id; unknown/stale ids are logged and discarded. |
| `state_id` | int | Echo of the request's `state_id`; mismatch with the current state drops the advice instead of rendering it. |
| `advice.note` | string | Status line (search config, sim count). |
| `advice.p_win` | number | Position win probability. Shown as text only: it saturates at ~1.0 for every legal action in a fight the search expects to win, so the HUD's eval bar is driven by the best option's `q` instead (the workbench's Reasoning panel made the same call). |
| `advice.options[]` | array | Ranked candidate actions. `label` is the display text; `card_index` (optional) anchors a per-card label using `play`'s hand indexing; `p_win`/`q`/`visits`/`visit_share`/`wins`/`losses`/`ties` come straight from the MCTS root outcome; `is_best` marks the recommendation (gold highlight); `low_confidence` marks flagged rows. For screen advice there is no per-action Q, so the net's probability rides on `visit_share` (the field the HUD draws as a bar) and `index` names the option to click. |
| `advice.best.action` | string | `play` / `end_turn` / `potion` for combat; `choose` (with `index`) or `skip` for screen decisions, which the mod executes by clicking the screen. |
| `advice.best` | object | Convenience copy of the recommended option (`stable_idx`/`target_index` are what phase 7's "AI takes this decision" executes). |
| `advice.best.label` | string | Also the takeover's **cross-check**: before clicking option `index`, the mod compares this against the option list it can see right now, and re-anchors (or refuses) if the list moved under it. Card screens spell it from `CardModel.Title` on both sides. |
| `advice.options[].col/row` | int | `map_select` only: the option's map-graph coordinate, so the HUD can pin its score to the node on the map instead of printing a coordinate in a panel. |
| `advice.path[]` | array | `map_select` only: the **projected route** through the act — `{col, row, type, hop, hp}`, `hop` 0 being the node being chosen now and `hp` the HP the projection expects to *arrive* at that node with (before its own room resolves). Produced by re-querying the same policy hop by hop with `map_coord` walked along the graph and HP walked forward through the run-AI's expected-cost tables, so the route runs the whole act rather than stopping a few floors out. Deck, gold, relics and potions stay frozen and the HP deltas are flat per-act expectations, so it is a projection, not a plan, and the HUD says so. `hp` is absent when the engine could not supply its cost tables (older wheel) — the route then draws with HP frozen, as it did before. Absent entirely when the net cannot see a route (single-child chains still count). |
| `advice.pv` | array of string | Principal variation, rendered as the "line:" row. |

An `advice` message is **never executed** -- it only feeds the HUD and the
action log. Phase 7's takeover hotkey executes the recommendation through
the normal game-side paths, not through this message.

### `human_action` -- outbound, new

Sent after the human commits a decision (card played, potion used, screen
choice made), for the advisor's log/review pipeline:

```json
{"type":"human_action", "state_id":412, "decision_type":"combat_action",
 "action":"play", "detail":"Bash → Cultist", "advice_request_id":"a77"}
```

`advice_request_id` links the action to the ranking that was on screen at
the time (or `null` if none), which is what makes phase-9 move review free
-- no second search needed. Fire-and-forget: no reply expected.

A **card play** additionally carries the MACHINE form of the action:

| Field | Type | Meaning |
|---|---|---|
| `card` | object \| null | `{"id": "HEADBUTT", "upgraded": false}` -- the card played. `null` for anything that is not a card play. |
| `potion` | object \| null | `{"id": "ATTACK_POTION"}` -- the potion drunk. `null` for anything that is not a drink. No `upgraded` twin (a potion has none) and no slot: two belt slots holding the same potion are interchangeable, so a client locates it by identity in its OWN belt, exactly as it locates a card in its own hand. The two sides then never have to agree on belt order. |
| `target_index` | int \| null | Index into the full `Enemies` list -- the same frame `SimCombatHandler.ResolveTarget` reads, so a corpse keeps its slot. `null` when the card or potion took no target. For a drink this is advisory: the engine resolves a null potion target to the first alive enemy exactly as C# `RunSimulator.DoUsePotion` does, and no potion that opens a card-select is single-enemy targeted. |

`detail` alone is a display string ("Bash -> Cultist"), which is enough to
LOG an action and not enough to REPLAY one. A client that wants to keep an
engine mirror in step with the fight needs the replay: without it the advisor
could only re-seed from each new board, so it predicted nothing and every
ordinary change read as a divergence -- 2232 desyncs over 540 decisions on
live run `001-ironclad-a8-0824-1939`, 42x the autopilot's rate. With these
fields plus the client-side turn catch-up it is 171 over 714 (0.24), against
the autopilot's 0.10.

Additive: a client that ignores the new fields sees exactly the old message.
A client that uses them must treat **neither `card` nor `potion` being
present** as "not replayable" and drop its mirror rather than assume the board
did not move -- an older mod build sends none of these fields.

`potion` was added because its absence was expensive. A drink used to arrive
with `card` null and nothing else, which a client could only read as an
unfollowable board change: every fight in which anything was drunk lost its
mirror at that moment, and with it every searched decision for the rest of the
fight. The engine models the drink as a first-class action, so the id is all a
client needs to follow it.

**A card that opens a selection screen reports LATE.** The history entry is
`CardPlayFinishedEntry`, and such a play finishes only once the selection is
answered -- so the screen's own `advice_request` arrives BEFORE the
`human_action` for the card that opened it. A client cannot use this message
to park its mirror on that choice; that is what `card_select`'s
`source_model` is for.

### `card_select.source_model` coverage

`source_model` names the model that opened the selection, so a client can
replay the opening play and park its own engine on the choice -- with the rest
of the card (Burning Pact's "draw 2", Brand's Strength) still suspended behind
it. It is filled from two places, in this order:

1. The declared `AbstractModel source` argument, where the entry point has one
   (`FromHand`, `FromHandForDiscard`, `FromHandForUpgrade`).
2. Otherwise `PlayerChoiceContext.LastInvolvedModel` -- the game's own model
   stack, pushed by `CardModel.OnPlayWrapper` for a card play and by
   `HookPlayerChoiceContext` for a hook-driven one.

Rung 2 is what covers the **combat pile** screens (`FromCombatPile`:
Headbutt, Dredge, Seance, Secret Technique, Hologram, Charge, Cleanse, ...) and
the **generative** ones (`FromChooseACardScreen`: Toolbox, Discovery, Quasar,
Splash, the four card potions). Neither declares a `source`, so before this
they arrived unattributed and no client could search them.

The two rungs disagree on purpose and rung 1 wins where both apply: the model
stack tracks the INNERMOST model -- a Sly'd Prepared autoplayed by Survivor
pushes Prepared -- while `source` is the card that owns the selection.

Still expect `null`: a screen opened by something with no model stack, and any
older mod build. `null` means "not attributable", not "no source" -- fall back,
do not guess.

### `card_select` source detail: `source_kind`, `source_target_index`, `source_upgraded`

`source_model` is a NAME. These three say what that name IS and which play it
was, so a client never has to reconstruct either from strings or from the
board.

* **`source_kind`** -- `"card"`, `"potion"`, `"relic"`, `"power"`,
  `"affliction"`, `"enchantment"`, or `"model"` for an `AbstractModel` of no
  recognized subtype. Taken from the model's RUNTIME TYPE, which is the same
  taxonomy `HookPlayerChoiceContext` switches on to decide who owns a choice.

  Without it the only test available to a client is absence: look the id up in
  the hand, then in the other piles, and call it "not a card" when neither has
  it. That misfires exactly where it matters -- a real card that has fallen out
  of the client's copy of the fight reports as "not a card", hiding a mirror
  fault behind a category. `TouchOfInsanity` (a POTION, opening a `FromHand`
  select with `source: this`) and a genuinely lost card were indistinguishable.

  `null` means "unknown", never "not a card".

  A `"potion"` source is REPLAYABLE, not a dead end: the engine models the
  drink as a first-class action, and a potion that opens a card-select parks a
  real choice behind it. What the kind selects is which replay a client
  performs, not whether one is possible at all. The kinds that genuinely
  cannot be replayed are the hook-fired ones -- `"relic"`, `"power"`,
  `"affliction"`, `"enchantment"` -- because no player action corresponds to
  them.

* **`source_target_index`** -- the source card's target as an index into the
  FULL `Enemies` list (a corpse keeps its slot, so the index is stable across a
  kill within one board), or `null` for an untargeted card. Read from
  `CardModel.CurrentTarget` while the play is still in flight.

  This is the one fact a client cannot obtain any other way at screen time.
  The play is reported over `human_action` as a `CardPlayFinishedEntry`, and a
  card that opens a selection does not FINISH until the selection is answered
  -- so that message, target and all, arrives AFTER the advice request it would
  have answered. Lacking it, a client must replay the card at every legal
  target and match the resulting board against the world's, which fails
  whenever its damage prediction is off by any amount.

* **`source_upgraded`** -- whether the copy played was upgraded. The other half
  of "which play was it": two copies in hand are interchangeable, but Burning
  Pact and Burning Pact+ draw different amounts and neither shows in a board
  comparison.

All three are additive and all three may be `null` (older mod build, or a
source with no such property). A client that has them should still VERIFY the
named play reproduces the world's board -- but the meaning of a mismatch
changes: unnamed it means "I cannot tell which play it was" (say nothing);
named it means "my copy of the fight is wrong" (drop it and re-seed).

---

## 6. Known remaining gaps

- No structured `PurchaseStatus`/`UnplayableReason`-aware retry protocol on
  the wire -- `unplayable_reason` is a raw flags-enum string, not a fixed
  vocabulary the client can safely pattern-match beyond string equality.
- `move_history` is capped at 5 entries and is per-combat only (cleared
  each combat, matching `CombatManager.Instance.History`'s own lifetime).
- `CardRewardAlternative` selection (`GetSelectedCardReward`'s
  `alternatives` parameter) is still never surfaced or selectable from the
  wire -- `card_reward`'s `CardRewardSelection.alternative` is always
  `null`. Out of scope for phase 1b (no `card_reward` payload changes were
  requested).
- `card_select`'s `cancelable` field is a documented approximation
  (`min_select <= 0`), not the game's real `CardSelectorPrefs.Cancelable`
  flag -- see the field's own doc note in section 3. There is no reachable
  game symbol to fix this without deeper reflection into the UI-facing
  (non-automation) card-selection call path.
- Deck-select/upgrade/transform/enchant screens registered as a defensive
  `ICardSelector`-bypass fallback (see README "runtime risks") are
  unaffected by phase 1b and still use the game's own random-choice logic
  if that fallback path is ever actually reached (believed unreachable).

## 7. Rewind lane (protocol v4) -- snapshots + in-process restore, additive only

The mod snapshots the `SerializableRun` at the game's own save points
(room entry + post-combat-with-rewards -- see `SimSnapshots.cs`) and can
restore one in-process (`SimRestore.cs`, the FileDropHandler sequence).
Like the advice lane this is additive: a v2/v3 client that never sends the
verbs below sees only the new broadcast messages, which it may ignore.

Transport: **fire-and-forget commands with broadcast replies.** The verbs
are routed in `BridgeServer.ProcessIncomingMessage` BEFORE the pending
autopilot action slot (like `ping`/`advice`), carry no `request_id`
correlation, and are answered by plain outbound messages. The advisor's
control plane (`realgame_control.py`, localhost HTTP for the workbench)
serializes its own calls, so one-at-a-time is enforced client-side.

### Inbound verbs (client -> mod)

| Verb | Fields | Effect |
|---|---|---|
| `list_snapshots` | -- | Answered with a `snapshots` message. |
| `restore_snapshot` | `snapshot_id` | Rewinds the live run to that snapshot in-process. Answered with `snapshot_restored` on success, `error` on failure. |

### Outbound messages (mod -> client)

| Type | Fields | When |
|---|---|---|
| `snapshot_saved` | `snapshot_id, seq, floor, act, post_combat, run_start_time, ts` | Every capture (i.e. every game save). `post_combat=false` is a room-entry save (standing on the node, room not rolled); `true` is the post-combat save with the reward set pending. |
| `snapshots` | `run_start_time, snapshots[{id, seq, ts, floor, act, post_combat, num_reloads}]` | Reply to `list_snapshots`. |
| `snapshot_restored` | `snapshot_id, floor, state_id` | After ANY successful restore -- including ones triggered by the in-game toolbar, so the client always learns of rewinds. `state_id` is fresh: any advice or action referencing an older id is stale. |
| `error` | `error` in `snapshot_not_found` \| `restore_busy` \| `no_active_run` \| `snapshot_corrupt` \| `restore_failed`, plus `detail`, `snapshot_id` | Failed restore. |

### Semantics and caveats

- Restore is **bit-exact** for the run state (RNG counters fast-forward on
  fresh streams), but a different line after the rewind consumes different
  draws, so downstream offers legitimately diverge from the original line.
- `SetUpSavedSingleplayer` bumps `NumReloads` and re-saves
  `current_run.save` with the restored state -- deliberate ("the run
  continues from here"), via the game's own cloud-safe save path.
- Snapshots persist under `%LOCALAPPDATA%\SpireMentor\snapshots\<start_time>\`
  (never cloud-synced, immune to `CleanupStaleCurrentRunSaves`), so a
  same-run rewind still works after a game restart.
- Mid-combat boards cannot be restored (the game never saves combat
  state); a mid-combat restore lands at the room's entry save.
- **Turn undo** (protocol v4.1): the mod replays the game's own
  CombatReplay tape (recorded for every room) on top of the room-entry
  save, so a mid-combat rewind CAN land at the start of the current player
  turn. Inbound verb: | `undo_turn` | `turn?: int, tape_id?: str` | Undo
  to the start of the current player turn (previous turn when nothing was
  played yet). `turn` >= 1 targets that turn's start instead. `tape_id`
  (the id of a post-combat snapshot whose `has_tape` is true) replays a
  PERSISTED past combat's tape: its room-entry save is reloaded first, so
  this lands mid-fight in a room the run already left -- the workbench
  tree's mid-combat nodes chain exactly this. | .
  Advisor-lane combat `advice_request`s carry `turn` (the player's combat
  turn number) so the tree can map decisions to undo targets; post-combat
  `snapshot_saved` / `snapshots` entries carry `has_tape`.
  Success broadcast: `combat_undone` with `floor, turn, tape_id,
  replayed_events, state_id` (fresh id -- older advice is stale). Errors:
  `undo_busy`, `undo_not_in_combat`, `undo_not_player_turn`,
  `undo_no_tape`, `undo_bad_turn`, `undo_failed`. Recorded card choices replay through the game's
  automated-selector hook; choices outside CardSelectCmd (rare mid-combat
  relic selects) may still pop their UI during the fast-forward.
- Progress stats / metrics for replayed floors double-count (v1: accepted
  and documented; single-player, private).

---

## 8. State copy (clipboard) -- no wire traffic

`SimStateCopy.Copy()` (toolbar **Copy state**, `Ctrl+Shift+C`) puts the
current position on the clipboard and writes the same bytes to
`%LOCALAPPDATA%\SpireMentor\state-<ts>.json`. Nothing is sent to a client:
this is a hand-carried payload for the workbench's **Import state**
(`sts2-pycli`: `workbench/state_import.py` -> `POST /api/live/import-state`).

The payload is a **normal outbound message**, not an export format:

| Situation | Payload |
|---|---|
| A fight is in progress | `SimCombatHandler.BuildCombatState` stamped `type: advice_request`, `advice_for: combat_action` -- byte-identical to what the advisor lane sends for that board |
| A screen is open | `SimScreenAdvice.Describe().State` stamped `type: advice_request` and that screen's `advice_for` |
| A run with neither | `{"type": "run_snapshot"}` + the section-1 envelope |

Reusing the wire dialect is the point: the consumer has ONE parser, and a
hand-copied state is one the advisor could have sent. The envelope is what
makes the run rebuildable -- `seed`, `act`, `floor`, `run_state`, `map`
(graph + `visited` + `current`) and `rng` (`run_seed` + every stream cursor)
all ride on every message already, so the consumer needs no new field.

`run_snapshot` is the only type in this document that never crosses the wire.
A client that somehow receives one should ignore it: it answers nothing and
expects no reply.
