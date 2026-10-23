using System.Collections.Concurrent;
using System.Reflection;
using MegaCrit.Sts2.Core.Combat;
using MegaCrit.Sts2.Core.Commands;
using MegaCrit.Sts2.Core.Context;
using MegaCrit.Sts2.Core.Events;
using MegaCrit.Sts2.Core.Entities.Cards;
using MegaCrit.Sts2.Core.Entities.Creatures;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.GameActions;
using MegaCrit.Sts2.Core.Helpers;
using MegaCrit.Sts2.Core.Map;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Models.Cards;
using MegaCrit.Sts2.Core.Models.Characters;
using MegaCrit.Sts2.Core.Models.Acts;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Multiplayer;
using MegaCrit.Sts2.Core.CardSelection;
using MegaCrit.Sts2.Core.Entities.CardRewardAlternatives;
using MegaCrit.Sts2.Core.Entities.Merchant;
using MegaCrit.Sts2.Core.Entities.RestSite;
using MegaCrit.Sts2.Core.Rewards;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Models.Powers;
using MegaCrit.Sts2.Core.GameActions.Multiplayer;
using MegaCrit.Sts2.Core.Runs;
using MegaCrit.Sts2.Core.TestSupport;
using HarmonyLib;
using MegaCrit.Sts2.Core.Localization;
using MegaCrit.Sts2.Core.Multiplayer.Serialization;
using MegaCrit.Sts2.Core.Saves;
using MegaCrit.Sts2.Core.Timeline;
using MegaCrit.Sts2.Core.Timeline.Epochs;
using MegaCrit.Sts2.Core.Unlocks;

namespace Sts2Headless;

/// <summary>
/// Synchronization context that executes continuations inline immediately when
/// possible, and queues nested posts to drain after the current callback. Used
/// to make Task.Yield() / Cmd.Wait() chains run synchronously on a single thread.
///
/// Thread-safe via a single lock around the queue + executing flag. Inline
/// execution is preserved (matches the previous InlineSynchronizationContext
/// behavior) so .GetAwaiter().GetResult() patterns inside handlers still work
/// without nested message loops.
/// </summary>
internal sealed class GameSyncContext : SynchronizationContext
{
    private readonly Queue<(SendOrPostCallback, object?)> _queue = new();
    private bool _executing;
    private readonly object _lock = new();

    public override void Post(SendOrPostCallback d, object? state)
    {
        lock (_lock)
        {
            if (_executing)
            {
                _queue.Enqueue((d, state));
                return;
            }
            _executing = true;
        }
        try { d(state); }
        finally { DrainAndExit(); }
    }

    public override void Send(SendOrPostCallback d, object? state)
    {
        d(state);
    }

    public void Pump()
    {
        lock (_lock)
        {
            if (_executing) return;
            _executing = true;
        }
        DrainAndExit();
    }

    // Drains queued callbacks until empty, then clears _executing under the
    // lock in the same critical section as the emptiness check. This closes
    // the race where another thread could observe _executing == true, enqueue
    // an item, and have nobody drain it. On a callback exception _executing
    // is still cleared before the exception propagates, so subsequent Post()
    // calls don't deadlock.
    private void DrainAndExit()
    {
        try
        {
            while (true)
            {
                (SendOrPostCallback cb, object? st) item;
                lock (_lock)
                {
                    if (_queue.Count == 0) { _executing = false; return; }
                    item = _queue.Dequeue();
                }
                item.cb(item.st);
            }
        }
        catch
        {
            lock (_lock) { _executing = false; }
            throw;
        }
    }
}

/// <summary>
/// Dedicated thread that owns the game's SynchronizationContext. All public
/// RunSimulator methods are dispatched here via Submit so that game state is
/// touched from a single thread. The main thread (Program.cs) is reduced to
/// JSON I/O only.
///
/// Phase 1 of the async refactor: handlers still execute synchronously on the
/// game thread; later phases convert them to async/await with a drain-loop
/// based on ActionExecutor.FinishedExecutingActions().
/// </summary>
internal sealed class GameThread : IDisposable
{
    private readonly BlockingCollection<(Action work, TaskCompletionSource<bool> tcs)> _commands = new();
    private readonly Thread _thread;
    public GameSyncContext SyncContext { get; }

    public GameThread()
    {
        SyncContext = new GameSyncContext();
        _thread = new Thread(Run) { IsBackground = true, Name = "STS2-GameThread" };
        _thread.Start();
    }

    private void Run()
    {
        SynchronizationContext.SetSynchronizationContext(SyncContext);
        foreach (var (work, tcs) in _commands.GetConsumingEnumerable())
        {
            try
            {
                work();
                tcs.TrySetResult(true);
            }
            catch (Exception ex)
            {
                tcs.TrySetException(ex);
            }
        }
    }

    public T Submit<T>(Func<T> work)
    {
        var tcs = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        T result = default!;
        _commands.Add(((Action)(() => { result = work(); }), tcs));
        tcs.Task.GetAwaiter().GetResult();
        return result;
    }

    public void Submit(Action work)
    {
        var tcs = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        _commands.Add((work, tcs));
        tcs.Task.GetAwaiter().GetResult();
    }

    public void Dispose()
    {
        _commands.CompleteAdding();
        if (_thread.IsAlive) _thread.Join();
    }
}

/// <summary>
/// English localization lookup — loads localization_eng JSON files for display names.
/// </summary>
internal class LocLookup
{
    private readonly Dictionary<string, Dictionary<string, string>> _eng = new();

    public LocLookup()
    {
        var baseDir = Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "..");
        Load(Path.Combine(baseDir, "localization_eng"), _eng);
    }

    private static void Load(string dir, Dictionary<string, Dictionary<string, string>> target)
    {
        if (!Directory.Exists(dir)) return;
        foreach (var file in Directory.GetFiles(dir, "*.json"))
        {
            try
            {
                var name = Path.GetFileNameWithoutExtension(file);
                var data = System.Text.Json.JsonSerializer.Deserialize<Dictionary<string, string>>(File.ReadAllText(file));
                if (data != null) target[name] = data;
            }
            catch { }
        }
    }

    /// <summary>Get the English display name, or the key if not found.</summary>
    public string Name(string table, string key)
    {
        return _eng.GetValueOrDefault(table)?.GetValueOrDefault(key) ?? key;
    }

    public string? En(string table, string key) => _eng.GetValueOrDefault(table)?.GetValueOrDefault(key);

    /// <summary>Strip BBCode tags like [gold], [/blue], [b], [sine], etc.</summary>
    private static string StripBBCode(string text)
    {
        return System.Text.RegularExpressions.Regex.Replace(text, @"\[/?[a-zA-Z_][a-zA-Z0-9_=]*\]", "");
    }

    /// <summary>Language is fixed to English. Property kept for callsite compatibility.</summary>
    public string Lang { get; set; } = "en";

    /// <summary>Return the localized string for JSON output.</summary>
    public string Bilingual(string table, string key)
    {
        var en = _eng.GetValueOrDefault(table)?.GetValueOrDefault(key) ?? key;
        return StripBBCode(en);
    }

    // Convenience helpers using ModelId
    public string Card(string entry) => Bilingual("cards", entry + ".title");
    public string Monster(string entry) => Bilingual("monsters", entry + ".name");
    public string Relic(string entry) => Bilingual("relics", entry + ".title");
    public string Potion(string entry) => Bilingual("potions", entry + ".title");
    public string Power(string entry) => Bilingual("powers", entry + ".title");
    public string Event(string entry) => Bilingual("events", entry + ".title");
    public string Act(string entry) => Bilingual("acts", entry + ".title");

    /// <summary>Resolve a full loc key like "TABLE.KEY.SUB" by searching all tables.</summary>
    public string BilingualFromKey(string locKey)
    {
        foreach (var tableName in _eng.Keys)
        {
            var en = _eng.GetValueOrDefault(tableName)?.GetValueOrDefault(locKey);
            if (en != null) return en;
        }
        return locKey;
    }

    public bool IsLoaded => _eng.Count > 0;
}

/// <summary>
/// Full run simulator — manages the game lifecycle from character selection
/// through map navigation, combat, events, rest sites, shops, and act transitions.
/// Drives the engine forward until it hits a "decision point" requiring external input.
/// </summary>
public partial class RunSimulator
{
    private const int CurrentSaveSchemaVersion = 16;
    private RunState? _runState;
    private static bool _modelDbInitialized;
    private static readonly GameThread _gameThread = new();
    private static GameSyncContext _syncCtx => _gameThread.SyncContext;
    private readonly ManualResetEventSlim _turnStarted = new(false);
    private readonly ManualResetEventSlim _combatEnded = new(false);
    private static readonly LocLookup _loc = new();
    private bool _eventOptionChosen;
    // GH #316 — the EXACT "did the event advance?" signal, replacing the old
    // option-TextKey-identity heuristic. `EventModel.SetEventState` is the only
    // way an event page changes (`SetEventFinished` routes through it too), and
    // it raises `StateChanged`, so counting those invocations across an option's
    // `Chosen()` answers the question outright instead of inferring it.
    //
    // Identity heuristics could not: an event that deliberately re-presents the
    // SAME options with a new description advances while its keys stand still,
    // and the guard killed it. AbyssalBaths was capped at ONE Linger — only its
    // DESCRIPTION key moves (LINGER1..LINGER9, then clamped), while `Linger`
    // itself loops for as long as the player survives it — and SlipperyBridge
    // died the moment `GetHoldOnSuffix` saturates at "LOOP"
    // (7+ Hold Ons, both suffixes constant) — the unexplained force-finish on
    // seeds 1011/1013. Counting is not fooled by either.
    //
    // The counter is bumped from the thread pool (`Chosen()` runs there), so all
    // access is interlocked/volatile.
    private EventModel? _advanceTrackedEvent;
    private Action<EventModel>? _eventAdvanceHandler;
    private int _eventAdvanceCount;
    private int _lastEventAdvanceCount;

    // Pending rewards for card selection (populated after combat, before proceeding)
    private List<Reward>? _pendingRewards;
    private CardReward? _pendingCardReward;
    private bool _rewardsProcessed;
    private int _goldBeforeCombat;
    private int _lastKnownHp;

    private readonly HeadlessCardSelector _cardSelector = new();

    /// <summary>
    /// sts2-agent fork: set per run by <c>start_run{agent_mode:true}</c>. In agent mode
    /// the host surfaces player choices instead of making them; with it off the
    /// upstream flow is unchanged.
    /// </summary>
    private bool _agentMode;
    // Pending bundle selection (Scroll Boxes: pick 1 of N packs)
    private IReadOnlyList<IReadOnlyList<CardModel>>? _pendingBundles;
    private TaskCompletionSource<IEnumerable<CardModel>>? _pendingBundleTcs;
    // Pending shop card removal: the OnTryPurchaseWrapper task that finishes
    // the removal AFTER the player resolves the card selection.
    private Task? _pendingPurchaseTask;
    // Pending rest-site task (Smith): the ChooseLocalOption task that finishes
    // the upgrade AFTER the player resolves the card selection.
    private Task? _pendingRestSiteTask;
    // Pending event-option task: the `EventOption.Chosen()` task that, for a
    // relic whose `AfterObtained` opens a card selection (e.g. Neow's
    // PrecariousShears → `FromDeckForRemoval(2)` then `RemoveFromDeck` +
    // `Damage(16)`), is SUSPENDED at the `await` and must be driven to
    // completion AFTER the player resolves the selection — otherwise the
    // post-await effect (deck removal + HP damage) is silently dropped and the
    // run advances to the map with an unmodified deck (the systematic Neow
    // "16-HP skew" the run-parity sweep flagged). Mirrors `_pendingRestSiteTask`
    // / `_pendingPurchaseTask`.
    private Task? _pendingEventOptionTask;

    public Dictionary<string, object?> StartRun(string character, int ascension = 0, string? seed = null, string lang = "en", bool playedSave = false, string? act1 = null, bool agentMode = false) =>
        _gameThread.Submit(() =>
        {
            _agentMode = agentMode;
            AgentModeActive = agentMode;
            _agentSphereClick = null;
            ResetAgentEventResume();
            PendingCrystalSphere = null;
            _cardSelector.WaitForever = agentMode;
            _cardSelector.KeepCancelableSingletons = agentMode;
            AgentCustomRewardsOffer = agentMode ? OfferCustomRewardsToClient : null;
            _lastDecision = null;
            return Tracked("start_run", () => StartRunImpl(character, ascension, seed, lang, playedSave, act1));
        });

    private Dictionary<string, object?> StartRunImpl(string character, int ascension, string? seed, string lang, bool playedSave = false, string? act1 = null)
    {
        try
        {
            _loc.Lang = lang;
            EnsureModelDbInitialized();

            var player = CreatePlayer(character, playedSave);
            if (player == null)
                return Error($"Unknown character: {character}");

            // Canonicalize a user-supplied seed exactly as the real game does
            // for typed seeds (StartRunLobby.cs:619 → SeedHelper.CanonicalizeSeed:
            // ToUpperInvariant, 'O'->'0', 'I'->'1', Trim). The headless previously
            // fed the raw string straight to RunState, so e.g. "seed2" and "SEED2"
            // produced different runs here while the game (and the sts2_rs engine,
            // which canonicalizes) treat them identically. Using the game's own
            // helper guarantees byte-for-byte parity with the real seeding path.
            var seedStr = seed != null
                ? SeedHelper.CanonicalizeSeed(seed)
                : "headless_" + DateTimeOffset.UtcNow.ToUnixTimeSeconds();
            Log($"Creating RunState with seed={seedStr}");

            // Act-1 variant. STS2 ships TWO acts at Index 0 -- Overgrowth
            // (IsDefault) and Underdocks -- and the run picks one in
            // StartRunLobby.BeginRunLocally, a LOBBY class this harness never
            // touches. So CreateForTest's `acts ?? ActModel.GetDefaultList()`
            // fallback (RunState.cs:577) silently pinned every oracle run to
            // Overgrowth, and every act-1 parity number this repo has produced
            // compared Overgrowth against Overgrowth -- a check that could not
            // fail. `act1` reopens that axis: "overgrowth" / "underdocks" force
            // a variant, "roll" reproduces the real lobby roll off the
            // `act_selection` stream. Null keeps the historical default.
            IReadOnlyList<ActModel>? acts = null;
            if (act1 != null)
            {
                var chosen = ResolveAct1(act1, seedStr, playedSave);
                if (chosen == null)
                    return Error($"Unknown act1: {act1} (expected overgrowth|underdocks|roll)");
                var actList = ActModel.GetDefaultList().ToList();
                actList[0] = chosen;
                acts = actList;
                Log($"Act 1 = {chosen.Id.Entry} (act1={act1})");
            }

            // Use CreateForTest which properly handles mutable copies internally
            _runState = RunState.CreateForTest(
                players: new[] { player },
                acts: acts,
                ascensionLevel: ascension,
                seed: seedStr
            );

            // Set up RunManager with test mode
            var netService = new NetSingleplayerGameService();
            RunManager.Instance.SetUpTest(_runState, netService);
            LocalContext.NetId = netService.NetId;

            // Force Neow event (blessing selection at start). Skipped for a
            // played_save run: SetUpTest -> InitializeNewRun already called
            // RunManager.SetStartedWithNeowFlag() above, which computed this
            // from the played-save UnlockState built in CreatePlayer (NEOW_EPOCH
            // sits at EpochState.Obtained there, never Revealed) and naturally
            // lands on false -- matching a real save with many runs where Neow
            // was never manually revealed on the Timeline screen. See
            // BuildUnlockState's doc comment for the full mechanism.
            if (!playedSave)
                _runState.ExtraFields.StartedWithNeow = true;

            // Generate rooms for all acts
            RunManager.Instance.GenerateRooms();
            Log("Rooms generated");

            // Launch the run
            RunManager.Instance.Launch();
            Log("Run launched");

            // Register event handlers for combat turn transitions
            CombatManager.Instance.TurnStarted += _ => _turnStarted.Set();
            CombatManager.Instance.CombatEnded += _ => _combatEnded.Set();

            // Finalize starting relics
            RunManager.Instance.FinalizeStartingRelics().GetAwaiter().GetResult();
            Log("Starting relics finalized");

            // Enter first act (generates map)
            RunManager.Instance.EnterAct(0, doTransition: false).GetAwaiter().GetResult();
            Log("Entered Act 0");

            // Register card selector for cards that need player choice
            CardSelectCmd.UseSelector(_cardSelector);
            LocPatches._bundleSimRef = this;

            // Now we should be at the map — detect decision point
            return DetectDecisionPoint();
        }
        catch (Exception ex)
        {
            return ErrorWithTrace("StartRun failed", ex);
        }
    }

    // ─── Test/Debug commands ───

    private static readonly System.Reflection.BindingFlags NonPublic =
        System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic;

    /// <summary>Get the backing List&lt;T&gt; behind an IReadOnlyList property via reflection.</summary>
    private static List<T>? GetBackingList<T>(object obj, string fieldName)
    {
        var field = obj.GetType().GetField(fieldName, NonPublic);
        return field?.GetValue(obj) as List<T>;
    }

    private static void SetField(object obj, string fieldName, object? value)
    {
        var field = obj.GetType().GetField(fieldName, NonPublic);
        field?.SetValue(obj, value);
    }

    /// <summary>Walk up the type hierarchy to find an inherited non-public field
    /// (e.g. ActModel's protected <c>_rooms</c> from a derived Overgrowth).</summary>
    private static object? GetInheritedField(object obj, string fieldName)
    {
        var t = obj.GetType();
        while (t != null)
        {
            var f = t.GetField(fieldName, NonPublic);
            if (f != null) return f.GetValue(obj);
            t = t.BaseType;
        }
        return null;
    }

    /// <summary>Dump each act's pre-generated room contents (the
    /// <c>RoomSet.normalEncounters</c> / <c>eliteEncounters</c> / <c>Boss</c>
    /// lists produced by <c>GenerateRooms</c> at run start) as encounter
    /// <c>Id.Entry</c> strings. The Rust port (`run_room_gen`) replicates these
    /// off the shared <c>up_front</c> stream; this is the ground-truth oracle for
    /// the M3 encounter-identity parity check
    /// (`run_parity.py --encounter-parity`).</summary>
    public Dictionary<string, object?> DumpRooms()
    {
        if (_runState == null) return Error("No run in progress");
        var acts = new List<object?>();
        foreach (var act in _runState.Acts)
        {
            if (GetInheritedField(act, "_rooms") is not MegaCrit.Sts2.Core.Rooms.RoomSet roomSet)
                continue;
            List<string> Ids(List<MegaCrit.Sts2.Core.Models.EncounterModel> xs) =>
                xs.Select(e => e?.Id.Entry ?? "?").ToList();
            acts.Add(new Dictionary<string, object?>
            {
                ["normal"] = Ids(roomSet.normalEncounters),
                ["elite"] = Ids(roomSet.eliteEncounters),
                ["boss"] = roomSet.Boss?.Id.Entry,
                // M5 event-pool parity: the per-act `RoomSet.events` list is
                // `(AllEvents ++ AllSharedEvents)` UnstableShuffled off `up_front`
                // in `ActModel.GenerateRooms`; `eventsVisited` is the pick cursor.
                ["events"] = roomSet.events.Select(e => e?.Id.Entry ?? "?").ToList(),
                ["events_visited"] = roomSet.eventsVisited,
            });
        }
        return new Dictionary<string, object?> { ["type"] = "rooms", ["acts"] = acts };
    }

    /// <summary>Dump the freshly-generated <c>ActMap</c> GRAPH for every act
    /// (1/2/3) without playing — the all-acts oracle for
    /// <c>event_parity</c>'s sibling <c>act23_map_parity.py</c> (the Act-1-only
    /// <c>run_setup_parity.py</c> can read only the live <c>_runState.Map</c>).
    /// Each act's map is a pure function of <c>(seed, act)</c>: the live game
    /// builds it via <c>StandardActMap.CreateFor</c> off
    /// <c>new Rng(RunState.Rng.Seed, $"act_{N}_map")</c> — independent of run
    /// progress — so acts 2/3 are fully determined at run start and need no
    /// combat to reach. We reconstruct each act's <c>StandardActMap</c> with
    /// the same per-act named RNG + <c>ActModel</c> and serialize its
    /// node-type/edge graph (the same shape <c>GetFullMapImpl</c> emits, minus
    /// the traversal-only visited/current fields). <c>replaceTreasureWithElites</c>
    /// is <c>false</c> to match the Rust <c>act_map_for</c> oracle and the
    /// base-ascension fresh-run default.</summary>
    public Dictionary<string, object?> DumpMaps() => _gameThread.Submit(() => DumpMapsImpl());

    private Dictionary<string, object?> DumpMapsImpl()
    {
        if (_runState == null) return Error("No run in progress");
        var actList = _runState.Acts.ToList();
        bool isMultiplayer = _runState.Players.Count > 1;
        var acts = new List<object?>();
        for (int i = 0; i < actList.Count; i++)
        {
            var act = actList[i];
            var rng = new MegaCrit.Sts2.Core.Random.Rng(_runState.Rng.Seed, $"act_{i + 1}_map");
            var map = new StandardActMap(rng, act, isMultiplayer, shouldReplaceTreasureWithElites: false, act.HasSecondBoss);
            acts.Add(SerializeMapGraph(map));
        }
        return new Dictionary<string, object?> { ["type"] = "maps", ["acts"] = acts };
    }

    /// <summary>Serialize an <c>ActMap</c>'s node-type/edge graph into the
    /// <c>{rows: [[{col,row,type,children}]], boss: {col,row,type}}</c> schema
    /// that <c>run_setup_parity._csharp_paths</c> consumes (a subset of
    /// <c>GetFullMapImpl</c>'s output — the traversal-dependent
    /// <c>visited</c>/<c>current</c> fields are irrelevant for a dumped
    /// map).</summary>
    private static Dictionary<string, object?> SerializeMapGraph(ActMap map)
    {
        var rows = new List<List<Dictionary<string, object?>>>();
        for (int row = 0; row < map.GetRowCount(); row++)
        {
            var rowNodes = new List<Dictionary<string, object?>>();
            foreach (var point in map.GetPointsInRow(row))
            {
                if (point == null) continue;
                var children = point.Children?.Select(ch => new Dictionary<string, object?>
                {
                    ["col"] = (int)ch.coord.col,
                    ["row"] = (int)ch.coord.row,
                }).ToList();
                rowNodes.Add(new Dictionary<string, object?>
                {
                    ["col"] = (int)point.coord.col,
                    ["row"] = (int)point.coord.row,
                    ["type"] = point.PointType.ToString(),
                    ["children"] = children,
                });
            }
            if (rowNodes.Count > 0)
                rows.Add(rowNodes);
        }
        var boss = new Dictionary<string, object?>
        {
            ["col"] = (int)map.BossMapPoint.coord.col,
            ["row"] = (int)map.BossMapPoint.coord.row,
            ["type"] = map.BossMapPoint.PointType.ToString(),
        };
        return new Dictionary<string, object?> { ["rows"] = rows, ["boss"] = boss };
    }

    /// <summary>Roll a single 3-card REGULAR-encounter card reward directly
    /// off the player's <c>PlayerRng.Rewards</c> stream (fresh, counter 0
    /// right after StartRun) and return each option's id + upgraded flag.
    /// Isolates the reward-RNG + character-pool ordering for the M4
    /// <c>run_parity.py --reward-parity</c> check, with no combat plumbing.
    /// Mirrors <c>CardFactory.CreateForReward(player, 3,
    /// CardCreationOptions.ForRoom(player, RoomType.Monster))</c> — the exact
    /// path a normal-encounter victory takes (CardReward.cs:119).</summary>
    public Dictionary<string, object?> DumpCardReward(int count = 3, string roomType = "Monster")
    {
        if (_runState == null) return Error("No run in progress");
        var player = _runState.Players[0];
        var rt = roomType switch
        {
            "Elite" => RoomType.Elite,
            "Boss" => RoomType.Boss,
            _ => RoomType.Monster,
        };
        var options = CardCreationOptions.ForRoom(player, rt);
        var results = MegaCrit.Sts2.Core.Factories.CardFactory
            .CreateForReward(player, count, options).ToList();
        var cards = results.Select(r => (object?)new Dictionary<string, object?>
        {
            ["id"] = r.Card.Id.Entry,
            ["upgraded"] = r.Card.IsUpgraded,
        }).ToList();
        return new Dictionary<string, object?> { ["type"] = "card_reward", ["cards"] = cards };
    }

    public Dictionary<string, object?> DumpRelicReward(int count = 1)
    {
        if (_runState == null) return Error("No run in progress");
        var player = _runState.Players[0];
        // Mirror an elite/combat relic drop: each pull rolls a rarity off the
        // player's `Rewards` stream (`RelicFactory.RollRarity(player)`) and pops
        // the front of that rarity's grab-bag deque (then drains it from the
        // shared bag). Counter starts at 0 on a fresh run, so `count==1` isolates
        // the first reward-stream relic roll + grab-bag ordering. The bag is
        // mutated per pull, so back-to-back pulls drain like real consecutive
        // drops. M4 relic-reward parity (run_parity.py --relic-reward-parity).
        var relics = new List<object?>();
        for (int i = 0; i < count; i++)
        {
            var relic = MegaCrit.Sts2.Core.Factories.RelicFactory.PullNextRelicFromFront(player);
            relics.Add(new Dictionary<string, object?> { ["id"] = relic.Id.Entry });
        }
        return new Dictionary<string, object?> { ["type"] = "relic_reward", ["relics"] = relics };
    }

    /// <summary>
    /// Serialize a relic for an OFFER surface (shop, content dump, …): localized
    /// name + description plus the relic's DynamicVars, so the renderer can
    /// substitute {Gold}/{Heal}/… with real numbers. Centralized so every offer
    /// path stays in sync — the placeholder-regression test pins this helper.
    /// </summary>
    private Dictionary<string, object?> RelicOfferDict(RelicModel? relic, int? index = null)
    {
        var entry = relic?.Id.Entry ?? "?";
        var vars = new Dictionary<string, object?>();
        if (relic != null)
        {
            try { foreach (var dv in relic.DynamicVars.Values) vars[dv.Name] = (int)dv.BaseValue; } catch { }
        }
        var d = new Dictionary<string, object?>
        {
            ["id"] = entry,
            ["name"] = _loc.Relic(entry),
            ["description"] = _loc.Bilingual("relics", entry + ".description"),
            ["vars"] = vars.Count > 0 ? vars : null,
        };
        if (index != null) d["index"] = index;
        return d;
    }

    /// <summary>
    /// Serialize a potion for an OFFER surface (shop, content dump, …): localized
    /// name + description plus the potion's DynamicVars, so {Block}/{Damage}/… in
    /// the template resolve to real numbers. Mirrors <see cref="RelicOfferDict"/>;
    /// centralized so the shop and the placeholder-regression test stay in sync.
    /// </summary>
    private Dictionary<string, object?> PotionOfferDict(PotionModel? potion, int? index = null)
    {
        var entry = potion?.Id.Entry ?? "?";
        var vars = new Dictionary<string, object?>();
        if (potion != null)
        {
            try { foreach (var dv in potion.DynamicVars.Values) vars[dv.Name] = (int)dv.BaseValue; } catch { }
        }
        var d = new Dictionary<string, object?>
        {
            ["id"] = entry,
            ["name"] = _loc.Potion(entry),
            ["description"] = _loc.Bilingual("potions", entry + ".description"),
            ["vars"] = vars.Count > 0 ? vars : null,
        };
        if (index != null) d["index"] = index;
        return d;
    }

    /// <summary>
    /// The full event roster, for the event/option coverage harness (GH #313).
    /// </summary>
    /// <remarks>
    /// A play-policy-driven sweep only ever exercises the options its policy
    /// likes, so a throw behind a rarely-taken branch can hide indefinitely —
    /// `Trial.Accept` threw on its FIRST line and went unnoticed until the
    /// 2026-07-29 corpus happened to walk into it. The harness needs to
    /// enumerate events itself rather than wait for one to roll, and
    /// `ModelDb.AllEvents` is the only authoritative roster.
    /// </remarks>
    public Dictionary<string, object?> DumpEvents()
    {
        var events = new List<object?>();
        var skipped = 0;
        foreach (var e in ModelDb.AllEvents)
        {
            try
            {
                events.Add(new Dictionary<string, object?>
                {
                    ["id"] = e.Id.Entry,
                    ["name"] = _loc.Event(e.Id.Entry),
                    ["type"] = e.GetType().Name,
                    ["is_deterministic"] = e.IsDeterministic,
                });
            }
            catch (Exception ex)
            {
                // A silently shortened roster is silently reduced coverage — the
                // harness would sweep fewer events and still report success. Count
                // it, publish the count, and say which one.
                skipped++;
                Log($"DumpEvents: skipping {e.Id.Entry}: {ex.GetType().Name}: {ex.Message}");
            }
        }
        return new Dictionary<string, object?>
        {
            ["type"] = "events",
            ["events"] = events,
            ["total"] = ModelDb.AllEvents.Count(),
            ["skipped"] = skipped,
        };
    }

    /// <summary>
    /// Enumerate every relic / potion / card with its localized description and
    /// the DynamicVars needed to resolve it. Backs the placeholder-regression
    /// test: no offered text should ever leak an unresolved {Var} placeholder.
    /// </summary>
    public Dictionary<string, object?> DumpContent()
    {
        var relics = new List<object?>();
        foreach (var r in ModelDb.AllRelics)
        {
            try { relics.Add(RelicOfferDict(r)); } catch { }
        }

        var potions = new List<object?>();
        foreach (var p in ModelDb.AllPotions)
        {
            try { potions.Add(PotionOfferDict(p)); } catch { }
        }

        var cards = new List<object?>();
        foreach (var c in ModelDb.AllCards)
        {
            try
            {
                var cm = c.ToMutable();
                var stats = new Dictionary<string, object?>();
                try { foreach (var dv in cm.DynamicVars.Values) stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue; } catch { }
                cards.Add(new Dictionary<string, object?>
                {
                    ["id"] = c.Id.Entry,
                    ["name"] = _loc.Card(c.Id.Entry),
                    ["description"] = _loc.Bilingual("cards", c.Id.Entry + ".description"),
                    ["stats"] = stats.Count > 0 ? stats : null,
                    ["upgraded"] = cm.IsUpgraded,
                });
            }
            catch { }
        }

        return new Dictionary<string, object?>
        {
            ["type"] = "content",
            ["relics"] = relics,
            ["potions"] = potions,
            ["cards"] = cards,
        };
    }

    public Dictionary<string, object?> DumpShop()
    {
        if (_runState == null) return Error("No run in progress");
        var player = _runState.Players[0];
        // Generate a full normal-merchant inventory off the player's fresh
        // `Shops` (card pick / cost / on-sale index / potion roll) and `Rewards`
        // (card-rarity roll + per-card upgrade no-op roll + relic RollRarity)
        // streams. `MerchantInventory.CreateForNormalMerchant` walks: 5 colored
        // card slots, 2 colorless slots, 3 relic slots, 3 potion slots, 1 card
        // removal. Capture both stream counters before/after so the Rust port
        // can be checked on RNG-read counts as well as item identity. M4 shop
        // parity (run_parity.py --shop-parity).
        int shops0 = player.PlayerRng.Shops.Counter;
        int rewards0 = player.PlayerRng.Rewards.Counter;
        var inv = MerchantInventory.CreateForNormalMerchant(player);
        int shopsReads = player.PlayerRng.Shops.Counter - shops0;
        int rewardsReads = player.PlayerRng.Rewards.Counter - rewards0;

        var cards = inv.CharacterCardEntries.Select(e => (object?)new Dictionary<string, object?>
        {
            ["id"] = e.CreationResult?.Card.Id.Entry,
            ["upgraded"] = e.CreationResult?.Card.IsUpgraded ?? false,
            ["on_sale"] = e.IsOnSale,
            ["price"] = e.Cost,
        }).ToList();
        var colorless = inv.ColorlessCardEntries.Select(e => (object?)new Dictionary<string, object?>
        {
            ["id"] = e.CreationResult?.Card.Id.Entry,
            ["upgraded"] = e.CreationResult?.Card.IsUpgraded ?? false,
            ["price"] = e.Cost,
        }).ToList();
        var relics = inv.RelicEntries.Select(e => (object?)new Dictionary<string, object?>
        {
            ["id"] = e.Model?.Id.Entry,
            ["price"] = e.Cost,
        }).ToList();
        var potions = inv.PotionEntries.Select(e => (object?)new Dictionary<string, object?>
        {
            ["id"] = e.Model?.Id.Entry,
            ["price"] = e.Cost,
        }).ToList();

        return new Dictionary<string, object?>
        {
            ["type"] = "shop",
            ["cards"] = cards,
            ["colorless"] = colorless,
            ["relics"] = relics,
            ["potions"] = potions,
            ["removal_cost"] = inv.CardRemovalEntry?.Cost,
            ["shops_reads"] = shopsReads,
            ["rewards_reads"] = rewardsReads,
        };
    }

    public Dictionary<string, object?> SetPlayer(Dictionary<string, System.Text.Json.JsonElement> args) =>
        _gameThread.Submit(() => SetPlayerImpl(args));

    private Dictionary<string, object?> SetPlayerImpl(Dictionary<string, System.Text.Json.JsonElement> args)
    {
        try
        {
            if (_runState == null) return Error("No run in progress");
            var player = _runState.Players[0];

            if (args.TryGetValue("hp", out var hpEl) && player.Creature != null)
                SetField(player.Creature, "_currentHp", hpEl.GetInt32());
            if (args.TryGetValue("max_hp", out var mhpEl) && player.Creature != null)
                SetField(player.Creature, "_maxHp", mhpEl.GetInt32());
            if (args.TryGetValue("gold", out var goldEl))
                player.Gold = goldEl.GetInt32();

            if (args.TryGetValue("relics", out var relicsEl))
            {
                var list = GetBackingList<RelicModel>(player, "_relics");
                if (list != null)
                {
                    list.Clear();
                    foreach (var rEl in relicsEl.EnumerateArray())
                    {
                        var id = rEl.GetString();
                        if (id == null) continue;
                        var model = ModelDb.GetById<RelicModel>(new ModelId("RELIC", id));
                        if (model != null)
                        {
                            // sts2-rs-hqb6k: must assign Owner before adding,
                            // otherwise RunState.IterateHookListeners → Contains
                            // NREs on `relicModel.Owner.IsActiveForHooks` the
                            // next time a hook iterates listeners (e.g. during
                            // EnterRoom → BeforeRoomEntered).
                            var mutable = model.ToMutable();
                            mutable.Owner = player;
                            list.Add(mutable);
                        }
                    }
                }
            }
            if (args.TryGetValue("deck", out var deckEl))
            {
                // Remove existing cards from RunState tracking
                foreach (var c in player.Deck.Cards.ToList())
                    _runState.RemoveCard(c);
                player.Deck.Clear(silent: true);
                // Add new cards via RunState.CreateCard (sets Owner + registers)
                foreach (var cEl in deckEl.EnumerateArray())
                {
                    var id = cEl.GetString();
                    if (id == null) continue;
                    // Sweep v2: optional "+1" upgrade marker ("BASH+1").
                    // Applied through the canonical save-load path:
                    // CardModel.FromSerializable replays UpgradeInternal +
                    // FinalizeUpgradeInternal per level (CardModel.cs:1736) so
                    // OnUpgrade fires and DynamicVars recalculate — never poke
                    // CurrentUpgradeLevel directly.
                    // Only "+1" is accepted (STS2 cards are single-level; the
                    // Rust scenario builder enforces the same gate), and the
                    // MaxUpgradeLevel clamp means a curse/status "+1" is
                    // silently a no-op — the loop never executes
                    // (MaxUpgradeLevel 0), which also guards the
                    // CurrentUpgradeLevel setter's throw — mirroring Rust's
                    // is_upgradable() check.
                    int upgrades = 0;
                    int plus = id.IndexOf('+');
                    if (plus >= 0)
                    {
                        if (!int.TryParse(id[(plus + 1)..], out upgrades) || upgrades != 1)
                            return Error($"bad upgrade suffix in card id '{id}': expected +1");
                        id = id[..plus];
                    }
                    var canonical = ModelDb.GetById<CardModel>(new ModelId("CARD", id));
                    // pre-existing: unknown ids are silently skipped (matches
                    // relic/potion handling); a typo'd base id with +1 lands
                    // here, not in the suffix error.
                    if (canonical != null)
                    {
                        var card = _runState.CreateCard(canonical, player);
                        for (int i = 0; i < upgrades && card.CurrentUpgradeLevel < card.MaxUpgradeLevel; i++)
                        {
                            card.UpgradeInternal();
                            card.FinalizeUpgradeInternal();
                        }
                        player.Deck.AddInternal(card, silent: true);
                    }
                }
            }
            if (args.TryGetValue("potions", out var potionsEl))
            {
                var slots = GetBackingList<PotionModel>(player, "_potionSlots")
                         ?? GetBackingList<PotionModel?>(player, "_potionSlots") as System.Collections.IList;
                if (slots != null)
                {
                    for (int i = 0; i < slots.Count; i++) slots[i] = null;
                    int idx = 0;
                    foreach (var pEl in potionsEl.EnumerateArray())
                    {
                        if (idx >= slots.Count) break;
                        var id = pEl.GetString();
                        if (id != null)
                        {
                            var model = ModelDb.GetById<PotionModel>(new ModelId("POTION", id));
                            if (model != null)
                            {
                                // sts2-rs-hqb6k: same Owner gotcha as relics —
                                // Contains(potionModel) NREs on null Owner during
                                // hook listener iteration.
                                var mutable = model.ToMutable();
                                mutable.Owner = player;
                                slots[idx] = mutable;
                            }
                        }
                        idx++;
                    }
                }
            }

            Log($"SetPlayer: hp={player.Creature?.CurrentHp} gold={player.Gold} relics={player.Relics.Count} deck={player.Deck?.Cards?.Count}");
            return new Dictionary<string, object?>
            {
                ["type"] = "ok",
                ["player"] = PlayerSummary(player),
            };
        }
        catch (Exception ex) { return ErrorWithTrace("SetPlayer failed", ex); }
    }

    public Dictionary<string, object?> EnterRoom(string roomType, string? encounter, string? eventId) =>
        _gameThread.Submit(() => Tracked("enter_room", () => EnterRoomImpl(roomType, encounter, eventId)));

    private Dictionary<string, object?> EnterRoomImpl(string roomType, string? encounter, string? eventId)
    {
        try
        {
            if (_runState == null) return Error("No run in progress");
            var runState = _runState;
            Log($"EnterRoom: type={roomType} encounter={encounter} event={eventId}");

            AbstractRoom room;
            switch (roomType.ToLowerInvariant())
            {
                case "combat":
                case "monster":
                case "elite":
                {
                    if (string.IsNullOrEmpty(encounter))
                        encounter = "SHRINKER_BEETLE_WEAK"; // default encounter
                    var encModel = ModelDb.GetById<EncounterModel>(new ModelId("ENCOUNTER", encounter));
                    if (encModel == null) return Error($"Unknown encounter: {encounter}");
                    room = new CombatRoom(encModel.ToMutable(), runState);
                    break;
                }
                case "shop":
                    room = new MerchantRoom();
                    break;
                case "rest":
                case "rest_site":
                    room = new RestSiteRoom();
                    break;
                case "event":
                {
                    if (string.IsNullOrEmpty(eventId))
                        return Error("event requires 'event' parameter (e.g. CHANGELING_GROVE)");
                    var evModel = ModelDb.GetById<EventModel>(new ModelId("EVENT", eventId));
                    if (evModel == null) return Error($"Unknown event: {eventId}");
                    room = new EventRoom(evModel);
                    break;
                }
                case "treasure":
                    room = new TreasureRoom(_runState.CurrentActIndex);
                    break;
                default:
                    return Error($"Unknown room type: {roomType}");
            }

            // sts2-agent fork: the new room gets its own rewards (upstream keeps the
            // previous combat's "rewards processed" flag, so a second entered combat
            // had none).
            if (_agentMode)
            {
                _rewardsProcessed = false;
                ResetAgentRewards();
                ResetAgentRestTreasure();
            }
            RunManager.Instance.EnterRoom(room).GetAwaiter().GetResult();
            _syncCtx.Pump();
            WaitForActionExecutor();
            ArmCombatHistoryTrace();
            return DetectDecisionPoint();
        }
        catch (Exception ex) { return ErrorWithTrace("EnterRoom failed", ex); }
    }

    // Combat-history trace (`STS2_COMBAT_HISTORY_TRACE=1`): stream every
    // `CombatHistory` entry to stderr as it is recorded, tagged with the live
    // Slow counters. `CombatHistoryEntry.Description` already renders each
    // event in full ("IRONCLAD started playing CATASTROPHE", "IRONCLAD dealt 12
    // damage to VINE_SHAMBLER"), so the result is the exact ORDER in which C#
    // started/finished every card play, dealt every hit and moved every card —
    // the one thing `PILETRACE` / `RNGTRACE` cannot show, and the only way to
    // diff an ordering defect that leaves the piles' CONTENTS identical.
    //
    // GH #500 was diagnosed with it: an auto-play loop's remaining iterations
    // and the turn-start tail were both running at the wrong point in the
    // nesting, which no count-based state diff can see. Inert (not even
    // subscribed) without the env var.
    private int _histTraceIdx;

    private void ArmCombatHistoryTrace()
    {
        if (Environment.GetEnvironmentVariable("STS2_COMBAT_HISTORY_TRACE") == null) return;
        var mgr = CombatManager.Instance;
        var hist = mgr?.History;
        var combatState = mgr?.DebugOnlyGetState();
        if (hist == null || combatState == null) return;
        _histTraceIdx = 0;
        hist.Changed += () =>
        {
            var all = hist.Entries.ToList();
            if (all.Count < _histTraceIdx) _histTraceIdx = 0;
            for (; _histTraceIdx < all.Count; _histTraceIdx++)
            {
                var e = all[_histTraceIdx];
                var slow = string.Join(
                    ",",
                    combatState.Enemies.Where(x => x != null)
                        .SelectMany(x => x.Powers)
                        .Where(pw => pw.Id.Entry.Contains("SLOW"))
                        .Select(pw => (pw.DisplayAmount / 10).ToString()));
                Console.Error.WriteLine(
                    $"[HISTTRACE] {e.GetType().Name}: {e.Description} slow=[{slow}]");
            }
        };
    }

    public Dictionary<string, object?> SetDrawOrder(List<string> cardIds) =>
        _gameThread.Submit(() => SetDrawOrderImpl(cardIds));

    private Dictionary<string, object?> SetDrawOrderImpl(List<string> cardIds)
    {
        try
        {
            if (_runState == null) return Error("No run in progress");
            var player = _runState.Players[0];
            var pcs = player.PlayerCombatState;
            if (pcs?.DrawPile == null) return Error("Not in combat");

            var drawList = GetBackingList<CardModel>(pcs.DrawPile, "_cards");
            if (drawList == null) return Error("Cannot access draw pile");

            var newOrder = new List<CardModel>();
            var available = new List<CardModel>(drawList);
            foreach (var cardId in cardIds)
            {
                var match = available.FirstOrDefault(c =>
                    c.Id.Entry.Equals(cardId, StringComparison.OrdinalIgnoreCase));
                if (match != null)
                {
                    newOrder.Add(match);
                    available.Remove(match);
                }
            }
            newOrder.AddRange(available);

            drawList.Clear();
            drawList.AddRange(newOrder);

            Log($"SetDrawOrder: {newOrder.Count} cards, top={newOrder.FirstOrDefault()?.Id.Entry}");
            return new Dictionary<string, object?>
            {
                ["type"] = "ok",
                ["draw_pile_count"] = drawList.Count,
                ["top_cards"] = newOrder.Take(5).Select(c => _loc.Card(c.Id.Entry)).ToList(),
            };
        }
        catch (Exception ex) { return ErrorWithTrace("SetDrawOrder failed", ex); }
    }

    // PARITY-FUZZ: legal_actions command for cross-engine fuzz harness.
    //
    // Emits ONLY actions that are playable RIGHT NOW. Filters via
    // card.CanPlay(out reason, out _) — the same predicate DoPlayCard
    // uses at L1049 / RunSimulator.cs:971. CanPlay covers energy cost,
    // unplayable flags, X-cost gating, and any per-card preconditions,
    // so callers can assume every emitted action will succeed if
    // dispatched immediately. No `cost_ok` field — implied true.
    //
    // PARITY-FUZZ: target_index semantics.
    //   - For TargetType.AnyEnemy cards we emit one play_card per
    //     alive enemy. target_index is an index into the *alive
    //     subset* of CombatState.Enemies, NOT the raw Enemies array.
    //     This matches how DoPlayCard resolves args["target_index"]
    //     (see L1034: state.Enemies.Where(e => e.IsAlive).ToList()).
    //   - Rust normalizer (Task 5) must canonicalize the same way:
    //     drop dead-enemy slots before indexing.
    //   - For non-AnyEnemy cards (Self / None / All) target_index is
    //     null and the game resolves the target internally.
    //
    // APIs used (verified in DoPlayCard, L1004-L1069):
    //   - pcs.Hand.Cards — list of CardModel
    //   - card.CanPlay(out reason, out _) — full playability gate
    //   - card.TargetType == TargetType.AnyEnemy — needs-target predicate
    //   - CombatManager.Instance.DebugOnlyGetState().Enemies + .IsAlive
    public Dictionary<string, object?> LegalActions() =>
        _gameThread.Submit(LegalActionsImpl);

    private Dictionary<string, object?> LegalActionsImpl()
    {
        try
        {
            if (_runState == null) return Error("No run in progress");
            var player = _runState.Players[0];
            var pcs = player.PlayerCombatState;
            if (pcs?.Hand == null) return Error("Not in combat");

            var energy = pcs.Energy;
            var hand = pcs.Hand.Cards;
            var combatState = CombatManager.Instance.DebugOnlyGetState();
            var aliveCount = combatState?.Enemies?
                .Count(e => e != null && e.IsAlive) ?? 0;

            var actions = new List<Dictionary<string, object?>>();
            for (int i = 0; i < hand.Count; i++)
            {
                var card = hand[i];
                // Full playability gate — mirrors DoPlayCard L1049.
                if (!card.CanPlay(out _, out _)) continue;

                var needsTarget = card.TargetType == TargetType.AnyEnemy;
                // sts2-agent fork: with no living enemy (e.g. TestSubject between
                // phases) a targeted card has no legal play; the game cancels it and the
                // host used to report "still in hand after action".
                if (needsTarget && aliveCount == 0) continue;
                if (needsTarget && aliveCount > 0)
                {
                    for (int t = 0; t < aliveCount; t++)
                    {
                        actions.Add(new Dictionary<string, object?>
                        {
                            ["action"] = "play_card",
                            ["card_index"] = i,
                            ["target_index"] = t,
                        });
                    }
                }
                else
                {
                    actions.Add(new Dictionary<string, object?>
                    {
                        ["action"] = "play_card",
                        ["card_index"] = i,
                        ["target_index"] = null,
                    });
                }
            }
            actions.Add(new Dictionary<string, object?> { ["action"] = "end_turn" });

            // PARITY-FUZZ: enumerate held potions as use_potion actions so the
            // cross-engine harness can exercise the potion surface (it picks
            // from this list). We emit ONE action per occupied slot with a
            // null target, matching the Rust engine's legal_actions, which
            // surfaces Action::DrinkPotion per slot with target=None and lets
            // the on-drink dispatcher resolve the target internally (per-enemy
            // target fan-out is deferred on the Rust side, sts2-rs-h9u). The
            // C# apply path (DoUsePotion L1724-1728) likewise resolves a null
            // target to the first alive enemy for AnyEnemy potions, so both
            // engines apply the equivalent effect.
            var potions = player.Potions?.ToList() ?? new();
            for (int i = 0; i < potions.Count; i++)
            {
                if (potions[i] == null) continue;
                // Skip PotionUsage.Automatic. Those potions can NEVER be drunk
                // by the player: the game disables the Use button for them
                // (NPotionPopup.cs:273) and FairyInABottle.OnUse is documented
                // "should never be executed directly by UsePotionAction" -- it
                // is only reached from AfterPreventingDeath. Fairy is the sole
                // Automatic potion in the v0.107.1 decomp.
                //
                // This enumerator had no Usage filter, so it offered a drink
                // the real game does not, and the coverage sweep read that as
                // a Rust bug (`LEGAL_ACTION|potion:FAIRY_IN_A_BOTTLE`,
                // csharp_only use_potion). Rust is right: it models
                // `PotionKind::is_automatic()`, keeps Fairy off the drink
                // surface, and fires the revive from
                // `try_fairy_in_a_bottle_prevent_death`. An ORACLE artifact,
                // not an engine divergence.
                if (potions[i].Usage == MegaCrit.Sts2.Core.Entities.Potions.PotionUsage.Automatic)
                {
                    continue;
                }
                // sts2-agent fork: list exactly what agent-mode validation accepts:
                // usable potions only, one entry per living enemy for targeted ones.
                if (_agentMode)
                {
                    if (!player.CanRemovePotions || !potions[i].PassesCustomUsabilityCheck) continue;
                    if (potions[i].TargetType == TargetType.AnyEnemy)
                    {
                        for (int t = 0; t < aliveCount; t++)
                            actions.Add(new Dictionary<string, object?>
                            {
                                ["action"] = "use_potion",
                                ["potion_index"] = i,
                                ["target_index"] = t,
                            });
                        continue;
                    }
                }
                actions.Add(new Dictionary<string, object?>
                {
                    ["action"] = "use_potion",
                    ["potion_index"] = i,
                    ["target_index"] = null,
                });
            }

            return new Dictionary<string, object?>
            {
                ["type"] = "ok",
                ["actions"] = actions,
                ["energy"] = energy,
                ["hand_size"] = hand.Count,
                ["alive_enemy_count"] = aliveCount,
            };
        }
        catch (Exception ex) { return ErrorWithTrace("LegalActions failed", ex); }
    }

    // PARITY-FUZZ (Task 3, get_state): canonical-id state snapshot for diff
    //   against the Rust port's state_to_dict_py. Differs from CombatPlayState:
    //   - monster.kind / power.kind / relic ids are canonical (.Id.Entry),
    //     NOT localized display names
    //   - monsters[] includes ALL slots (raw pcs.Enemies order). Dead slots
    //     are emitted with "alive": false, NOT filtered (probe Task 1
    //     confirmed combat-play silently drops them).
    //   - uid == raw slot index; kind is null for empty/dead slots.
    //   - relics are sorted (Ordinal); Rust normalizer (Task 5) must sort
    //     identically.
    //   - combat_terminal: true when pcs == null (out of combat) or
    //     RunManager.IsGameOver (player dead). player_won: null in combat;
    //     otherwise true iff player still alive at combat exit (null
    //     creature mirror at terminal is treated as dead → not won).
    //
    // APIs used:
    //   monster id     : EnemyCreature.Monster.Id.Entry        (MonsterModel.Id.Entry)
    //   relic id       : Relic.Id.Entry                        (RelicModel.Id.Entry)
    //   power id       : Power.Id.Entry                        (PowerModel.Id.Entry)
    //   power amount   : Power.Amount                          (int)
    //   turn counter   : CombatManager...DebugOnlyGetState().RoundNumber
    //   combat-ended   : PlayerCombatState == null  ||  RunManager.Instance.IsGameOver
    //   victory flag   : player.Creature.IsDead == false at combat exit
    public Dictionary<string, object?> GetState() =>
        _gameThread.Submit(GetStateImpl);

    /// <summary>
    /// sts2-agent fork: re-detect the current decision point without sending an action.
    /// While a selection is pending this only reads state. Otherwise it behaves like the
    /// tail of any action: it may pump combat and, after combat, generate rewards.
    /// </summary>
    public Dictionary<string, object?> GetDecision() =>
        _gameThread.Submit(() => Tracked("get_decision", DetectDecisionPoint));

    private Dictionary<string, object?> GetStateImpl()
    {
        try
        {
            if (_runState == null) return Error("No run in progress");
            var player = _runState.Players[0];
            var pcs = player.PlayerCombatState;
            var combatState = CombatManager.Instance.DebugOnlyGetState();

            bool runGameOver = false;
            try { runGameOver = RunManager.Instance.IsGameOver; }
            catch (NullReferenceException) { /* RunManager singleton not initialized yet */ }
            // sts2-rs ic-parity-tail: also treat IsOverOrEnding as terminal.
            // A killing blow flips CombatManager.IsEnding while the victory
            // fade awaits (Decimillipede DoFadeOutOnAllSegments and similar),
            // during which pcs is still non-null and trailing legal actions
            // leak out — the Rust engine is already terminal there, so the
            // parity diff saw a phantom LEGAL_ACTION / pile mismatch (seeds
            // 1486/3752). IsOverOrEnding is the same gate every C# command
            // no-ops on.
            var combatTerminal = pcs == null || runGameOver
                || (CombatManager.Instance?.IsOverOrEnding ?? false);

            // Player block: while in combat the creature mirror is authoritative;
            // out of combat fall back to 0.
            var playerPowers = player.Creature?.Powers?.Select(pw => new Dictionary<string, object?>
            {
                ["kind"] = pw.Id.Entry,
                ["amount"] = pw.Amount,
                // GH #500: Counter powers (Slow, Juggling, ...) keep their
                // scaling in DynamicVars, which `Amount` (the stack count)
                // does not show. `DisplayAmount` is the only handle the parity
                // harness has on it. Additive field - the normalized comparison
                // (`parity_fuzz_state._sort_powers`) reads only `kind` +
                // `amount`, so this never changes a verdict; it is there so an
                // hp-only drift can be attributed to a counter.
                ["display_amount"] = pw.DisplayAmount,
            }).ToList() ?? new List<Dictionary<string, object?>>();

            var playerDict = new Dictionary<string, object?>
            {
                ["hp"] = player.Creature?.CurrentHp ?? 0,
                ["max_hp"] = player.Creature?.MaxHp ?? 0,
                ["block"] = player.Creature?.Block ?? 0,
                ["energy"] = pcs?.Energy ?? 0,
                ["powers"] = playerPowers,
            };

            // Monsters: emit ALL raw slots so the index == canonical uid.
            var monsters = new List<Dictionary<string, object?>>();
            if (combatState?.Enemies != null)
            {
                for (int i = 0; i < combatState.Enemies.Count; i++)
                {
                    var e = combatState.Enemies[i];
                    if (e == null)
                    {
                        monsters.Add(new Dictionary<string, object?>
                        {
                            ["uid"] = i,
                            ["kind"] = null,
                            ["hp"] = 0,
                            ["max_hp"] = 0,
                            ["block"] = 0,
                            ["alive"] = false,
                            ["powers"] = new List<Dictionary<string, object?>>(),
                        });
                        continue;
                    }
                    var ePowers = e.Powers?.Select(pw => new Dictionary<string, object?>
                    {
                        ["kind"] = pw.Id.Entry,
                        ["amount"] = pw.Amount,
                        // GH #500 - see the playerPowers note above.
                        ["display_amount"] = pw.DisplayAmount,
                    }).ToList() ?? new List<Dictionary<string, object?>>();
                    monsters.Add(new Dictionary<string, object?>
                    {
                        ["uid"] = i,
                        ["kind"] = e.Monster?.Id.Entry,
                        ["hp"] = e.CurrentHp,
                        ["max_hp"] = e.MaxHp,
                        ["block"] = e.Block,
                        ["alive"] = e.IsAlive,
                        ["powers"] = ePowers,
                        // sts2-rs-u3wjj: the move-state id this monster is
                        // currently telegraphing (`MoveState.StateId`, e.g.
                        // "CHOMP" / "CHEW" / "MORE_TEETH" for ScrollOfBiting).
                        // The coverage sweep forwards this to the Rust
                        // `build_scenario_state` so RNG-branched starter
                        // rotations (ScrollsOfBiting{Weak,Normal}'s
                        // `StarterMoveIdx = Rng.NextInt(3)` rotation) can be
                        // force-matched without porting C#'s per-encounter RNG
                        // seeding into the synthetic scenario builder. Additive
                        // field — the normalized parity comparison
                        // (parity_fuzz_state.normalize_*) does NOT read it, so
                        // emitting it can't perturb any existing bucket.
                        ["move_state"] = e.Monster?.NextMove?.Id,
                    });
                }
            }

            var relics = player.Relics?
                .Select(r => r.Id.Entry)
                .Where(s => !string.IsNullOrEmpty(s))
                .OrderBy(s => s, StringComparer.Ordinal)
                .ToList() ?? new List<string>();

            object? playerWon = null;
            if (combatTerminal)
            {
                // Null creature mirror at terminal → treat as dead → not won.
                playerWon = player.Creature == null ? (object?)false : (object?)!player.Creature.IsDead;
            }

            return new Dictionary<string, object?>
            {
                ["type"] = "ok",
                ["player"] = playerDict,
                ["monsters"] = monsters,
                ["hand_count"] = pcs?.Hand?.Cards?.Count ?? 0,
                // sts2-rs-6rge3 diagnostics: ordered list of card ids in
                // the player's hand. Lets parity-fuzz attribute legal-
                // action drifts (target_index null vs 0) to specific
                // cards rather than only positional indices.
                ["hand_ids"] = pcs?.Hand?.Cards?.Select(c => c.Id.ToString()).ToList(),
                // Read-only diagnostic (no logic change): resolved energy cost
                // per hand card, parallel to hand_ids. Lets the parity harness
                // pin Confused / cost-reduction divergences directly (per-card
                // cost) instead of inferring from legal-action playability.
                ["hand_costs"] = pcs?.Hand?.Cards?.Select(c => c.EnergyCost?.GetResolved() ?? 0).ToList(),
                ["draw_count"] = pcs?.DrawPile?.Cards?.Count ?? 0,
                // sts2-rs-fqcx0 diagnostics: ordered card-id lists for the
                // draw/discard piles, mirroring hand_ids above. parity_fuzz
                // reads discard_ids (and draw_ids) to attribute a
                // discard_count divergence to the specific card that
                // differs, instead of only seeing the count drift. Draw
                // order is the engine's internal pile order (NOT sorted) so
                // a shuffle-RNG mismatch is visible too.
                ["draw_ids"] = pcs?.DrawPile?.Cards?.Select(c => c.Id.ToString()).ToList(),
                ["discard_count"] = pcs?.DiscardPile?.Cards?.Count ?? 0,
                ["discard_ids"] = pcs?.DiscardPile?.Cards?.Select(c => c.Id.ToString()).ToList(),
                ["exhaust_count"] = pcs?.ExhaustPile?.Cards?.Count ?? 0,
                // sts2-rs-4utkk follow-up: ordered card-id lists for the
                // exhaust + play piles, mirroring hand/draw/discard above.
                // Needed to pin pile-fate divergences (e.g. CATASTROPHE vs
                // HEMOKINESIS discard-vs-exhaust order, seed 4043) that the
                // discard_ids list alone cannot localize — a card missing from
                // discard could be in exhaust OR still in the play pile.
                ["exhaust_ids"] = pcs?.ExhaustPile?.Cards?.Select(c => c.Id.ToString()).ToList(),
                ["play_ids"] = pcs?.PlayPile?.Cards?.Select(c => c.Id.ToString()).ToList(),
                ["turn"] = combatState?.RoundNumber ?? 0,
                ["relics"] = relics,
                ["combat_terminal"] = combatTerminal,
                ["player_won"] = playerWon,
                // Per-stream RNG cursor (each Rng.Counter = number of reads
                // so far). Mirrors RunRngSet.ToSerializable().Counters; keyed
                // by RunRngType snake_case to line up with the Rust dump's
                // rng_counters. Diagnostic only: in a real-vs-real replay a
                // counter mismatch pinpoints which stream desynced and by how
                // many reads — visible even when a value diff is masked by a
                // constant (e.g. a fixed-HP monster). See sts2-rs parity docs.
                ["rng_counters"] = RngCounters(),
                // Defect orb belt (parity surface, bead orb-parity). `orb_slots`
                // is the belt capacity; `orbs` is the ordered list of channelled
                // (non-empty) orbs with their Focus-folded passive/evoke values
                // (OrbModel.PassiveVal / EvokeVal). The parity normalizer
                // compares kind + passive (channel order matters); both are
                // null/empty for non-Defect states, so this is additive.
                ["orb_slots"] = pcs?.OrbQueue?.Capacity ?? 0,
                ["orbs"] = pcs?.OrbQueue?.Orbs?.Select(o => new Dictionary<string, object?>
                {
                    ["type"] = o.GetType().Name.Replace("Orb", ""),
                    ["passive"] = (int)o.PassiveVal,
                    ["evoke"] = (int)o.EvokeVal,
                }).ToList(),
                // Necrobinder Osty pet (parity surface). The companion's MaxHp
                // IS the mechanic (Summon grows it; OstyAttacks scale on it), so
                // expose it the same way GetStateImpl does for the workbench. A
                // live pet emits hp/max_hp/block/alive; a Necrobinder with no
                // live pet emits {alive:false}; other characters omit the key.
                // The parity normalizer (`_canon_osty`) collapses null/dead to
                // None on both sides and compares hp/max_hp/block when alive.
                ["osty"] = player.Osty is { } osty
                    ? new Dictionary<string, object?>
                    {
                        ["hp"] = osty.CurrentHp,
                        ["max_hp"] = osty.MaxHp,
                        ["block"] = osty.Block,
                        ["alive"] = osty.IsAlive,
                    }
                    : (player.Character?.Id.Entry == "NECROBINDER"
                        ? new Dictionary<string, object?> { ["alive"] = false }
                        : null),
            };
        }
        catch (Exception ex) { return ErrorWithTrace("GetState failed", ex); }
    }

    // Per-stream RNG read-counts, keyed to match the Rust dump's
    // rng_counters (RunRngType snake_case). Each RunRngSet stream is a
    // public Rng with a public Counter; no reflection needed. Returns an
    // empty dict if no run is active.
    private Dictionary<string, object?> RngCounters()
    {
        var d = new Dictionary<string, object?>();
        var rng = _runState?.Rng;
        if (rng == null) return d;
        try
        {
            d["up_front"] = rng.UpFront.Counter;
            d["shuffle"] = rng.Shuffle.Counter;
            d["combat_card_generation"] = rng.CombatCardGeneration.Counter;
            d["combat_potion_generation"] = rng.CombatPotionGeneration.Counter;
            d["combat_card_selection"] = rng.CombatCardSelection.Counter;
            d["combat_energy_costs"] = rng.CombatEnergyCosts.Counter;
            d["combat_targets"] = rng.CombatTargets.Counter;
            d["monster_ai"] = rng.MonsterAi.Counter;
            d["niche"] = rng.Niche.Counter;
            d["combat_orbs"] = rng.CombatOrbGeneration.Counter;
            // PlayerRng streams (diagnostic): rewards/shops/transformations cursors.
            var prng = (_runState?.Players != null && _runState.Players.Count > 0)
                ? _runState.Players[0].PlayerRng
                : null;
            if (prng != null)
            {
                d["rewards"] = prng.Rewards.Counter;
                d["shops"] = prng.Shops.Counter;
                d["transformations"] = prng.Transformations.Counter;
            }
        }
        catch { /* best-effort diagnostic */ }
        return d;
    }

    /// <summary>
    /// The mirror's `$.rng` block: `run_seed` and `counters`, together or not at
    /// all.
    ///
    /// <see cref="RngCounters"/> is best-effort by design — it also feeds a
    /// purely diagnostic field where a partial answer beats none. This consumer
    /// cannot tolerate that: `combat_mirror.reconcile` reads the presence of
    /// `run_seed` alone as "the world owns the dice" and stops carrying its own
    /// valid cursors, and a `state_from_dict` handed a seed with no counters
    /// rebuilds every stream at count 0. So seed-without-counters does not
    /// degrade to the pre-field behaviour — it destroys lineage that was intact.
    /// Emit the pair or emit an empty block, which every consumer already reads
    /// as "this producer does not carry the run's dice".
    /// </summary>
    private Dictionary<string, object?> MirrorRngBlock()
    {
        var d = new Dictionary<string, object?>();
        var seed = _runState?.Rng?.Seed;
        if (seed == null) return d;
        var counters = RngCounters();
        if (counters.Count == 0) return d;
        d["run_seed"] = seed;
        d["counters"] = counters;
        return d;
    }

    // ─── Game actions ───
    public Dictionary<string, object?> LoadSave(string saveJson) =>
        _gameThread.Submit(() => LoadSaveImpl(saveJson));

    private Dictionary<string, object?> LoadSaveImpl(string saveJson)
    {
        try
        {
            EnsureModelDbInitialized();

            Log("Loading save file...");

            if (!ValidateSaveSchemaVersion(saveJson, out var schemaError))
                return Error($"Save schema mismatch: {schemaError}");

            var readResult = SaveManager.FromJson<SerializableRun>(saveJson);
            if (!readResult.Success || readResult.SaveData == null)
                return Error($"Failed to parse save file: {readResult.Status} {readResult.ErrorMessage}");

            var save = readResult.SaveData;
            Log($"Save loaded: seed={save.SerializableRng?.Seed}, act={save.CurrentActIndex}, ascension={save.Ascension}");

            // Tear down any prior run before loading. RunManager / LocalContext both
            // hold singleton state ("State is already set" otherwise), and a stuck
            // ActionExecutor would block the new load.
            if (RunManager.Instance.IsInProgress || _runState != null)
            {
                Log("Cleaning up previous run before loading save");
                try { RunManager.Instance.ActionExecutor.Cancel(); } catch { }
                try { if (RunManager.Instance.IsInProgress) RunManager.Instance.CleanUp(graceful: true); } catch (Exception ex) { LogException("CleanUp", ex); }
                ClearCardSelector();
                // A task stashed by the OUTGOING run can never be driven by the
                // incoming one, and leaving it non-null would carry the scoped NGame
                // stub across the reload boundary into a run that is not in an event.
                ReleaseStrandedEventOptionTask("run torn down for save reload");
                _runState = null;
                _syncCtx.Pump();
            }

            _runState = RunState.FromSerializable(save);
            if (_runState == null)
                return Error("Failed to create RunState from save");

            Log($"RunState created, players={_runState.Players?.Count}");

            var netService = new NetSingleplayerGameService();
            RunManager.Instance.SetUpSavedSingleplayer(_runState, save);
            LocalContext.NetId = netService.NetId;

            CombatManager.Instance.TurnStarted += _ => _turnStarted.Set();
            CombatManager.Instance.CombatEnded += _ => _combatEnded.Set();
            CardSelectCmd.UseSelector(_cardSelector);
            LocPatches._bundleSimRef = this;

            var savedRoom = _runState.CurrentRoom;

            // Save visited coords before Launch (EnterAct will clear them)
            var savedVisitedCoords = _runState.VisitedMapCoords?.ToList() ?? new List<MapCoord>();
            var shouldResumeInitialNeow = IsInitialNeowSave(saveJson);
            Log($"Save has {savedVisitedCoords.Count} visited coords");

            RunManager.Instance.Launch();
            Log("Run launched");

            if (savedRoom is MapRoom || savedRoom == null)
            {
                // Preserve Neow for saves created before the first blessing choice.
                // Once the run has visited at least one map node, re-entering Act 1
                // should not send the player back through the Ancient start node.
                if (_runState.CurrentActIndex == 0 && savedVisitedCoords.Count > 0)
                    _runState.ExtraFields.StartedWithNeow = false;
                RunManager.Instance.EnterAct(_runState.CurrentActIndex, doTransition: false).GetAwaiter().GetResult();
                _syncCtx.Pump();
                Log($"Entered Act {_runState.CurrentActIndex}");

                if (shouldResumeInitialNeow && _runState.Map?.StartingMapPoint != null)
                {
                    Log("Restoring initial Neow event");
                    RunManager.Instance.EnterMapCoord(_runState.Map.StartingMapPoint.coord).GetAwaiter().GetResult();
                    _syncCtx.Pump();
                }

                // EnterAct clears visited coords and ActFloor — restore them from save
                if (savedVisitedCoords.Count > 0)
                {
                    if (_runState.VisitedMapCoords == null || _runState.VisitedMapCoords.Count == 0)
                    {
                        foreach (var coord in savedVisitedCoords)
                            _runState.AddVisitedMapCoord(coord);
                    }
                    _runState.ActFloor = savedVisitedCoords.Count;
                    var last = savedVisitedCoords[^1];
                    Log($"Restored map position: floor={_runState.ActFloor}, coord=({last.col},{last.row})");
                }
            }
            else
            {
                Log($"Preserving saved room: {savedRoom.GetType().Name}");
            }

            return DetectDecisionPoint();
        }
        catch (Exception ex)
        {
            return ErrorWithTrace("LoadSave failed", ex);
        }
    }

    private static bool ValidateSaveSchemaVersion(string saveJson, out string error)
    {
        error = "";
        try
        {
            using var doc = System.Text.Json.JsonDocument.Parse(saveJson);
            var root = doc.RootElement;

            if (!root.TryGetProperty("schema_version", out var versionElem))
            {
                error = "missing schema_version";
                return false;
            }

            if (versionElem.ValueKind != System.Text.Json.JsonValueKind.Number ||
                !versionElem.TryGetInt32(out var schemaVersion))
            {
                error = "schema_version is not a valid integer";
                return false;
            }

            if (schemaVersion != CurrentSaveSchemaVersion)
            {
                error = $"expected v{CurrentSaveSchemaVersion}, got v{schemaVersion}";
                return false;
            }

            return true;
        }
        catch (Exception ex)
        {
            error = $"could not inspect save: {ex.Message}";
            return false;
        }
    }

    private static bool TrySetPropertyValue(object target, string propertyName, object? value)
    {
        var prop = target.GetType().GetProperty(propertyName);
        if (prop?.CanWrite != true)
            return false;
        prop.SetValue(target, value);
        return true;
    }

    private static bool IsInitialNeowSave(string saveJson)
    {
        try
        {
            using var doc = System.Text.Json.JsonDocument.Parse(saveJson);
            var root = doc.RootElement;

            if (!root.TryGetProperty("current_act_index", out var actIndexElem) || actIndexElem.GetInt32() != 0)
                return false;

            var hasVisitedCoords = root.TryGetProperty("visited_map_coords", out var visitedElem)
                                && visitedElem.ValueKind == System.Text.Json.JsonValueKind.Array
                                && visitedElem.GetArrayLength() > 0;
            if (hasVisitedCoords)
                return false;

            return root.TryGetProperty("extra_fields", out var extraFieldsElem)
                && extraFieldsElem.ValueKind == System.Text.Json.JsonValueKind.Object
                && extraFieldsElem.TryGetProperty("started_with_neow", out var startedElem)
                && startedElem.ValueKind == System.Text.Json.JsonValueKind.True;
        }
        catch
        {
            return false;
        }
    }

    private static bool TryRollbackSerializedSaveToPreRoom(SerializableRun serializableRun, out string error)
    {
        error = "";

        var saveType = serializableRun.GetType();
        var visitedProp = saveType.GetProperty("VisitedMapCoords");
        if (visitedProp == null)
        {
            error = "Save data is missing VisitedMapCoords";
            return false;
        }

        var visitedValue = visitedProp.GetValue(serializableRun);
        var visitedItems = new List<object?>();
        if (visitedValue is System.Collections.IEnumerable visitedEnumerable)
        {
            foreach (var item in visitedEnumerable)
                visitedItems.Add(item);
        }

        if (visitedItems.Count == 0)
        {
            error = "Cannot roll back save before the first room";
            return false;
        }

        visitedItems.RemoveAt(visitedItems.Count - 1);

        var visitedType = visitedProp.PropertyType;
        if (visitedType.IsArray)
        {
            var elementType = visitedType.GetElementType()!;
            var array = Array.CreateInstance(elementType, visitedItems.Count);
            for (int i = 0; i < visitedItems.Count; i++)
                array.SetValue(visitedItems[i], i);
            visitedProp.SetValue(serializableRun, array);
        }
        else if (visitedType.IsGenericType)
        {
            var elementType = visitedType.GetGenericArguments()[0];
            var listType = typeof(List<>).MakeGenericType(elementType);
            var list = (System.Collections.IList)Activator.CreateInstance(listType)!;
            foreach (var item in visitedItems)
                list.Add(item);
            visitedProp.SetValue(serializableRun, list);
        }
        else
        {
            error = $"Unsupported VisitedMapCoords type: {visitedType.Name}";
            return false;
        }

        TrySetPropertyValue(serializableRun, "ActFloor", visitedItems.Count);
        TrySetPropertyValue(serializableRun, "CurrentMapCoord", visitedItems.Count > 0 ? visitedItems[^1] : null);
        TrySetPropertyValue(serializableRun, "PreFinishedRoom", null);
        TrySetPropertyValue(serializableRun, "CurrentRoom", null);
        return true;
    }

    public Dictionary<string, object?> SaveCheckpoint(string? outputPath) =>
        _gameThread.Submit(() => SaveCheckpointImpl(outputPath));

    private Dictionary<string, object?> SaveCheckpointImpl(string? outputPath)
    {
        try
        {
            if (_runState == null)
                return Error("No active run to save");

            if (string.IsNullOrEmpty(outputPath))
                return Error("No output path specified for quit save");

            var currentRoom = _runState.CurrentRoom;
            SerializableRun serializableRun;

            if (currentRoom is MapRoom || currentRoom == null)
            {
                Log($"Saving map checkpoint (room={currentRoom?.GetType().Name ?? "null"}, outputPath={outputPath})...");
                serializableRun = RunManager.Instance.ToSave(currentRoom);
            }
            else
            {
                Log($"Saving pre-room checkpoint from {currentRoom.GetType().Name} (outputPath={outputPath})...");
                serializableRun = RunManager.Instance.ToSave(new MapRoom());
                if (!TryRollbackSerializedSaveToPreRoom(serializableRun, out var rollbackError))
                    return Error($"Cannot save checkpoint: {rollbackError}");
            }

            var saveJson = SaveManager.ToJson(serializableRun);
            Log($"Serialized save: {saveJson.Length} chars");

            var dir = System.IO.Path.GetDirectoryName(outputPath);
            if (!string.IsNullOrEmpty(dir) && !System.IO.Directory.Exists(dir))
                System.IO.Directory.CreateDirectory(dir);
            System.IO.File.WriteAllText(outputPath, saveJson);
            Log($"Save written to: {outputPath}");

            return new Dictionary<string, object?>
            {
                ["type"] = "save_result",
                ["success"] = true,
                ["path"] = outputPath,
                ["size"] = saveJson.Length,
                ["room_type"] = currentRoom?.GetType().Name,
            };
        }
        catch (Exception ex)
        {
            return ErrorWithTrace("SaveCheckpoint failed", ex);
        }
    }
    public Dictionary<string, object?> ExecuteAction(string action, Dictionary<string, object?>? args) =>
        _gameThread.Submit(() => Tracked(action, () => ExecuteActionImpl(action, args)));

    private Dictionary<string, object?> ExecuteActionImpl(string action, Dictionary<string, object?>? args)
    {
        try
        {
            if (_runState == null)
                return _agentMode ? Reject("no_run", "No run in progress") : Error("No run in progress");

            var player = _runState.Players[0];

            // sts2-agent fork: check the action against the decision point before
            // anything executes (RunSimulator.Agent.cs).
            if (_agentMode && ValidateAgentAction(action, args, player) is { } rejected)
                return rejected;

            // Reconcile the scoped NGame stub at every dispatch, so its lifetime is an
            // asserted invariant rather than an inference from a field with leak paths.
            // A stashed task with nothing pending to resume it is stranded by
            // definition (see ReleaseStrandedEventOptionTask); anything else just
            // re-asserts `EventOptionInFlight == (task != null)`.
            if (!AnySelectionPending())
            {
                ReleaseStrandedEventOptionTask($"no selection pending at '{action}'");
                ReleaseStrandedRewardClaimTask($"no selection pending at '{action}'");
            }

            switch (action)
            {
                case "select_map_node":
                    return DoMapSelect(player, args);
                case "play_card":
                    return DoPlayCard(player, args);
                case "end_turn":
                    return DoEndTurn(player);
                case "choose_option":
                    return DoChooseOption(player, args);
                case "select_card_reward":
                    return DoSelectCardReward(player, args);
                case "skip_card_reward":
                    return DoSkipCardReward(player);
                // sts2-agent fork: purchases run like rest-site options (RunSimulator.Shop.cs).
                case "buy_card" or "buy_relic" or "buy_potion" or "remove_card" when _agentMode:
                    return DoAgentBuy(action, args);
                case "buy_card":
                    return DoBuyCard(player, args);
                case "buy_relic":
                    return DoBuyRelic(player, args);
                case "buy_potion":
                    return DoBuyPotion(player, args);
                case "remove_card":
                    return DoRemoveCard(player);
                case "select_bundle":
                    return DoSelectBundle(player, args);
                case "select_cards":
                    return DoSelectCards(player, args);
                case "skip_select":
                    return DoSkipSelect(player);
                // sts2-agent fork: the deck screen's Close button and the choose-a-card
                // screen's Skip button; both answer the selection with no card, as
                // skip_select does, but each only where the game shows that button.
                case "cancel_select" when _agentMode:
                    return DoSkipSelect(player, "Cancelling card selection");
                case "skip_card_choice" when _agentMode:
                    return DoSkipSelect(player, "Skipping the card choice");
                case "use_potion":
                    return DoUsePotion(player, args);
                case "discard_potion":
                    return DoDiscardPotion(player, args);
                case "leave_room":
                    return DoLeaveRoom(player);
                case "proceed":
                    // sts2-agent fork: leaving the rewards screen (RunSimulator.Rewards.cs).
                    if (_agentMode && _lastDecision?.GetValueOrDefault("decision") as string == "rewards")
                        return ProceedFromAgentRewards();
                    // sts2-agent fork: leaving the rest site or the treasure room (RunSimulator.RestTreasure.cs).
                    if (_agentMode && _lastDecision?.GetValueOrDefault("decision") as string == "rest_site")
                        return ProceedFromRestSite();
                    if (_agentMode && _lastDecision?.GetValueOrDefault("decision") as string == "treasure")
                        return ProceedFromTreasure();
                    if (_agentMode && _lastDecision?.GetValueOrDefault("decision") as string == "shop")
                        return ProceedFromShop();
                    return DoProceed(player);
                case "continue" when _agentMode:
                    return DoParasolContinue();
                case "divine" when _agentMode:
                    return DoDivine(args);
                case "take_relic" when _agentMode:
                    return DoTakeRelic(player, args);
                case "claim_reward" when _agentMode:
                    return DoClaimReward(player, args);
                case "choose_card_reward_alternative" when _agentMode:
                    return DoChooseCardRewardAlternative(args);
                default:
                    return _agentMode ? Reject("unknown_action", $"Unknown action: {action}") : Error($"Unknown action: {action}");
            }
        }
        catch (Exception ex)
        {
            // sts2-agent fork: an exception escaping an action can come from anywhere
            // in it, so whether it changed state is unknown.
            if (_agentMode)
            {
                var inner = ex;
                while (inner.InnerException != null) inner = inner.InnerException;
                return Failure("error", "unknown", $"Action '{action}' failed: {inner.GetType().Name}: {inner.Message}",
                    null, "host_exception", new() { ["stack_trace"] = inner.StackTrace });
            }
            return ErrorWithTrace($"Action '{action}' failed", ex);
        }
    }

    #region Actions

    private Dictionary<string, object?> DoMapSelect(Player player, Dictionary<string, object?>? args)
    {
        if (args == null || !args.ContainsKey("col") || !args.ContainsKey("row"))
            return Error("select_map_node requires 'col' and 'row'");

        // Reset tracking for new room
        _rewardsProcessed = false;
        ResetAgentRewards();
        ResetAgentRestTreasure();
        _pendingCardReward = null;
        _eventOptionChosen = false;
        ResetEventAdvanceTracking();
        _pendingRewards = null;
        _lastKnownHp = player.Creature?.CurrentHp ?? 0;

        var col = Convert.ToInt32(args["col"]);
        var row = Convert.ToInt32(args["row"]);
        var coord = new MapCoord((byte)col, (byte)row);

        // Winged Boots: the GAME counts off-path travel itself —
        // WingedBoots.AfterRoomEntered compares the entered room against the
        // previous node's children and bumps TimesUsed, whose setter writes
        // the "Rooms" dynamic var (3 - TimesUsed). EnterMapCoord fires that
        // hook, so an extra local counter here DOUBLE-counted every off-path
        // move and cut free travel off after 2 uses instead of the game's 3
        // (2026-07-18 shadow-replay census, seeds 1017/1038/1057). Log only.
        if (WingedBootsMaxUses() > 0 && IsOffPathChoice(col, row))
        {
            Log($"WINGED_BOOTS: off-path move to ({col},{row}); {WingedBootsMaxUses()} uses left before entry");
        }

        Log($"Moving to map coord ({col},{row})");

        // BUG-013: Wait for any pending actions (relic sessions, etc.) to complete before entering new room
        WaitForActionExecutor();
        _syncCtx.Pump();

        // Call EnterMapCoord directly (same as what MoveToMapCoordAction does in TestMode)
        // This avoids the action executor which can swallow errors silently.
        RunManager.Instance.EnterMapCoord(coord).GetAwaiter().GetResult();
        _syncCtx.Pump();
        WaitForActionExecutor();

        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoPlayCard(Player player, Dictionary<string, object?>? args)
    {
        if (args == null || !args.ContainsKey("card_index"))
            return Error("play_card requires 'card_index'");

        var cardIndex = Convert.ToInt32(args["card_index"]);
        var pcs = player.PlayerCombatState;
        if (pcs == null)
            return Error("Not in combat");

        var hand = pcs.Hand.Cards;
        if (cardIndex < 0 || cardIndex >= hand.Count)
            return Error($"Invalid card index {cardIndex}, hand has {hand.Count} cards");

        var card = hand[cardIndex];

        // Determine target based on card's TargetType first
        // Self/None/All cards: target = null (game handles internally)
        // AnyEnemy cards: use target_index or auto-pick first alive enemy
        Creature? target = null;
        var cardTargetType = card.TargetType;
        if (cardTargetType == TargetType.AnyEnemy)
        {
            // Use caller's target_index if provided
            if (args.TryGetValue("target_index", out var targetObj) && targetObj != null)
            {
                var targetIndex = Convert.ToInt32(targetObj);
                var state = CombatManager.Instance.DebugOnlyGetState();
                if (state != null)
                {
                    var enemies = state.Enemies.Where(e => e != null && e.IsAlive).ToList();
                    if (targetIndex >= 0 && targetIndex < enemies.Count)
                        target = enemies[targetIndex];
                }
            }
            // Fallback: auto-target first alive enemy
            if (target == null)
            {
                var state = CombatManager.Instance.DebugOnlyGetState();
                target = state?.Enemies?.FirstOrDefault(e => e != null && e.IsAlive);
            }
        }
        // All other target types (None, All, etc.) → leave target as null

        // Check if card can be played
        if (!card.CanPlay(out var reason, out var _))
        {
            return Error($"Cannot play card {card.GetType().Name}: {reason}");
        }

        Log($"Playing card {card.GetType().Name} (index {cardIndex}) targeting {(target != null ? target.Monster?.GetType().Name ?? "creature" : "none")}");

        var handCountBefore = hand.Count;
        var playsBefore = CombatManager.Instance.History.CardPlaysStarted.Count();

        var playAction = new PlayCardAction(card, target);
        RunManager.Instance.ActionQueueSet.EnqueueWithoutSynchronizing(playAction);
        WaitForActionExecutor();

        // Check if card play had no effect (hand unchanged, same card still at
        // same index). A return-to-hand card (e.g. ParticleWall — its result
        // pile override redirects Discard->Hand) lands back at the SAME hand
        // index with the hand count unchanged, so the hand-state check alone
        // false-flags a fully-successful play as "could not be played". Also
        // require that NO CardPlayStarted was recorded: if the play actually
        // executed, CombatManager.History.CardPlaysStarted grew by one.
        var handAfter = pcs.Hand.Cards;
        var playsAfter = CombatManager.Instance.History.CardPlaysStarted.Count();
        if (playsAfter == playsBefore && handAfter.Count == handCountBefore && cardIndex < handAfter.Count && handAfter[cardIndex] == card)
        {
            // sts2-agent fork: the play was queued but never registered as started;
            // whether it changed anything (energy, triggers) is not known.
            if (_agentMode)
                return Failure("error", "unknown",
                    $"Card could not be played (still in hand after action): {card.GetType().Name} [{card.Id}]",
                    "the card play to start", "play_not_executed");
            return Error($"Card could not be played (still in hand after action): {card.GetType().Name} [{card.Id}]");
        }

        return DetectDecisionPoint();
    }

    // v0.107.1: the global CombatManager.IsPlayPhase bool was removed. The play
    // phase is now per-player on PlayerCombatState.Phase (== PlayerTurnPhase.Play).
    // Singleplayer headless always has exactly one player (_runState.Players[0]).
    private bool IsPlayPhase()
    {
        if (_runState?.Players == null || _runState.Players.Count == 0) return false;
        var pcs = _runState.Players[0].PlayerCombatState;
        return pcs != null && pcs.Phase == MegaCrit.Sts2.Core.Combat.PlayerTurnPhase.Play;
    }

    private Dictionary<string, object?> DoEndTurn(Player player)
    {
        if (!IsPlayPhase())
        {
            // Might be between phases — pump and check
            _syncCtx.Pump();
            if (!IsPlayPhase())
            {
                if (!CombatManager.Instance.IsInProgress || player.Creature.IsDead)
                    return DetectDecisionPoint();
                // Brief wait for ThreadPool if sync context didn't catch it
                Thread.Sleep(100);
                _syncCtx.Pump();
                if (!IsPlayPhase())
                    return DetectDecisionPoint();
            }
        }

        // Ensure no actions are still running before ending turn
        WaitForActionExecutor();

        Log($"Ending turn (round={CombatManager.Instance.DebugOnlyGetState()?.RoundNumber ?? 0})");
        _turnStarted.Reset();
        _combatEnded.Reset();

        // Enable SuppressYield so Task.Yield() runs inline during enemy turn processing.
        // This prevents deadlocks during boss fights (e.g., Vantom) where continuations
        // would otherwise be posted to ThreadPool and never complete.
        YieldPatches.SuppressYield = true;
        try
        {
            PlayerCmd.EndTurn(player, canBackOut: false);
            _syncCtx.Pump();
        }
        finally
        {
            YieldPatches.SuppressYield = false;
        }

        // Fallback: if turn didn't complete synchronously, wait for the enemy turn
        // to actually finish before declaring "stuck." A blind 250ms timeout was
        // wrong: legitimate enemy turns on multi-enemy boss fights (e.g. Kin Priest +
        // 2 Kin Followers, each with intents and Minion bookkeeping) can take longer.
        // Cancelling mid-flight corrupts the play-phase transition — _turnStarted
        // never fires, the new turn never begins, and subsequent end_turn calls are
        // no-ops because IsPlayPhase is false (early-return above).
        //
        // "Stuck" = ActionExecutor has been idle for a streak AND IsPlayPhase is
        // still false. While IsRunning is true, the game is doing real work — wait.
        //
        // CRITICAL (sts2-rs TOOLS_OF_THE_TRADE parity): a pending card selection
        // is NOT a deadlock — it is a legitimate player decision raised mid-EndTurn
        // by a start-of-turn power. Tools of the Trade's AfterPlayerTurnStart draws
        // +1 and opens a FromHandForDiscard choice at the *next* turn's start, which
        // PlayerCmd.EndTurn drives into before the play phase opens. That pauses the
        // turn-start sequence (a WaitUntilQueue TCS) with IsPlayPhase still false and
        // the player alive — exactly the "stuck" shape below. The old code then
        // cancelled and re-ran PlayerCmd.EndTurn, which executed the enemy turn a
        // SECOND time, double-applying enemy damage (e.g. the seed-82 SlitheringStrangler
        // arena: a survivable 6-dmg Constrict tick became a lethal ~25 after the
        // re-run added a phantom CLUMP_SHOT + attack). Guard every stuck branch with
        // !HasPending so a pending selection falls through to DetectDecisionPoint(),
        // which surfaces the card_select the caller resolves normally.
        if (CombatManager.Instance.IsInProgress && !IsPlayPhase()
            && !player.Creature.IsDead && !_cardSelector.HasPending)
        {
            var executor = RunManager.Instance.ActionExecutor;
            int idleStreak = 0;
            // ~500ms truly idle ⇒ real deadlock. The previous 50ms streak
            // false-fired under parallel-recording load (thread starvation
            // makes the executor LOOK idle mid-turn with no pending
            // selection), and the cancel+retry then double-ran the enemy
            // turn — the trace-corruption class behind ~20 sweep seeds
            // (famD/famF triage 2026-06-09). A real WaitUntilQueue deadlock
            // stays deadlocked forever, so waiting 10x longer costs only
            // latency on the (rare) genuine-deadlock path.
            const int idleStreakRequired = 500;
            const int hardCapIters = 10000;    // upper bound: 10s
            for (int i = 0; i < hardCapIters; i++)
            {
                _syncCtx.Pump();
                if (_turnStarted.IsSet || _combatEnded.IsSet) break;
                if (!CombatManager.Instance.IsInProgress || player.Creature.IsDead) break;
                if (IsPlayPhase()) break;
                if (_cardSelector.HasPending) break;  // pending decision, not a deadlock
                if (executor.IsRunning) idleStreak = 0;
                else if (++idleStreak >= idleStreakRequired) break;
                Thread.Sleep(1);
            }

            // sts2-agent fork: in agent mode never cancel and re-run the turn (the
            // re-run can execute the enemy turn twice); report the stuck turn instead.
            if (_agentMode && CombatManager.Instance.IsInProgress && !IsPlayPhase()
                && !player.Creature.IsDead && !_cardSelector.HasPending)
            {
                Log("EndTurn stuck (agent mode): reporting a failure instead of cancel + re-run");
                return Failure("timeout", "started",
                    "end_turn did not reach the next player turn or a choice; upstream would cancel and re-run it",
                    "next player turn", "end_turn_stuck");
            }

            // If STILL stuck, the WaitUntilQueue TCS is likely deadlocked.
            // Cancel the ActionExecutor to break out, then re-trigger EndTurn.
            if (CombatManager.Instance.IsInProgress && !IsPlayPhase()
                && !player.Creature.IsDead && !_cardSelector.HasPending)
            {
                Log("EndTurn stuck, cancelling and retrying with SuppressYield...");
                try
                {
                    RunManager.Instance.ActionExecutor.Cancel();
                    _syncCtx.Pump();
                    Thread.Sleep(50);
                    _syncCtx.Pump();

                    // Reset the player ready state and try again with SuppressYield
                    CombatManager.Instance.UndoReadyToEndTurn(player);
                    _syncCtx.Pump();

                    // LAST-INSTANT re-check: the Cancel/Undo pumps above can
                    // surface a card selection that was opening when the idle
                    // loop broke (a relic auto-play window, e.g. a
                    // HistoryCourse dupe of a selection-opening Skill at the
                    // next turn's start — seed 5056's corruption class). A
                    // pending selection must NEVER reach the EndTurn re-run,
                    // which would double-execute the enemy turn; fall through
                    // to DetectDecisionPoint() instead.
                    if (_cardSelector.HasPending)
                    {
                        Log("EndTurn stuck-retry aborted: selection surfaced after cancel");
                    }
                    else
                    {
                        YieldPatches.SuppressYield = true;
                        try
                        {
                            PlayerCmd.EndTurn(player, canBackOut: false);
                            _syncCtx.Pump();
                        }
                        finally
                        {
                            YieldPatches.SuppressYield = false;
                        }
                    }

                    for (int i = 0; i < 100; i++)
                    {
                        _syncCtx.Pump();
                        if (_turnStarted.IsSet || _combatEnded.IsSet) break;
                        if (!CombatManager.Instance.IsInProgress || player.Creature.IsDead) break;
                        if (IsPlayPhase()) break;
                        Thread.Sleep(10);
                    }
                }
                catch (Exception ex) { LogException("Cancel retry", ex); }
            }

            // NUCLEAR OPTION: If STILL stuck after 2 attempts, use ThreadPool to force
            // the enemy turn processing to complete with SuppressYield permanently on.
            // Still guarded by !HasPending (see the top-level comment): a pending
            // card selection must never reach the nuclear re-run, which would
            // double-execute the enemy turn.
            if (CombatManager.Instance.IsInProgress && !IsPlayPhase()
                && !player.Creature.IsDead && !_cardSelector.HasPending)
            {
                var stuckState = CombatManager.Instance.DebugOnlyGetState();
                var stuckEnemies = stuckState?.Enemies?.Where(e => e != null && e.IsAlive)
                    .Select(e => $"{e.Monster?.GetType().Name}(hp={e.CurrentHp})").ToList();
                Log($"EndTurn STILL stuck after retry — nuclear fallback. Round={stuckState?.RoundNumber}, " +
                    $"Enemies=[{string.Join(",", stuckEnemies ?? new())}], " +
                    $"IsPlayPhase={IsPlayPhase()}, " +
                    $"IsInProgress={CombatManager.Instance.IsInProgress}, " +
                    $"ActionExecutor.IsRunning={RunManager.Instance.ActionExecutor.IsRunning}");
                try
                {
                    // Cancel again and undo
                    RunManager.Instance.ActionExecutor.Cancel();
                    _syncCtx.Pump();
                    CombatManager.Instance.UndoReadyToEndTurn(player);
                    _syncCtx.Pump();
                    Thread.Sleep(50);

                    // Run EndTurn on ThreadPool with SuppressYield permanently on
                    YieldPatches.SuppressYield = true;
                    var endTurnTask = Task.Run(() =>
                    {
                        PlayerCmd.EndTurn(player, canBackOut: false);
                    });

                    // Aggressively pump sync context while waiting (up to 5 seconds)
                    for (int i = 0; i < 500; i++)
                    {
                        _syncCtx.Pump();
                        if (endTurnTask.IsCompleted) break;
                        if (_turnStarted.IsSet || _combatEnded.IsSet) break;
                        if (!CombatManager.Instance.IsInProgress || player.Creature.IsDead) break;
                        if (IsPlayPhase()) break;
                        Thread.Sleep(10);
                    }
                    YieldPatches.SuppressYield = false;

                    // If still not play phase, try just waiting a bit more
                    if (CombatManager.Instance.IsInProgress && !IsPlayPhase() && !player.Creature.IsDead)
                    {
                        for (int i = 0; i < 200; i++)
                        {
                            _syncCtx.Pump();
                            Thread.Sleep(10);
                            if (IsPlayPhase() || !CombatManager.Instance.IsInProgress || player.Creature.IsDead)
                                break;
                        }
                    }

                    if (IsPlayPhase())
                        Log("Nuclear fallback SUCCEEDED — play phase resumed");
                    else
                        Log("Nuclear fallback FAILED — returning stuck state");
                }
                catch (Exception ex)
                {
                    LogException("Nuclear fallback error", ex);
                    YieldPatches.SuppressYield = false;
                }
            }
        }

        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoSelectCardReward(Player player, Dictionary<string, object?>? args)
    {
        // Handle event-triggered card reward (blocking GetSelectedCardReward)
        if (_cardSelector.HasPendingReward)
        {
            if (args == null || !args.ContainsKey("card_index"))
                return Error("select_card_reward requires 'card_index'");
            var idx = Convert.ToInt32(args["card_index"]);
            Log($"Resolving event card reward: index {idx}");
            _cardSelector.ResolveReward(idx);
            _syncCtx.Pump();
            WaitForActionExecutor();

            // Same continuation-drain as DoSelectCards: GetSelectedCardReward
            // unblocks INSIDE the stashed event-option task (e.g. Neow's
            // LostCoffer AfterObtained), whose post-await CardPileCmd.Add is
            // what actually puts the chosen card in the deck. Without driving
            // the task the pick resolved but the deck never gained the card
            // (run-parity RP0: cs deck 10 vs rs 11 across all characters).
            DrainEventOptionTaskIfDue();
            return DetectDecisionPoint();
        }

        if (_pendingCardReward == null)
            return Error("No pending card reward");
        if (args == null || !args.ContainsKey("card_index"))
            return Error("select_card_reward requires 'card_index'");

        var cardIndex = Convert.ToInt32(args["card_index"]);
        var cards = _pendingCardReward.Cards.ToList();
        if (cardIndex < 0 || cardIndex >= cards.Count)
            return Error($"Invalid card index {cardIndex}, {cards.Count} cards available");

        var card = cards[cardIndex];
        Log($"Selected card reward: {card.GetType().Name}");

        // Add card to deck
        try
        {
            MegaCrit.Sts2.Core.Commands.CardPileCmd
                .Add(card, MegaCrit.Sts2.Core.Entities.Cards.PileType.Deck)
                .GetAwaiter().GetResult();
            _syncCtx.Pump();
            RunManager.Instance.RewardSynchronizer.SyncLocalObtainedCard(card);
        }
        catch (Exception ex) { LogException("Add card to deck", ex); }

        _pendingCardReward = null;
        // Check if more rewards pending
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoSkipCardReward(Player player)
    {
        if (_cardSelector.HasPendingReward)
        {
            Log("Skipping event card reward");
            // sts2-agent fork: skip is the game's Skip alternative (validated as offered).
            if (_agentMode) _cardSelector.ResolveAlternative("Skip");
            else _cardSelector.SkipReward();
            _syncCtx.Pump();
            WaitForActionExecutor();

            // Same continuation-drain as DoSelectCardReward: the skipped
            // GetSelectedCardReward unblocks inside the stashed event-option
            // task whose post-await effects (e.g. follow-up grants) must run.
            DrainEventOptionTaskIfDue();
            return DetectDecisionPoint();
        }
        if (_pendingCardReward != null)
        {
            Log("Skipping card reward");
            _pendingCardReward.OnSkipped();
            _pendingCardReward = null;
        }
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoBuyCard(Player player, Dictionary<string, object?>? args)
    {
        if (_runState?.CurrentRoom is not MerchantRoom merchantRoom)
            return Error("Not in a shop");
        if (args == null || !args.ContainsKey("card_index"))
            return Error("buy_card requires 'card_index'");

        var idx = Convert.ToInt32(args["card_index"]);
        var allEntries = merchantRoom.GetLocalInventory().CharacterCardEntries
            .Concat(merchantRoom.GetLocalInventory().ColorlessCardEntries).ToList();
        if (idx < 0 || idx >= allEntries.Count)
            return Error($"Invalid card index {idx}");

        var entry = allEntries[idx];
        if (!entry.IsStocked) return Error("Card already purchased");
        if (player.Gold < entry.Cost) return Error("Not enough gold");

        try
        {
            entry.OnTryPurchaseWrapper(merchantRoom.GetLocalInventory()).GetAwaiter().GetResult();
            _syncCtx.Pump();
            Log($"Bought card: {entry.CreationResult?.Card?.GetType().Name ?? "?"} for {entry.Cost}g");
        }
        catch (Exception ex) { return Error($"Buy card failed: {ex.Message}"); }

        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoBuyRelic(Player player, Dictionary<string, object?>? args)
    {
        if (_runState?.CurrentRoom is not MerchantRoom merchantRoom)
            return Error("Not in a shop");
        if (args == null || !args.ContainsKey("relic_index"))
            return Error("buy_relic requires 'relic_index'");

        var idx = Convert.ToInt32(args["relic_index"]);
        var entries = merchantRoom.GetLocalInventory().RelicEntries;
        if (idx < 0 || idx >= entries.Count) return Error($"Invalid relic index {idx}");

        var entry = entries[idx];
        if (!entry.IsStocked) return Error("Relic already purchased");
        if (player.Gold < entry.Cost) return Error("Not enough gold");

        // Capture before purchase: a successful OnTryPurchaseWrapper calls
        // ClearAfterPurchase() which nulls entry.Model, so reading it after
        // the await NRE'd INSIDE the try — returning "Buy relic failed" for a
        // purchase that had already completed (gold spent, relic obtained).
        var relicName = entry.Model?.GetType().Name ?? "?";
        var relicCost = entry.Cost;
        try
        {
            // Run the purchase on the thread pool so a relic whose `AfterObtained`
            // opens an interactive card selection (e.g. a deck transform/duplicate
            // — "pick 1 of your N deck cards") can PARK at that selection instead
            // of deadlocking the game thread. The old blocking
            // `OnTryPurchaseWrapper(...).GetAwaiter().GetResult()` hung forever
            // when the relic's AfterObtained suspended at a card-select: the
            // select's continuation lands on the GameSyncContext, which the
            // blocked game thread can never pump (the lone whole-run-parity
            // record_timeout, PARITY118 Ironclad — a shop relic at floor 7).
            // Mirrors the shop card-removal path (DoRemoveCard): settle, and if a
            // selection surfaced, stash the purchase task in `_pendingPurchaseTask`
            // so DoSelectCards (MerchantRoom branch) drives it to completion after
            // the player resolves the selection.
            var task = Task.Run(() => entry.OnTryPurchaseWrapper(merchantRoom.GetLocalInventory()));
            WaitUntilOptionTaskSettles(task, "relic-purchase task or selection");
            // Only `HasPending` (a `select_cards` screen) is stashed here: the
            // MerchantRoom branch of DoSelectCards is the sole driver of
            // `_pendingPurchaseTask`. (PARITY118's relic opens exactly such a deck
            // card-select.) A relic AfterObtained that instead opened a card-reward
            // or a bundle would need its resolver to drive the task too — not
            // wired, and no shipped shop relic hits it.
            if (_cardSelector.HasPending)
            {
                _pendingPurchaseTask = task;
                WaitForActionExecutor();
                return DetectDecisionPoint();
            }
            WaitForTask(task, 2000, "relic-purchase task");
            _syncCtx.Pump();
            Log($"Bought relic: {relicName} for {relicCost}g");
        }
        catch (Exception ex) { return Error($"Buy relic failed: {ex.Message}"); }

        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoBuyPotion(Player player, Dictionary<string, object?>? args)
    {
        if (_runState?.CurrentRoom is not MerchantRoom merchantRoom)
            return Error("Not in a shop");
        if (args == null || !args.ContainsKey("potion_index"))
            return Error("buy_potion requires 'potion_index'");

        var idx = Convert.ToInt32(args["potion_index"]);
        var entries = merchantRoom.GetLocalInventory().PotionEntries;
        if (idx < 0 || idx >= entries.Count) return Error($"Invalid potion index {idx}");

        var entry = entries[idx];
        if (!entry.IsStocked) return Error("Potion already purchased");
        if (player.Gold < entry.Cost) return Error("Not enough gold");

        // Same post-purchase null trap as DoBuyRelic: ClearAfterPurchase()
        // nulls entry.Model on success, so the old `entry.Model.GetType()`
        // log line NRE'd after every completed purchase (the "sometimes
        // NullRefs in headless" this catch used to paper over).
        var potionName = entry.Model?.GetType().Name ?? "?";
        var potionCost = entry.Cost;
        try
        {
            entry.OnTryPurchaseWrapper(merchantRoom.GetLocalInventory()).GetAwaiter().GetResult();
            _syncCtx.Pump();
            Log($"Bought potion: {potionName} for {potionCost}g");
        }
        catch (Exception ex)
        {
            LogException("Buy potion failed", ex);
        }

        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoRemoveCard(Player player)
    {
        if (_runState?.CurrentRoom is not MerchantRoom merchantRoom)
            return Error("Not in a shop");

        var removal = merchantRoom.GetLocalInventory().CardRemovalEntry;
        if (removal == null) return Error("No card removal available");
        if (player.Gold < removal.Cost) return Error("Not enough gold");

        try
        {
            // Run on background thread so card selection can pause (same pattern as event options)
            var task = Task.Run(() => removal.OnTryPurchaseWrapper(merchantRoom.GetLocalInventory()));
            // Wait until the task settles (completes or parks at a selection) rather
            // than on a fixed 1 s budget — see WaitUntilOptionTaskSettles for why a
            // short timeout here drops a still-in-flight task and races its writes.
            WaitUntilOptionTaskSettles(task, "card-removal task or selection");
            if (_cardSelector.HasPending)
            {
                // Stash the task so DoSelectCards can wait for the purchase to
                // finish (and the chosen card to actually leave the deck) AFTER
                // the selection is resolved.
                _pendingPurchaseTask = task;
                WaitForActionExecutor();
                return DetectDecisionPoint();
            }
            WaitForTask(task, 2000, "card-removal task");
            Log($"Removed card for {removal.Cost}g");
        }
        catch (Exception ex) { return Error($"Remove card failed: {ex.Message}"); }

        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoSelectBundle(Player player, Dictionary<string, object?>? args)
    {
        if (_pendingBundleTcs == null || _pendingBundles == null)
            return Error("No pending bundle selection");
        if (args == null || !args.ContainsKey("bundle_index"))
            return Error("select_bundle requires 'bundle_index'");

        var idx = Convert.ToInt32(args["bundle_index"]);
        Log($"Bundle selection: pack {idx}");
        var bundles = _pendingBundles;
        var tcs = _pendingBundleTcs;
        _pendingBundles = null;
        _pendingBundleTcs = null;

        var selected = (idx >= 0 && idx < bundles.Count) ? bundles[idx] : bundles[0];

        // Add the chosen pack to the master deck ourselves, on the game thread,
        // BEFORE letting the suspended continuation resume. Order matters: the
        // game's `ScrollBoxes.AfterObtained` adds its bundle cards (via its own
        // `await CardPileCmd.Add`) to the deck tail and only then returns to the
        // Neow option, which may obtain a SECOND relic whose `AfterObtained`
        // mutates the deck (seed A9P310 Silent A9: LeafyPoultice transforms a
        // Strike + a Defend into two random cards). So the bundle cards must be
        // present, in order, before that follow-on mutation runs.
        //
        // We resolve the bundle TCS EMPTY and add the cards manually rather than
        // resuming the continuation with the real cards because that continuation
        // runs on a thread-pool task (`Task.Run(() => option.Chosen())` in
        // DoChooseOption — no GameSyncContext), so its `CardPileCmd.Add` would
        // mutate the pile off the game thread and not stick.
        //
        // CRITICAL determinism fix — eliminate the writer race by ensuring the
        // game thread and the suspended thread-pool continuation never mutate the
        // deck concurrently:
        //
        //   1. The continuation is still PARKED on the bundle TCS (we have not
        //      resolved it yet), so right now the game thread is the only writer.
        //      Append the chosen pack here — this reproduces ScrollBoxes adding
        //      its bundle cards to the deck tail before it returns.
        //   2. THEN resolve the TCS empty and drain the continuation to full
        //      completion. It runs ScrollBoxes' tail (empty add) and any
        //      follow-on relic obtain (seed A9P310: LeafyPoultice → LoseMaxHp +
        //      transform a Strike & a Defend into two random cards), with the
        //      game thread idle, so it is the sole writer for that phase.
        //
        // Previously the manual adds (each followed by `_syncCtx.Pump()`) ran
        // CONCURRENTLY with the resumed thread-pool continuation's deck
        // mutations; the two writers interleaved nondeterministically, so the
        // settled master-deck order — hence the first combat's StableShuffle
        // input, hence the draw-pile/opening-hand split — flipped run-to-run.
        foreach (var card in selected)
        {
            try
            {
                MegaCrit.Sts2.Core.Commands.CardPileCmd
                    .Add(card, MegaCrit.Sts2.Core.Entities.Cards.PileType.Deck)
                    .GetAwaiter().GetResult();
                _syncCtx.Pump();
                RunManager.Instance.RewardSynchronizer.SyncLocalObtainedCard(card);
            }
            catch (Exception ex) { LogException("Scroll Boxes add to deck", ex); }
        }

        // Now release the parked continuation and drive it to quiescence.
        tcs.TrySetResult(System.Array.Empty<CardModel>());
        _syncCtx.Pump();
        WaitForActionExecutor();
        if (_pendingEventOptionTask != null)
        {
            WaitForTask(_pendingEventOptionTask, 5000, "neow bundle continuation");
            _pendingEventOptionTask = null;
            SyncEventOptionInFlight();
            WaitForActionExecutor();
            _syncCtx.Pump();
            WaitForActionExecutor();
        }
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoSelectCards(Player player, Dictionary<string, object?>? args)
    {
        if (!_cardSelector.HasPending)
            return Error("No pending card selection");
        if (args == null || !args.ContainsKey("indices"))
            return Error("select_cards requires 'indices' (comma-separated card indices)");

        var indicesStr = args["indices"]?.ToString() ?? "";
        var indices = indicesStr.Split(',')
            .Select(s => int.TryParse(s.Trim(), out var v) ? v : -1)
            .Where(i => i >= 0)
            .ToArray();

        Log($"Card selection: indices [{string.Join(",", indices)}]");

        // In COMBAT, resolving this selection can resume a card play that chains
        // into an auto-play (Catastrophe/Havoc/Mayhem) whose sub-cards open
        // FURTHER choices deep inside the resumed play. The manual pump loop
        // cannot surface those follow-on choices in time — their TCS strands and
        // the play deadlocks with cards stuck in the PlayPile (seed 1279: an
        // auto-played Splash's "choose 1 of 3 generated Attacks" screen, leaving
        // the generated card unadded and Splash/Catastrophe in Play → hand 5v6 +
        // discard 1v3 vs Rust). Resolve any such nested choice INLINE at the
        // same option-0 prefix the drain loop uses (and that the Rust engine
        // uses for its inline auto-play choices), so the play runs to
        // completion. Combat-only; the first surfaced choice already resolved
        // above, so this only affects choices opened by the RESUMED play.
        bool combatResolve = CombatManager.Instance.IsInProgress;
        // sts2-agent fork: in agent mode a nested choice is surfaced, not auto-picked.
        bool inlineResolve = combatResolve && !_agentMode;
        if (inlineResolve) _cardSelector.AutoResolveInline = true;
        try
        {
            _cardSelector.ResolvePendingByIndices(indices);
            _syncCtx.Pump();
            if (combatResolve && _agentMode)
            {
                if (!WaitUntilCombatSettlesOrAsks("select_cards"))
                    return Failure("timeout", "started",
                        "the selection was applied but combat did not settle or ask for another choice",
                        "next decision after select_cards", "settle_timeout");
            }
            else
                WaitForActionExecutor();

            // Await the stashed ChooseLocalOption task so the upgrade is fully
            // applied before we force-transition to the map.
            if (!_agentMode && _runState?.CurrentRoom is RestSiteRoom)
            {
                var restSiteTask = _pendingRestSiteTask;
                _pendingRestSiteTask = null;
                if (restSiteTask != null) WaitForTask(restSiteTask, 2000, "rest-site task");
                WaitForActionExecutor();
                Log("Card selection in rest site (SMITH), forcing to map");
                ForceToMap();
                return MapSelectState();
            }

            // Extra wait for shop card removal: the purchase task needs to finish.
            // Continuations land on the GameSyncContext, so a passive Wait would
            // deadlock — WaitForTask drives them.
            if (_runState?.CurrentRoom is MerchantRoom)
            {
                var purchaseTask = _pendingPurchaseTask;
                _pendingPurchaseTask = null;
                if (purchaseTask != null) WaitForTask(purchaseTask, 2000, "shop purchase task");
                WaitForActionExecutor();
                Log("Card selection in shop (card removal), refreshing shop state");
            }

            // Extra wait for an event-relic AfterObtained selection (e.g. Neow's
            // PrecariousShears → remove 2 cards + lose 16 HP). The suspended
            // `EventOption.Chosen()` task resumes its post-await effect once the
            // selection resolves; drive it to completion so the deck removal +
            // HP damage actually apply before the snapshot / map transition.
            // (Without this the run reached the map with an unmodified deck and
            // full HP — the run-parity Neow skew.)
            DrainEventOptionTaskIfDue();

            return DetectDecisionPoint();
        }
        finally
        {
            if (inlineResolve) _cardSelector.AutoResolveInline = false;
        }
    }

    private Dictionary<string, object?> DoSkipSelect(Player player, string what = "Skipping card selection")
    {
        if (_cardSelector.HasPending)
        {
            Log(what);
            _cardSelector.CancelPending();
            _syncCtx.Pump();
            // sts2-agent fork: in agent mode wait for a follow-on selection (or for
            // combat to settle) the same way DoSelectCards does.
            if (_agentMode && CombatManager.Instance.IsInProgress)
            {
                if (!WaitUntilCombatSettlesOrAsks("skip_select"))
                    return Failure("timeout", "started",
                        "the empty selection was applied but combat did not settle or ask for another choice",
                        "next decision after skip_select", "settle_timeout");
            }
            else
                WaitForActionExecutor();

            // Same continuation-drain as DoSelectCards: a skipped event-relic
            // AfterObtained selection (e.g. Neow's HeftyTablet → choose 1 of 3
            // Rares, skippable) still has post-await effects that must run —
            // HeftyTablet adds its Injury curse AFTER the screen resolves,
            // chosen or not. Without driving the stashed task the Injury never
            // landed on a skip (run-parity RP47: cs deck 10 vs rs 11).
            DrainEventOptionTaskIfDue();
        }
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoUsePotion(Player player, Dictionary<string, object?>? args)
    {
        if (args == null || !args.ContainsKey("potion_index"))
            return Error("use_potion requires 'potion_index'");

        var idx = Convert.ToInt32(args["potion_index"]);
        var potionsList = player.Potions?.ToList() ?? new();
        if (idx < 0 || idx >= potionsList.Count) return Error($"Invalid potion index {idx}");
        var potion = potionsList[idx];
        if (potion == null) return Error($"No potion at index {idx}");

        // Determine target based on potion's TargetType first, then fall back to target_index
        Creature? target = null;
        var potionTargetType = potion.TargetType;

        // Player-targeting potions ALWAYS resolve to the player's creature,
        // regardless of any target_index the caller provides. This covers
        // Self / TargetedNoCreature AND AnyPlayer — the latter is the
        // TargetType for the common buff potions (Block, Strength,
        // Dexterity, Flex, Blood, Energy, Swift, …). Their OnUse calls
        // e.g. `CreatureCmd.GainBlock(target, …)` / `ApplyPower(target, …)`,
        // so a null target silently no-ops the whole effect (the potion is
        // consumed but block/strength/energy never land). In single-player
        // AnyPlayer == the local player's creature.
        // sts2-agent fork: in agent mode a potion thrown at no creature (the Foul Potion at a
        // merchant) gets no target, as the game's merchant button gives; with the player's
        // creature it would fail IsValidTarget and the action would be cancelled.
        if (potionTargetType == TargetType.Self
            || (potionTargetType == TargetType.TargetedNoCreature && !_agentMode)
            || potionTargetType == TargetType.AnyPlayer)
        {
            target = player.Creature;
        }
        else if (potionTargetType == TargetType.AnyEnemy)
        {
            // Use caller's target_index if provided, otherwise pick first alive enemy
            if (args.TryGetValue("target_index", out var tObj) && tObj != null)
            {
                var targetIdx = Convert.ToInt32(tObj);
                var combatState = CombatManager.Instance.DebugOnlyGetState();
                if (combatState != null)
                {
                    var enemies = combatState.Enemies.Where(e => e != null && e.IsAlive).ToList();
                    if (targetIdx >= 0 && targetIdx < enemies.Count)
                        target = enemies[targetIdx];
                }
            }
            if (target == null && CombatManager.Instance.IsInProgress)
            {
                var combatState = CombatManager.Instance.DebugOnlyGetState();
                target = combatState?.Enemies?.FirstOrDefault(e => e != null && e.IsAlive);
            }
        }
        // All other target types (None, All, etc.) → leave target as null

        Log($"Using potion: {potion.GetType().Name} at slot {idx} target={target?.GetType().Name ?? "none"}");
        try
        {
            var action = new MegaCrit.Sts2.Core.GameActions.UsePotionAction(potion, target, CombatManager.Instance.IsInProgress);
            RunManager.Instance.ActionQueueSet.EnqueueWithoutSynchronizing(action);
            WaitForActionExecutor();
            _syncCtx.Pump();
            // Verify potion was consumed
            var afterPotions = player.Potions?.ToList() ?? new();
            if (afterPotions.Contains(potion) && _agentMode)
            {
                // sts2-agent fork: report it; discarding would hide why the use failed.
                return Failure("error", "started", $"{potion.Id.Entry} was used but not consumed",
                    "the potion to be consumed", "potion_not_consumed");
            }
            if (afterPotions.Contains(potion))
            {
                // Potion wasn't consumed — manually discard it
                Log("Potion not consumed by action, manually discarding");
                MegaCrit.Sts2.Core.Commands.PotionCmd.Discard(potion).GetAwaiter().GetResult();
                _syncCtx.Pump();
            }
        }
        catch (Exception ex)
        {
            LogException("Use potion failed", ex);
            if (_agentMode) return DetectDecisionPoint();  // reported as an engine error, no discard
            // Try manual discard as fallback
            try { MegaCrit.Sts2.Core.Commands.PotionCmd.Discard(potion).GetAwaiter().GetResult(); } catch { }
        }

        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoDiscardPotion(Player player, Dictionary<string, object?>? args)
    {
        if (args == null || !args.ContainsKey("potion_index"))
            return Error("discard_potion requires 'potion_index'");

        var idx = Convert.ToInt32(args["potion_index"]);
        var potionsList = player.Potions?.ToList() ?? new();
        if (idx < 0 || idx >= potionsList.Count) return Error($"Invalid potion index {idx}");
        var potion = potionsList[idx];
        if (potion == null) return Error($"No potion at index {idx}");

        MegaCrit.Sts2.Core.Commands.PotionCmd.Discard(potion).GetAwaiter().GetResult();
        _syncCtx.Pump();
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoChooseOption(Player player, Dictionary<string, object?>? args)
    {
        if (args == null || !args.ContainsKey("option_index"))
            return Error("choose_option requires 'option_index'");

        var optionIndex = Convert.ToInt32(args["option_index"]);
        Log($"Choosing option {optionIndex}");

        // Dispatch based on ROOM TYPE (not event state) to avoid cross-contamination
        // sts2-agent fork: no forced map after the option (RunSimulator.RestTreasure.cs).
        if (_agentMode && _runState?.CurrentRoom is RestSiteRoom)
            return DoAgentRestOption(optionIndex);

        if (_runState?.CurrentRoom is RestSiteRoom restSiteRoom)
        {
            Log($"Rest site: choosing option {optionIndex}");
            try
            {
                // Run on background thread so Smith card selection can pause
                var task = Task.Run(() => RunManager.Instance.RestSiteSynchronizer.ChooseLocalOption(optionIndex));
                // Wait until the task settles (completes or parks at a selection)
                // rather than on a fixed 1 s budget — see WaitUntilOptionTaskSettles.
                WaitUntilOptionTaskSettles(task, "rest-site option task or selection");
                if (_cardSelector.HasPending)
                {
                    // Stash the task so DoSelectCards can await its completion
                    // (the upgrade) after the player resolves the selection.
                    _pendingRestSiteTask = task;
                    WaitForActionExecutor();
                    return DetectDecisionPoint();
                }
                WaitForTask(task, 2000, "rest-site option task");
            }
            catch (Exception ex)
            {
                LogException("Rest site ChooseLocalOption failed", ex);
            }

            // After non-Smith rest site options (HEAL, etc.), the options may not clear.
            // Wait for the action to complete (heal/dig), then force transition to map.
            if (!_cardSelector.HasPending)
            {
                Log("Rest site: option chosen (non-Smith), waiting for action then forcing to map");
                WaitForActionExecutor();
                _syncCtx.Pump();
                WaitForActionExecutor();
                ForceToMap();
                return MapSelectState();
            }
        }
        // For events — use EventSynchronizer
        // Run Chosen() on a background thread so card selections can pause
        else if (_runState?.CurrentRoom is EventRoom)
        {
            var eventSync = RunManager.Instance.EventSynchronizer;
            var localEvent = eventSync?.GetLocalEvent();
            if (localEvent != null && !localEvent.IsFinished)
            {
                var options = localEvent.CurrentOptions;
                // Subscribe BEFORE `Chosen()` runs, so a `SetEventState` raised
                // on the thread pool is counted no matter how fast it lands.
                TrackEventAdvance(localEvent);
                var advanceBefore = Volatile.Read(ref _eventAdvanceCount);
                if (options != null && optionIndex >= 0 && optionIndex < options.Count)
                {
                    try
                    {
                        _eventOptionChosen = true;
                        _agentLastEventOption = optionIndex;
                        _lastEventAdvanceCount = advanceBefore;
                        // Hand out the non-null NGame stub for the duration of this leg
                        // only — see YieldPatches.EventOptionInFlight for why it must not
                        // be global. Cleared in the finally below, including on the
                        // suspended-at-a-selection path, which returns early.
                        YieldPatches.EventOptionInFlight = true;
                        // Run on thread pool so GetSelectedCards/GetSelectedCardReward can block
                        var task = Task.Run(() =>
                        {
                            // sts2-agent fork: rewards this option offers go to the client.
                            SurfaceCustomRewards.Value = _agentMode;
                            return options[optionIndex].Chosen();
                        });
                        // Wait until the option's `Chosen()` either completes or
                        // suspends at a player decision (card selection / reward /
                        // bundle). The terminal condition is `task.IsCompleted ||
                        // AnySelectionPending()`, NOT a fixed wall-clock budget: a
                        // relic chain (e.g. NeowsBones → RewardsSet.Offer of two
                        // relics, each with its own `AfterObtained` hooks) can take
                        // well over a second on the thread pool before it reaches its
                        // first card selection. A short fixed timeout used to fire
                        // FIRST, the harness fell through to `WaitForTask` (which only
                        // checks `task.IsCompleted`), and when the task then parked on
                        // the selection it never completed → the task was DROPPED,
                        // never stashed, and its post-selection deck writes (NewLeaf
                        // Strike→GunkUp transform + NeowsBones' DEBT curse add) ran
                        // free on the thread pool, racing the snapshot. That is the
                        // A5P441 (Defect asc5) nondeterminism: the settled deck flipped
                        // size 13 (with DEBT + GUNK_UP) ↔ size 12 (missing both),
                        // ~1/15 runs. WaitUntilOptionTaskSettles keeps pumping as long
                        // as the task is still making progress, so the selection is
                        // ALWAYS observed and the task ALWAYS stashed/driven.
                        WaitUntilOptionTaskSettles(task, "event option task or selection/bundle");
                        if (AnySelectionPending())
                        {
                            // The option's `Chosen()` (e.g. a relic AfterObtained that
                            // opens a card selection) is suspended at the `await`. Stash
                            // it so `DoSelectCards` can drive its continuation — the
                            // post-await deck removal / HP damage — to completion after
                            // the player resolves the selection.
                            _pendingEventOptionTask = task;
                            WaitForActionExecutor();
                            return DetectDecisionPoint();
                        }
                        WaitForTask(task, 2000, "event option task");
                    }
                    catch (Exception ex) { LogException("Event choose", ex); }
                    // Stays set when the leg SUSPENDED at a selection (the task is
                    // stashed and `DriveEventOptionTask` resumes it later) — Amalgamator
                    // needs the stub AFTER its card-removal selection, not before.
                    finally { SyncEventOptionInFlight(); }
                }

                // Bead sts2-rs-ck9z6 (GH #166): an event option may have entered a
                // NESTED combat via EventModel.EnterCombatWithoutExitingEvent, which
                // fires RunManager.EnterRoomWithoutExitingCurrentRoom fire-and-forget
                // (the returned Task is discarded). Drive that entry to the play phase
                // so the EXISTING combat decision loop runs the real fight, instead of
                // (a) falling into the "didn't advance" force-finish below, which would
                // abort the fight, or (b) DetectDecisionPoint short-circuiting to an
                // instant-victory reward because CombatManager.IsInProgress is still
                // false. On victory the parent event resumes via
                // ProceedFromTerminalRewardsScreen (see DetectPostCombatState).
                if (_runState?.CurrentRoom is CombatRoom)
                {
                    DriveNestedEventCombatToPlayPhase();
                    return DetectDecisionPoint();
                }

                // "Didn't advance" is MEASURED, not inferred (GH #316): the
                // option's `Chosen()` raised no `StateChanged`, i.e. it called
                // neither `SetEventState` nor `SetEventFinished`, so the event
                // is sitting on exactly the page it started on. That is the real
                // stuck case the guard exists for — `Trial.DoubleDown`, whose
                // whole body is an unreachable UI popup, still gets cut here.
                //
                // Neither predecessor could say that. Count-identity killed the
                // single-option dialogue chain (TheArchitect, the post-Act-3
                // VICTORY event: THREATEN → dialogue.1 → PROCEED, count == 1 at
                // every line) after its FIRST line, so PROCEED's WinRun() never
                // ran and won runs ground the step cap on an empty boss-node map
                // (2/20 masked wins, sweep 20260716T232653Z). Key-identity then
                // killed the repeat-option loops — AbyssalBaths' Linger,
                // SlipperyBridge's HOLD_ON_LOOP — which advance without ever
                // moving a key.
                var optsAfter = localEvent.CurrentOptions;
                var optCountAfter = optsAfter?.Count ?? 0;
                var advanced = Volatile.Read(ref _eventAdvanceCount) != advanceBefore;
                if (!localEvent.IsFinished && !advanced && optCountAfter > 0 && _agentMode)
                {
                    // sts2-agent fork: as the game, the page stays, with the option
                    // disabled (RunSimulator.Events.cs).
                    AgentMarkNoEffect(localEvent, optionIndex);
                    _eventOptionChosen = false;
                }
                else if (!localEvent.IsFinished && !advanced && optCountAfter > 0)
                {
                    Log($"Event {localEvent.GetType().Name} didn't advance, force-finishing");
                    ForceToMap();
                }
            }
        }

        WaitForActionExecutor();
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoLeaveRoom(Player player)
    {
        Log("Leaving room");
        try { RunManager.Instance.ProceedFromTerminalRewardsScreen().GetAwaiter().GetResult(); }
        catch (Exception ex) { LogException("Leave room proceed", ex); }
        _syncCtx.Pump();
        WaitForActionExecutor();

        // If still in a non-combat room, force to map
        var room = _runState?.CurrentRoom;
        if (room is RestSiteRoom || room is MerchantRoom || room is EventRoom || room is TreasureRoom)
        {
            Log("Force leaving non-combat room to map");
            try
            {
                RunManager.Instance.EnterRoom(new MapRoom()).GetAwaiter().GetResult();
                _syncCtx.Pump();
                WaitForActionExecutor();
            }
            catch (Exception ex) { LogException("Force leave", ex); }
        }
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> DoProceed(Player player)
    {
        Log("Proceeding");

        // Check if we need to move to next act (boss defeated)
        var room = _runState?.CurrentRoom;
        if (room is CombatRoom combatRoom && combatRoom.RoomType == RoomType.Boss)
        {
            if (combatRoom.IsPreFinished || !CombatManager.Instance.IsInProgress)
            {
                RunManager.Instance.EnterNextAct().GetAwaiter().GetResult();
                WaitForActionExecutor();
                return DetectDecisionPoint();
            }
        }

        RunManager.Instance.ProceedFromTerminalRewardsScreen().GetAwaiter().GetResult();
        WaitForActionExecutor();
        return DetectDecisionPoint();
    }

    #endregion

    #region Decision Point Detection

    private Dictionary<string, object?> DetectDecisionPoint()
    {
        if (_runState == null)
            return Error("No run in progress");

        var player = _runState.Players[0];

        // sts2-agent fork: a rest-site option or treasure-room step that was waiting on
        // an answered prompt continues first (RunSimulator.RestTreasure.cs).
        if (_agentMode) DriveAgentRoomTasks();
        // sts2-agent fork: an event resumed after its fight that hung (RunSimulator.EventResume.cs).
        if (_agentMode && EventResumeNotSettled() is { } resumeHung) return resumeHung;

        // Check game over (death). A WON run also ends with dead players:
        // RunManager.WinRun → OnEnded(isVictory: true) → GuaranteeKillAllPlayers.
        // Victory is distinguished by the current room being TheArchitect's
        // victory room — the same check NGameOverScreen uses (decomp
        // NGameOverScreen.cs:453: `win = _runState.CurrentRoom?.IsVictoryRoom`).
        if (player.Creature != null && player.Creature.IsDead)
        {
            return GameOverState(_runState.CurrentRoom?.IsVictoryRoom ?? false);
        }

        // sts2-agent fork: Lord's Parasol paused before a potion purchase (RunSimulator.Shop.cs);
        // after the death check, so a run that has ended is game_over, not the pause.
        if (_agentMode && ParasolContinueState(player) is { } parasolPause) return parasolPause;

        // Check if there's a pending bundle selection (Scroll Boxes: pick 1 of N packs)
        if (_pendingBundles != null && _pendingBundleTcs != null && !_pendingBundleTcs.Task.IsCompleted)
        {
            var bundles = _pendingBundles.Select((bundle, i) => new Dictionary<string, object?>
            {
                ["index"] = i,
                // sts2-agent fork: the deck's card JSON, ids included (RunSimulator.Events.cs).
                ["cards"] = _agentMode ? bundle.Select(AgentCardDict).ToList() : bundle.Select(card =>
                {
                    var stats = new Dictionary<string, object?>();
                    try { foreach (var dv in card.DynamicVars.Values) stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue; } catch { }
                    return new Dictionary<string, object?>
                    {
                        ["name"] = _loc.Card(card.Id.Entry),
                        ["cost"] = card.EnergyCost?.GetResolved() ?? 0,
                        ["type"] = card.Type.ToString(),
                        ["description"] = _loc.Bilingual("cards", card.Id.Entry + ".description"),
                        ["stats"] = stats.Count > 0 ? stats : null,
                    };
                }).ToList(),
            }).ToList();

            return new Dictionary<string, object?>
            {
                ["type"] = "decision",
                ["decision"] = "bundle_select",
                ["context"] = RunContext(),
                ["bundles"] = bundles,
                ["player"] = PlayerSummary(player),
            };
        }

        // Check if there's a pending card reward from event (GetSelectedCardReward blocking)
        if (_cardSelector.HasPendingReward)
        {
            var rewardCards = _cardSelector.PendingRewardCards!;
            var cards = rewardCards.Select((cr, i) =>
            {
                var stats = new Dictionary<string, object?>();
                try { foreach (var dv in cr.Card.DynamicVars.Values) stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue; } catch { }
                return new Dictionary<string, object?>
                {
                    ["index"] = i,
                    ["id"] = cr.Card.Id.ToString(),
                    ["name"] = _loc.Card(cr.Card.Id.Entry),
                    ["cost"] = cr.Card.EnergyCost?.GetResolved() ?? 0,
                    ["type"] = cr.Card.Type.ToString(),
                    ["rarity"] = cr.Card.Rarity.ToString(),
                    ["description"] = _loc.Bilingual("cards", cr.Card.Id.Entry + ".description"),
                    ["stats"] = stats.Count > 0 ? stats : null,
                    ["after_upgrade"] = GetUpgradedInfo(cr.Card),
                };
            }).ToList();

            var decision = new Dictionary<string, object?>
            {
                ["type"] = "decision",
                ["decision"] = "card_reward",
                ["context"] = RunContext(),
                ["cards"] = cards,
                ["can_skip"] = true,
                ["from_event"] = true,
                ["player"] = PlayerSummary(_runState!.Players[0]),
            };
            if (_agentMode)
            {
                // sts2-agent fork: the alternatives by id; skip_card_reward is the Skip one.
                var alternatives = CardRewardAlternativesDict();
                decision["alternatives"] = alternatives;
                decision["can_skip"] = alternatives.Any(a => a["id"] as string == "Skip");
                decision["from_event"] = _pendingRewardClaims.Count == 0;
                AgentRewardCardExtras(cards, rewardCards.Select(cr => cr.Card));
            }
            return decision;
        }

        // Check if there's a pending card selection (upgrade, remove, transform, start-of-turn powers)
        checkCardSelect:
        if (_cardSelector.HasPending && _cardSelector.PendingOptions != null)
        {
            var opts = _cardSelector.PendingOptions.Select((card, i) =>
            {
                var stats = new Dictionary<string, object?>();
                try { foreach (var dv in card.DynamicVars.Values) stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue; } catch { }
                return new Dictionary<string, object?>
                {
                    ["index"] = i,
                    ["id"] = card.Id.ToString(),
                    ["name"] = _loc.Card(card.Id.Entry),
                    ["cost"] = card.EnergyCost?.GetResolved() ?? 0,
                    ["type"] = card.Type.ToString(),
                    ["upgraded"] = card.IsUpgraded,
                    ["rarity"] = card.Rarity.ToString(),
                    ["stats"] = stats.Count > 0 ? stats : null,
                    ["description"] = _loc.Bilingual("cards", card.Id.Entry + ".description"),
                    ["after_upgrade"] = GetUpgradedInfo(card),
                };
            }).ToList();

            var cardSelect = new Dictionary<string, object?>
            {
                ["type"] = "decision",
                ["decision"] = "card_select",
                ["context"] = RunContext(),
                ["cards"] = opts,
                ["min_select"] = _cardSelector.PendingMinSelect,
                ["max_select"] = _cardSelector.PendingMaxSelect,
                // Selection semantics captured by the CardSelectCmd context
                // patch (PatchCardSelectContext) just before the game called
                // the selector: prompt_key is the CardSelectorPrefs prompt loc
                // key ("TO_UPGRADE"/"TO_DISCARD"/...), source the CardSelectCmd
                // entry-point name — same fields the SpireMentor mod emits, so
                // pick_card_select_semantic can quality-rank picks the MCTS
                // replay can't answer. Null when no patch fired.
                ["prompt_key"] = _cardSelector.PendingPromptKey,
                ["source"] = _cardSelector.PendingSource,
                ["source_model"] = _cardSelector.PendingSourceModel,
                ["cancelable"] = _cardSelector.PendingCancelable,
                ["player"] = PlayerSummary(player),
                // The whole board, when this selection was opened mid-fight.
                // A card_select message otherwise carries only the offered
                // cards and a player summary — no enemies, hand, energy or
                // piles — which is why the driver used to replay the previous
                // combat_play snapshot forward and hope the engine re-derived
                // the same options (263 of 635 prompts failed that gate on the
                // realdll_v4 sweep). The persistent combat mirror reconciles
                // against this payload instead of guessing. Null out of combat
                // (rest-site upgrade, event grant), where there is no board.
                ["combat"] = InCombatBoard(player),
            };
            // sts2-agent fork: agent mode tells skip, cancel and choosing nothing apart
            // (RunSimulator.Agent.cs, AgentMinSelect).
            if (_agentMode)
            {
                cardSelect["min_select"] = AgentMinSelect;
                cardSelect["cancelable"] = AgentCancelable;
                cardSelect["can_skip"] = AgentCanSkip;
                cardSelect["pile"] = _cardSelector.PendingPile;
                foreach (var (dict, card) in opts.Zip(_cardSelector.PendingOptions))
                    AgentCardExtras(dict, card);
            }
            return cardSelect;
        }

        // sts2-agent fork: a divination that hung, then the minigame waiting for a move.
        if (_agentMode && PendingSphereClickFailure() is { } hung)
            return hung;
        if (_agentMode && CrystalSpherePending)
            return CrystalSphereState(PendingCrystalSphere!, player);

        // sts2-agent fork: rewards offered by an event or relic, innermost set first.
        if (_agentMode && CustomRewardsState(player) is { } customRewards)
            return customRewards;

        // Check if there's a pending card reward
        if (_pendingCardReward != null)
        {
            return CardRewardState(player, _runState.CurrentRoom as CombatRoom);
        }

        // Check if RunManager reports game over (victory)
        if (RunManager.Instance.IsGameOver)
        {
            return GameOverState(true);
        }

        var room = _runState.CurrentRoom;

        // Map room — need to select a node
        if (room is MapRoom || room == null)
        {
            return MapSelectState();
        }

        // Combat room
        if (room is CombatRoom combatRoom)
        {
            // With Task.Yield() patched, combat init should be synchronous
            _syncCtx.Pump();
            WaitForActionExecutor();

            // sts2-agent fork: the play phase alone doesn't mean the last action is done
            // (a card play can still be running). Wait, bounded, for an idle executor, a
            // pending choice or the end of combat; never present a busy combat as a decision.
            if (_agentMode && CombatManager.Instance.IsInProgress && !AnySelectionPending()
                && RunManager.Instance.ActionExecutor.IsRunning
                && !WaitUntilCombatSettlesOrAsks("decision point"))
                return Failure("timeout", "started",
                    "combat did not settle: the action executor is still running and no choice is pending",
                    "an idle executor in the play phase, a choice, or the end of combat", "combat_not_settled");

            // Re-check for pending card selections AFTER pump (BUG-024: start-of-turn effects
            // like Tools of Trade create card selections during Pump, AFTER the initial HasPending check)
            if (_cardSelector.HasPending && _cardSelector.PendingOptions != null)
            {
                goto checkCardSelect;  // Jump back to card_select handling
            }

            if (CombatManager.Instance.IsInProgress && IsPlayPhase())
            {
                return CombatPlayState(player);
            }
            if (!CombatManager.Instance.IsInProgress || (player.Creature != null && player.Creature.IsDead))
            {
                return DetectPostCombatState(player, combatRoom);
            }
            // Fallback: brief wait for combat phase to settle
            WaitFor(() => IsPlayPhase() || !CombatManager.Instance.IsInProgress,
                100, "combat phase settle");
            if (IsPlayPhase()) return CombatPlayState(player);
            if (!CombatManager.Instance.IsInProgress) return DetectPostCombatState(player, combatRoom);
            // sts2-agent fork: neither the play phase nor the end of combat; don't
            // present that as a decision.
            if (_agentMode && !AnySelectionPending())
                return Failure("timeout", "started", "combat neither reached the play phase nor ended",
                    "play phase or end of combat", "combat_not_settled");
            return CombatPlayState(player);
        }

        // Event room
        if (room is EventRoom eventRoom)
        {
            return EventChoiceState(eventRoom);
        }

        // Rest site
        if (room is RestSiteRoom restRoom)
        {
            return RestSiteState(restRoom);
        }

        // Merchant/Shop
        if (room is MerchantRoom merchantRoom)
        {
            return ShopState(merchantRoom, player);
        }

        // Treasure room
        if (room is TreasureRoom treasureRoom)
        {
            return TreasureState(treasureRoom);
        }

        // Fallback
        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "unknown",
            ["context"] = RunContext(),
            ["room_type"] = room?.GetType().Name,
            ["message"] = "Unknown room type or state",
        };
    }

    /// Keep `_eventAdvanceCount` wired to `ev`'s `StateChanged` (GH #316).
    ///
    /// `EventSynchronizer` hands out the SAME `EventModel` instance for the
    /// whole event (it is a slot in `_events`), so re-hooking only happens when
    /// the player actually enters a different event — at which point the counter
    /// restarts from zero, and the stuck-event guards below detect the instance
    /// swap rather than comparing counts across two different events.
    private void TrackEventAdvance(EventModel? ev)
    {
        if (ev == null || ReferenceEquals(ev, _advanceTrackedEvent))
            return;
        if (_advanceTrackedEvent != null && _eventAdvanceHandler != null)
        {
            try { _advanceTrackedEvent.StateChanged -= _eventAdvanceHandler; }
            catch { /* the old event is gone; nothing left to unhook */ }
        }
        _eventAdvanceHandler = _ => Interlocked.Increment(ref _eventAdvanceCount);
        Volatile.Write(ref _eventAdvanceCount, 0);
        _lastEventAdvanceCount = 0;
        ev.StateChanged += _eventAdvanceHandler;
        _advanceTrackedEvent = ev;
    }

    /// Drop the hook when leaving the room: the next event gets a fresh counter,
    /// and nothing keeps the finished `EventModel` alive through our delegate.
    private void ResetEventAdvanceTracking()
    {
        if (_advanceTrackedEvent != null && _eventAdvanceHandler != null)
        {
            try { _advanceTrackedEvent.StateChanged -= _eventAdvanceHandler; }
            catch { /* the old event is gone; nothing left to unhook */ }
        }
        _advanceTrackedEvent = null;
        _eventAdvanceHandler = null;
        Volatile.Write(ref _eventAdvanceCount, 0);
        _lastEventAdvanceCount = 0;
    }

    /// Return Winged Boots' total "ignore paths" uses for the run (the relic's
    /// ``Rooms`` dynamic var, default 3), or 0 if the player doesn't own it.
    private int WingedBootsMaxUses()
    {
        var boots = _runState?.Players?.FirstOrDefault()?.Relics?
            .FirstOrDefault(r => r.Id.Entry == "WINGED_BOOTS");
        if (boots == null)
            return 0;
        try
        {
            foreach (var dv in boots.DynamicVars.Values)
                if (dv.Name == "Rooms")
                    return (int)dv.BaseValue;
        }
        catch { /* fall through to default */ }
        return 3;
    }

    /// True if (col,row) is NOT a connected child of the current map node — i.e.
    /// reaching it requires Winged Boots' ignore-paths effect.
    private bool IsOffPathChoice(int col, int row)
    {
        var cur = _runState?.CurrentMapCoord;
        if (cur == null || _runState?.Map == null)
            return false;
        var curPoint = _runState.Map.GetPoint(cur.Value);
        var isChild = curPoint?.Children?.Any(ch => ch.coord.col == col && ch.coord.row == row) ?? false;
        return !isChild;
    }

    private Dictionary<string, object?> MapSelectState()
    {
        var map = _runState?.Map;
        if (map == null)
        {
            Log("Map is null, generating...");
            try
            {
                RunManager.Instance.GenerateMap().GetAwaiter().GetResult();
                _syncCtx.Pump();
                map = _runState?.Map;
            }
            catch (Exception ex)
            {
                LogException("GenerateMap failed", ex);
            }
            if (map == null)
                return Error("No map available");
        }
        var currentCoord = _runState!.CurrentMapCoord;

        List<Dictionary<string, object?>> choices;
        if (currentCoord.HasValue)
        {
            var currentPoint = map.GetPoint(currentCoord.Value);
            if (currentPoint == null)
            {
                Log($"GetPoint returned null for coord ({currentCoord.Value.col},{currentCoord.Value.row}), falling back to start");
                // Current coord is invalid (stale after forced room transition); treat as no position
                choices = new List<Dictionary<string, object?>>();
                var sp = map.StartingMapPoint;
                if (sp?.Children != null)
                {
                    foreach (var child in sp.Children)
                    {
                        choices.Add(new Dictionary<string, object?>
                        {
                            ["col"] = (int)child.coord.col,
                            ["row"] = (int)child.coord.row,
                            ["type"] = child.PointType.ToString(),
                        });
                    }
                }
            }
            else
            {
                choices = (currentPoint.Children ?? Enumerable.Empty<MapPoint>())
                    .Select(child => new Dictionary<string, object?>
                    {
                        ["col"] = (int)child.coord.col,
                        ["row"] = (int)child.coord.row,
                        ["type"] = child.PointType.ToString(),
                    })
                    .ToList();

                // Winged Boots: the relic lets the player ignore path
                // connections when choosing the next room, a limited number of
                // times. While charges remain, offer every node in the next row
                // (the connected children are a subset of these); once the uses
                // are spent, fall back to children only. The relic's "Rooms"
                // dynamic var IS the remaining-use counter (TimesUsed setter
                // writes 3 - TimesUsed; AfterRoomEntered counts off-path
                // entries) — gate on it alone, mirroring the game's own
                // ShouldAllowFreeTravel() => !IsUsedUp.
                var bootsMax = WingedBootsMaxUses();
                if (bootsMax > 0)
                {
                    int nextRow = currentPoint.coord.row + 1;
                    var rowChoices = (map.GetPointsInRow(nextRow) ?? Enumerable.Empty<MapPoint>())
                        .Where(p => p != null)
                        .Select(p => new Dictionary<string, object?>
                        {
                            ["col"] = (int)p.coord.col,
                            ["row"] = (int)p.coord.row,
                            ["type"] = p.PointType.ToString(),
                        })
                        .ToList();
                    if (rowChoices.Count > 0)
                    {
                        Log($"WINGED_BOOTS: {bootsMax} uses left — offering {rowChoices.Count} next-row nodes");
                        choices = rowChoices;
                    }
                }
            }
        }
        else
        {
            // Starting point — pick from starting row
            var startPoint = map.StartingMapPoint;
            choices = new List<Dictionary<string, object?>>
            {
                new()
                {
                    ["col"] = (int)startPoint.coord.col,
                    ["row"] = (int)startPoint.coord.row,
                    ["type"] = startPoint.PointType.ToString(),
                }
            };

            // Act 1 played_save start (StartedWithNeow=false, no coord visited
            // yet): the real game's EnterAct else-branch (RunManager.cs:1268-1282)
            // shows the map with the player positioned below it — the only
            // reachable node is StartingMapPoint itself (its children aren't
            // offered until AFTER it's entered, exactly like every other act's
            // MapRoom start below). Verified against the recorded real trail for
            // seed 12345: real's first map_select offers exactly one choice,
            // (col=3,row=0,Monster) == StartingMapPoint. Gated tightly on the
            // exact condition RunManager uses for this branch so default
            // (StartedWithNeow=true, act 0) and every other act's start (which
            // always hits this same null-coord branch and has offered
            // start+children here since before played_save existed) are
            // untouched byte-for-byte.
            var isPlayedSaveAct1Start = _runState.CurrentActIndex == 0 && !_runState.ExtraFields.StartedWithNeow;
            if (!isPlayedSaveAct1Start && startPoint.Children != null)
            {
                // Add all children of start point as well since we can travel to them
                foreach (var child in startPoint.Children)
                {
                    choices.Add(new Dictionary<string, object?>
                    {
                        ["col"] = (int)child.coord.col,
                        ["row"] = (int)child.coord.row,
                        ["type"] = child.PointType.ToString(),
                    });
                }
            }
        }

        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "map_select",
            ["context"] = RunContext(),
            ["choices"] = choices,
            ["player"] = PlayerSummary(_runState!.Players[0]),
            ["act"] = _runState.CurrentActIndex + 1,
            ["act_name"] = _loc.Act(_runState.Act?.Id.Entry ?? "OVERGROWTH"),
            ["floor"] = _runState.ActFloor,
        };
    }

    /// The TinkerTime rider effect of a Mad Science card (Sapping / Violence /
    /// …), or null for any other card. Mad Science morphs per-instance: its
    /// `type` (= `c.Type` = TinkerTimeType) plus this rider drive both the
    /// per-type art and the conditional description branches the workbench
    /// resolves. See `MadScience.cs::AddExtraArgsToDescription`.
    private static string? MadScienceRider(CardModel c) =>
        c is MadScience ms ? ms.TinkerTimeRider.ToString() : null;

    /// Serialize a single combat card. `includePlayability` controls whether
    /// `can_play` / `target_type` are included (true for hand cards, false for
    /// pile cards where playability is meaningless).
    /// <summary>
    /// The card's own energy cost, local modifiers applied and global power
    /// hooks excluded — the value a state reconstruction must use as the base
    /// so transient discounts stay transient.
    ///
    /// -1 is the engine's single "not a real cost" sentinel, covering both
    /// X-cost cards and unplayable ones (its Dazed/Burn specs use -1 the same
    /// way its Whirlwind/Volley specs do). GetResolved() clamps those to 0,
    /// which is indistinguishable from a genuinely free card, so they are
    /// normalized here instead.
    /// </summary>
    /// <summary>
    /// A power's own `DynamicVars`, as `{name: baseValue}`. Null when it has
    /// none, which is the common case.
    ///
    /// `Amount` and `DisplayAmount` describe the stack; they say nothing about
    /// a power's per-instance accumulator, which lives in a DynamicVar.
    /// CrimsonMantle is the clear case: `Amount` is the block it grants, while
    /// the damage it deals you every turn is `SelfDamage`, bumped once per
    /// copy played (`CrimsonMantle.cs:28` -> `IncrementSelfDamage`). With no
    /// wire representation, every reconstruction rebuilt it at zero and the
    /// mirror simply skipped that damage — 104 of the 107 single-field
    /// `$.player.hp` divergences on the v10 sweep were a mirror sitting
    /// exactly 1 HP high at `end_turn`, with CrimsonMantle up.
    ///
    /// Deliberately raw and generic: this emits whatever vars the power
    /// carries and takes no view on what they mean. Mapping a var onto the
    /// engine's own vocabulary is the adapter's job, where that vocabulary
    /// actually lives.
    /// </summary>
    /// <summary>
    /// A relic's live counter, or null when it does not track one.
    /// `ShowCounter` is the game's own "this relic counts something" flag and
    /// `DisplayAmount` is the value; the backing field is private.
    /// </summary>
    /// <summary>
    /// Relics whose live state is a once-per-combat LATCH rather than a
    /// displayed tally: a private bool with no public accessor and no
    /// <c>ShowCounter</c>, so <see cref="SafeRelicCounter"/> reports null and
    /// every reconstruction re-arms an effect the game has already spent.
    ///
    /// Measured on the v18 corpus: the mirror tracks these on
    /// <c>Relic.counter</c> (Vambrace 68 records, RuinedHelmet 21) while the
    /// world sent a counter for them exactly ZERO times out of 140 and 63
    /// occurrences. Receipt for RuinedHelmet — it doubles the first positive
    /// Strength gain of a combat, so a re-armed helmet doubles again: with the
    /// seed-1000 relic set the engine took Strength 2 -> 8 where the game took
    /// it 2 -> 5, which is the whole `$.player.powers` mirror-high family.
    ///
    /// Deliberately an ALLOWLIST, not "reflect every private bool". The decomp
    /// has ~45 relics with a private bool, but most are `_isActivating`
    /// re-entrancy guards that carry no cross-decision meaning, and emitting a
    /// counter for a relic whose engine side does not read one is noise at
    /// best. Entries are added when the corpus shows the mirror tracking a
    /// counter the world never sends — the same evidence gate the
    /// `DisplayAmount` counter family used.
    ///
    /// Polarity is verified per entry against the engine: `1` == latched
    /// (Vambrace "block already gained this combat", RuinedHelmet "already
    /// doubled"), matching what the engine writes to `counter` itself.
    /// </summary>
    private static readonly Dictionary<string, System.Reflection.FieldInfo?> RelicLatchFields =
        new()
        {
            ["VAMBRACE"] = LatchField("Vambrace", "_blockGainedThisCombat"),
            ["RUINED_HELMET"] = LatchField("RuinedHelmet", "_usedThisCombat"),
            // First-Power-card-this-combat 7-block grant; the engine latches it
            // on Relic.counter. 51 of the 140 on-play $.player.block records on
            // the v27wr sweep were a re-armed Permafrost paying its unpowered 7
            // again on every Power card (delta exactly -7, invariant across
            // which card was played).
            ["PERMAFROST"] = LatchField("Permafrost", "_activatedThisCombat"),
        };

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

    /// <summary>
    /// Read a `CardModel` bool that walks the keyword set, defaulting to false
    /// if the model throws. `Keywords` is computed (intrinsic + `LocalKeywords`)
    /// and a card mid-`RemoveFromState` can fault on it; a missing flag costs
    /// the reconstruction one turn of Sly/Retain, an exception costs the whole
    /// decision record. GH #370.
    /// </summary>
    private static bool SafeCardBool(Func<bool> read)
    {
        try { return read(); }
        catch { return false; }
    }

    private static int? SafeRelicCounter(RelicModel r)
    {
        try
        {
            if (r.ShowCounter) return r.DisplayAmount;
            // A latch relic declares no counter, but its spent/unspent bit is
            // exactly the state a reconstruction must not lose.
            if (RelicLatchFields.TryGetValue(r.Id.Entry, out var f) && f != null)
            {
                return (f.GetValue(r) is bool b && b) ? 1 : 0;
            }
            return null;
        }
        catch { return null; }
    }

    private Dictionary<string, object?>? PowerVars(PowerModel pw) => PowerVars(pw, _agentMode ? AgentLocText : null);

    /// <summary>
    /// A power's variables. sts2-agent fork: given `text`, a string variable keeps its text,
    /// localized as an event option's (Nightmare's card title), instead of 0.
    /// </summary>
    private static Dictionary<string, object?>? PowerVars(PowerModel pw, Func<string, string>? text)
    {
        try
        {
            var vars = pw.DynamicVars?.Values;
            if (vars == null) return null;
            var d = new Dictionary<string, object?>();
            foreach (var dv in vars)
                d[dv.Name] = text != null && dv is MegaCrit.Sts2.Core.Localization.DynamicVars.StringVar sv
                    ? text(sv.StringValue ?? "") : (int)dv.BaseValue;
            return d.Count > 0 ? d : null;
        }
        catch { return null; }
    }

    /// <summary>
    /// Monsters whose cross-decision AI state lives in a PRIVATE field on the
    /// mutable <see cref="MonsterModel"/> — the moveset analogue of
    /// <see cref="RelicLatchFields"/>, and the same evidence-gated ALLOWLIST
    /// discipline: an entry is added only when the engine demonstrably tracks
    /// the value (on <c>MonsterAi.counter</c>) and the wire never carries it.
    ///
    /// - <c>Fabricator._lastSpawned</c> (Fabricator.cs:37): excluded from the
    ///   next <c>SpawnBot</c> pool. The engine infers it as the kind of the
    ///   highest-uid living bot, which is unrecoverable once that bot died —
    ///   the mirror then re-widens a forced pick back to a coin flip
    ///   (Zapbot-vs-Stabbot summon flips, v35 seed 1001).
    /// - <c>KnowledgeDemon._curseOfKnowledgeCounter</c> (KnowledgeDemon.cs:66):
    ///   indexes both the curse card set and the Disintegration damage table
    ///   (6/7/8) and gates CURSE_OF_KNOWLEDGE re-entry at 3. Not derivable
    ///   from the wire mid-fight — the DISINTEGRATION off-by-one.
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
    /// already-built enemy dict. Quiet for every monster not on the list, and
    /// quiet for a Fabricator that has not spawned yet (absent field ==
    /// "nothing to disclose", mirroring the relic-latch convention).
    /// Wire shape: <c>fabricator_last_spawned</c> = the spawned bot's
    /// canonical <c>MonsterModel.Id.Entry</c> (e.g. <c>"ZAPBOT"</c>);
    /// <c>curse_of_knowledge_counter</c> = the raw int (0..3).
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

    // Null when the cost model cannot be read at all — NOT -1, which the
    // consumer decodes as X-cost/unplayable. Guarded like its bridge twin
    // `SimStateExtractor.SafeBaseCost`: `GetWithModifiers` walks the modifier
    // list and a serializer must not take the whole message down over one card.
    internal static int? BaseEnergyCost(CardModel c)
    {
        try
        {
            var ec = c.EnergyCost;
            if (ec == null) return null;
            if (ec.CostsX) return -1;
            var v = ec.GetWithModifiers(CostModifiers.Local);
            return v < 0 ? -1 : v;
        }
        catch { return null; }
    }

    // CardEnergyCost._localModifiers. The list itself is the one piece of the
    // cost model with no public accessor; LocalCostModifier and every member
    // we read off it (Amount / Type / Expiration / IsReduceOnly / Modify) are
    // public, so this is a single field read, not a reimplementation.
    private static readonly System.Reflection.FieldInfo? LocalModifiersField =
        typeof(CardEnergyCost).GetField(
            "_localModifiers",
            System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);

    /// <summary>
    /// What <see cref="BaseEnergyCost"/> will be once this turn's cost
    /// modifiers wear off — the card's cost after C# <c>EndOfTurnCleanup()</c>.
    ///
    /// A local modifier carries a LIFETIME (EndOfCombat / EndOfTurn /
    /// WhenPlayed) that a single number cannot express, and collapsing them
    /// loses exactly the distinction a state reconstruction needs: Stomp's
    /// "1 less per Attack played THIS TURN" and Enlightenment's "costs 1 for
    /// the rest of COMBAT" both arrive as a cheaper card, but only the first
    /// springs back at turn start. Rebuilt from <c>base_cost</c> alone, a
    /// mirror keeps every transient discount forever: on the v8 sweep a
    /// shaved STOMP stayed at 0-2 while the game reset it to 3, and 163 of
    /// its 175 cost divergences landed on the turn boundary that should have
    /// restored it.
    ///
    /// Computed by re-folding the local modifiers while skipping the ones
    /// EndOfTurnCleanup would remove — the same loop as
    /// <c>GetWithModifiers(Local)</c>, with the same guards. Null when the
    /// value cannot differ from <c>base_cost</c> (no local modifiers, X-cost,
    /// canonical/negative base) or when the field read fails, which keeps the
    /// wire quiet for the overwhelmingly common unmodified card.
    /// </summary>
    internal static int? PersistentEnergyCost(CardModel c) =>
        FoldLocalModifiers(c, LocalCostModifierExpiration.EndOfTurn);

    /// <summary>
    /// The cost this card will have once it has been PLAYED — the local
    /// modifiers re-folded with the <see cref="LocalCostModifierExpiration.WhenPlayed"/>
    /// ones dropped, mirroring <c>CardEnergyCost.AfterCardPlayedCleanup()</c>.
    /// </summary>
    /// <remarks>
    /// Distinct from <see cref="PersistentEnergyCost"/>, and the difference is
    /// not academic: the game's own comments say the "costs N this turn"
    /// helpers are <c>SetThisTurnOrUntilPlayed</c> / <c>AddThisTurnOrUntilPlayed</c>
    /// (<c>EndOfTurn | WhenPlayed</c>, whichever comes first), and flag the
    /// pure-EndOfTurn variants "BE CAREFUL USING THIS!". A consumer that
    /// restores such a discount only at the turn boundary keeps it alive on a
    /// card the game already retired it on — 169 of the 1569 divergences on
    /// the v15 sweep were a discarded card whose cost the mirror still had
    /// shaved to 0.
    /// </remarks>
    internal static int? PlayedEnergyCost(CardModel c) =>
        FoldLocalModifiers(c, LocalCostModifierExpiration.WhenPlayed);

    /// <summary>
    /// The cost once BOTH transient edges have passed — the card played and
    /// the turn ended — leaving only the modifiers that last all combat.
    /// </summary>
    /// <remarks>
    /// The third number is load-bearing, not belt-and-braces — though not
    /// because the other two collide. One combined <c>EndOfTurn | WhenPlayed</c>
    /// modifier folds to <c>(persistent, played) = (base, base)</c>; two
    /// separate modifiers fold to <c>(M2(base), M1(base))</c>, and those differ
    /// unless both modifiers are no-ops.
    ///
    /// What the pair cannot say is how much of the shave SURVIVES the first
    /// edge. With one combined modifier the whole shave retires at whichever
    /// edge fires first and there is nothing left to re-bank; with two, each
    /// edge retires only its own half. The cost once BOTH cleanups have run is
    /// the fixed point that tells the consumer's
    /// <c>rebank_wire_cost_after_edge</c> which of the two it is holding.
    /// </remarks>
    internal static int? StableEnergyCost(CardModel c) =>
        FoldLocalModifiers(
            c, LocalCostModifierExpiration.EndOfTurn | LocalCostModifierExpiration.WhenPlayed);

    /// Re-fold a card's local cost modifiers from its permanent base, skipping
    /// every modifier carrying ANY of the <paramref name="expiring"/> flags.
    /// Null when the card has no local modifiers to fold, or the fold cannot
    /// be trusted.
    private static int? FoldLocalModifiers(CardModel c, LocalCostModifierExpiration expiring)
    {
        try
        {
            // Guards INSIDE the try, like the bridge twin: `c.EnergyCost` and
            // `c.IsCanonical` are the reads most likely to throw here, and
            // outside the guard they took the whole message with them.
            var ec = c.EnergyCost;
            if (ec == null || ec.CostsX || !ec.HasLocalModifiers || c.IsCanonical) return null;
            // GetWithModifiers(None) returns the private `_base` untouched by
            // any modifier — the upgrade-adjusted permanent cost to fold from.
            var baseCost = ec.GetWithModifiers(CostModifiers.None);
            if (baseCost < 0) return null;
            if (LocalModifiersField?.GetValue(ec) is not IEnumerable<LocalCostModifier> mods) return null;
            var cost = baseCost;
            foreach (var m in mods)
            {
                if ((m.Expiration & expiring) != 0) continue;
                cost = m.Modify(cost);
            }
            return Math.Max(0, cost);
        }
        catch { return null; }
    }

    /// <summary>
    /// Serialize a creature's telegraphed intents. Shared by the alive and the
    /// dead enemy branches — a corpse the game keeps in combat (Decimillipede
    /// segments awaiting REATTACH) telegraphs its revive one turn ahead, and
    /// that is exactly the turn a consumer most needs to know about it.
    /// Returns an empty list when the move carries no intents; C# builds plenty
    /// of moves that way (`new MoveState("DEAD_MOVE", DeadMove)` passes no
    /// `params AbstractIntent[]`), and "no intents" is a real, observable state
    /// distinct from a `HiddenIntent`.
    /// </summary>
    private static List<Dictionary<string, object?>> SerializeIntents(
        Creature e, List<Creature>? playerCreatures)
    {
        var intents = new List<Dictionary<string, object?>>();
        try
        {
            if (e.Monster?.NextMove?.Intents == null) return intents;
            foreach (var intent in e.Monster.NextMove.Intents)
            {
                var intentInfo = new Dictionary<string, object?>
                {
                    ["type"] = intent.IntentType.ToString(),
                };
                // Get damage for attack intents. Emit PER-HIT damage
                // (GetSingleDamage) and the repeat count separately so
                // downstream consumers (notably sts2-rs's
                // state_dict::parse_intent under bead sts2-rs-2rp7) can
                // reconstruct Intent::MultiAttack { damage, hits } without
                // double-multiplying. The previous GetTotalDamage emission was
                // overstating multi-hit damage by `hits×` once consumers
                // learned to honor the `hits` field. Bead sts2-rs-x4nn.
                if (intent is MegaCrit.Sts2.Core.MonsterMoves.Intents.AttackIntent atk && playerCreatures != null)
                {
                    try
                    {
                        intentInfo["damage"] = atk.GetSingleDamage(playerCreatures, e);
                        intentInfo["hits"] = atk.Repeats;
                    }
                    catch { }
                }
                intents.Add(intentInfo);
            }
        }
        catch { }
        return intents;
    }

    private Dictionary<string, object?> SerializeCombatCard(
        CardModel c, int index, PlayerCombatState? pcs, bool includePlayability)
    {
        var stats = new Dictionary<string, object?>();
        try
        {
            foreach (var dv in c.DynamicVars.Values)
                stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue;
        }
        catch { }

        var starCost = c.BaseStarCost;
        var cardInfo = new Dictionary<string, object?>
        {
            ["index"] = index,
            ["id"] = c.Id.ToString(),
            ["name"] = _loc.Card(c.Id.Entry),
            ["cost"] = c.EnergyCost?.GetResolved() ?? 0,
            // The card instance's OWN cost: upgrade-adjusted base with its
            // LOCAL modifiers applied (Stomp's per-attack shave, Eidolon's
            // until-played zero) but WITHOUT the global power hooks
            // (FreeAttack, Tangled, Enthralled). That is exactly what the
            // engine stores in `Card.cost` — the globals are re-derived from
            // the powers we already ship, so a consumer that rebuilds from
            // `cost` alone bakes a transient discount in FOREVER (a spent
            // FreeAttack left every Strike in hand at 0 for the rest of the
            // fight). X-cost cards report -1, the engine's X sentinel;
            // `GetWithModifiers` would give 0 for them, which is a real cost.
            ["base_cost"] = BaseEnergyCost(c),
            // What `base_cost` becomes once this turn's cost modifiers wear
            // off. Only present when it can differ — see PersistentEnergyCost.
            ["persistent_cost"] = PersistentEnergyCost(c),
            // What this card's cost becomes the moment it is PLAYED. The
            // "this turn" discounts are really "this turn OR until played",
            // so a consumer that only expires them at the turn boundary keeps
            // a spent discount alive. See PlayedEnergyCost.
            ["played_cost"] = PlayedEnergyCost(c),
            // And once BOTH edges have passed. Needed to tell a single
            // "this turn or until played" modifier apart from two separate
            // ones — see StableEnergyCost.
            ["stable_cost"] = StableEnergyCost(c),
            ["type"] = c.Type.ToString(),
            // Upgrade identity for state reconstruction: `stats` only carries
            // damage/block, so without this flag every effect-var upgrade
            // (magic numbers, draw counts, status amounts) reverts to the
            // unupgraded registry value in the dict-reconstructed search
            // state. The run-scope deck serializer already emits it.
            ["upgraded"] = c.IsUpgraded,
            ["stats"] = stats.Count > 0 ? stats : null,
            ["description"] = _loc.Bilingual("cards", c.Id.Entry + ".description"),
            ["tinker_time_rider"] = MadScienceRider(c),
            // GH #370. The SINGLE-TURN halves of Sly and Retain live in two
            // private `CardModel` bools (`_hasSingleTurnSly`,
            // `_hasSingleTurnRetain`, cleared in `EndOfTurnCleanup`) that the
            // `keywords` list below cannot show: `Keywords` carries only the
            // intrinsic set plus `LocalKeywords` (what `AddKeyword` adds).
            // Without these two the reconstruction cannot tell a Well-Laid
            // Plans retain-for-this-turn from an intrinsic Retain, so it had
            // to drop both — and a rebuilt mirror then let the card flush at
            // end of turn while the world kept it.
            //
            // Emitted as the PUBLIC properties rather than the private bools,
            // so the consumer subtracts the keyword itself:
            //   single-turn retain == will_retain && !keywords.Retain
            //   single-turn sly    == sly_this_turn && !keywords.Sly
            // (`CardModel.cs:591` / `:620` — each is `keyword || singleTurn`.)
            ["will_retain"] = SafeCardBool(() => c.ShouldRetainThisTurn),
            ["sly_this_turn"] = SafeCardBool(() => c.IsSlyThisTurn),
        };
        if (includePlayability)
        {
            cardInfo["can_play"] = c.CanPlay(out _, out _);
            cardInfo["target_type"] = c.TargetType.ToString();
        }
        if (starCost > 0)
        {
            cardInfo["star_cost"] = starCost;
            // BUG-007: Override can_play for star-cost cards when player lacks stars
            // sts2-agent fork: not in agent mode, where CanPlay already checks the star cost
            // with its modifiers (a free Dying Star from Splash is playable at 0 stars).
            if (includePlayability && pcs != null && pcs.Stars < starCost && !_agentMode)
                cardInfo["can_play"] = false;
        }
        var kws = c.Keywords?.Where(k => k != CardKeyword.None).Select(k => k.ToString()).ToList();
        if (kws?.Count > 0) cardInfo["keywords"] = kws;
        if (c.Enchantment != null)
        {
            cardInfo["enchantment"] = _loc.Bilingual("enchantments", c.Enchantment.Id.Entry + ".title");
            try { if (c.Enchantment.Amount != 0) cardInfo["enchantment_amount"] = c.Enchantment.Amount; } catch { }
        }
        if (c.Affliction != null)
        {
            cardInfo["affliction"] = _loc.Bilingual("afflictions", c.Affliction.Id.Entry + ".title");
            try { if (c.Affliction.Amount != 0) cardInfo["affliction_amount"] = c.Affliction.Amount; } catch { }
        }
        if (_agentMode) AgentCardExtras(cardInfo, c);  // sts2-agent fork: every card view's fields
        return cardInfo;
    }

    /// <summary>
    /// The full combat board as a <c>combat_play</c> payload, or null when no
    /// fight is in progress.
    ///
    /// Attached to <c>card_select</c> so a mid-combat selection carries the
    /// same board a <c>combat_play</c> decision does. Deliberately the very
    /// same builder: the Python side already knows how to translate a
    /// <c>combat_play</c> dict, so embedding one whole needs no second
    /// translation path that could drift from the first.
    ///
    /// A failure here is logged and reported as "no board" rather than thrown,
    /// which would abort the run. The consumer decides how serious that is —
    /// the combat mirror treats a missing board on an in-combat selection as a
    /// divergence and refuses to guess.
    /// </summary>
    private Dictionary<string, object?>? InCombatBoard(Player player)
    {
        try
        {
            if (!CombatManager.Instance.IsInProgress) return null;
            return CombatPlayState(player);
        }
        catch (Exception ex)
        {
            LogException("card_select: could not build the combat board", ex);
            return null;
        }
    }

    /// <summary>
    /// Agent mode: an enemy's model id (`MONSTER.X`, the name's own key) and
    /// its `combat_id`, the game's `Creature.CombatId`. The game assigns that
    /// once, when the creature is attached to the fight (`CombatState.
    /// AttachCreature`, from a counter that only grows), and a revival heals
    /// the same creature, so the id survives its own death and revival and
    /// any other enemy's death, revival or summoning; a summoned enemy gets
    /// the next one. The list index stays the targeting position among the
    /// living enemies. Default mode keeps upstream's shape.
    /// </summary>
    private void AppendEnemyIdentity(Dictionary<string, object?> dict, Creature e)
    {
        if (!_agentMode) return;
        dict["model_id"] = e.Monster?.Id.ToString();
        dict["combat_id"] = e.CombatId.HasValue ? (long)e.CombatId.Value : null;
    }

    private Dictionary<string, object?> CombatPlayState(Player player)
    {
        var pcs = player.PlayerCombatState;
        var combatState = CombatManager.Instance.DebugOnlyGetState();

        // Track last known HP for accurate game_over reporting (BUG-005)
        if (player.Creature != null && player.Creature.CurrentHp > 0)
            _lastKnownHp = player.Creature.CurrentHp;

        var hand = pcs?.Hand?.Cards?.Select((c, i) =>
            SerializeCombatCard(c, i, pcs, includePlayability: true)).ToList() ?? new();

        var playerCreatures = combatState?.PlayerCreatures?.ToList();

        var enemies = combatState?.Enemies?
            .Where(e => e != null && e.IsAlive)
            .Select((e, i) =>
            {
                var intents = SerializeIntents(e, playerCreatures);

                // Enemy powers
                var ePowers = e.Powers?.Select(pw => new Dictionary<string, object?>
                {
                    ["name"] = _loc.Power(pw.Id.Entry),
                    // Canonical power id (PowerModel.Id.Entry, e.g. "MANGLE_POWER"),
                    // the same value the canonical/wire shape emits as "kind". The
                    // hint adapter reconstructs the power from this id unambiguously
                    // — the localized title alone cannot (e.g. "Mangle" maps to both
                    // the real ManglePower and its legacy Mangle alias, and powers
                    // with no loc title resolve to None and were silently dropped,
                    // defanging the enemy in the dict-reconstructed search).
                    ["id"] = pw.Id.Entry,
                    ["description"] = _loc.Bilingual("powers", pw.Id.Entry + ".description"),
                    ["amount"] = pw.Amount,
                    // Counter powers (e.g. Slow) override DisplayAmount: Amount is
                    // the stack count (constant) while DisplayAmount is the value
                    // shown to the player and the one that actually scales. Emit
                    // both so the UI can render the meaningful number.
                    ["display_amount"] = pw.DisplayAmount,
                    // Same contract as the player-power block below: a power's
                    // per-instance accumulator lives in a DynamicVar that
                    // neither Amount nor DisplayAmount describes. Withholding
                    // it from ENEMY powers only (the historical shape) left
                    // Thievery's stolen-Gold tally — and every other
                    // enemy-side var — structurally undisclosed to the
                    // mirror, while the SpireMentor twin (SerializePower)
                    // already emitted vars for all creatures.
                    ["vars"] = PowerVars(pw),
                }).ToList();

                var enemyDict = new Dictionary<string, object?>
                {
                    ["index"] = i,
                    ["name"] = _loc.Monster(e.Monster?.Id.Entry ?? "UNKNOWN"),
                    ["hp"] = e.CurrentHp,
                    ["max_hp"] = e.MaxHp,
                    ["block"] = e.Block,
                    ["intents"] = intents.Count > 0 ? intents : null,
                    ["intends_attack"] = e.Monster?.IntendsToAttack ?? false,
                    ["powers"] = ePowers?.Count > 0 ? ePowers : null,
                    // The telegraphed move-state id (MoveState.StateId), same
                    // field the parity exporter emits. The hint adapter pins
                    // the rebuilt monster AI to it (sts2_rs force_current_
                    // state_by_name_relaxed); without it every reconstructed
                    // enemy is AI-less after its first move in rollouts — the
                    // exact planning blindness fixed live in PRs #263/#264.
                    ["move_state"] = e.Monster?.NextMove?.Id,
                };
                AppendEnemyIdentity(enemyDict, e);
                // Allowlisted private AI state (Fabricator._lastSpawned,
                // KnowledgeDemon curse counter) — see AppendMonsterHiddenState.
                AppendMonsterHiddenState(enemyDict, e.Monster);
                return enemyDict;
            }).ToList() ?? new();

        // Dead-but-still-in-combat enemies (revivers: Decimillipede segments
        // awaiting REATTACH, Eye with Teeth illusions, Test Subject phases...).
        // The game removes ordinary corpses from Enemies; whatever remains
        // with IsAlive == false WILL come back, and a search model that never
        // sees the corpse treats every kill as permanent. Kept in a SEPARATE
        // list so the alive list's index semantics (play_card targeting) are
        // untouched.
        var deadEnemies = combatState?.Enemies?
            .Where(e => e != null && !e.IsAlive)
            .Select(e =>
            {
                var dIntents = SerializeIntents(e, playerCreatures);
                var dPowers = e.Powers?.Select(pw => new Dictionary<string, object?>
                {
                    ["name"] = _loc.Power(pw.Id.Entry),
                    ["id"] = pw.Id.Entry,
                    ["amount"] = pw.Amount,
                    ["display_amount"] = pw.DisplayAmount,
                    // See the alive-enemy block: same vars contract, and a
                    // reviving corpse keeps its accumulators through death
                    // (ReattachPower and friends), so the corpse wire must
                    // not silently zero them either.
                    ["vars"] = PowerVars(pw),
                }).ToList();
                var corpseDict = new Dictionary<string, object?>
                {
                    ["name"] = _loc.Monster(e.Monster?.Id.Entry ?? "UNKNOWN"),
                    ["hp"] = e.CurrentHp,
                    ["max_hp"] = e.MaxHp,
                    ["block"] = e.Block,
                    ["is_dead"] = true,
                    ["powers"] = dPowers?.Count > 0 ? dPowers : null,
                    // A corpse that is coming back telegraphs what it will do.
                    // `ReattachPower.AfterDeath` arms the segment onto
                    // DEAD_MOVE (no intents), whose FollowUpState is
                    // REATTACH_MOVE (a HealIntent) — so the turn before a
                    // Decimillipede segment revives, the wire is the only
                    // place that says so. Omitting this field made every
                    // reviving corpse read as an intent divergence against a
                    // mirror that models the revive correctly, and left the
                    // search blind to a 25-HP enemy about to re-enter the
                    // fight. Same shape as the alive branch above.
                    ["intents"] = dIntents.Count > 0 ? dIntents : null,
                    ["move_state"] = e.Monster?.NextMove?.Id,
                };
                AppendEnemyIdentity(corpseDict, e);
                // Same allowlisted private AI state as the alive branch — a
                // reviving monster's counters survive its death.
                AppendMonsterHiddenState(corpseDict, e.Monster);
                return corpseDict;
            }).ToList() ?? new();

        // Player powers/buffs
        var playerPowers = player.Creature?.Powers?.Select(pw => new Dictionary<string, object?>
        {
            ["name"] = _loc.Power(pw.Id.Entry),
            // Canonical power id; see the enemy-power note above (lets the hint
            // adapter reconstruct the power from the id, not the ambiguous title).
            ["id"] = pw.Id.Entry,
            ["description"] = _loc.Bilingual("powers", pw.Id.Entry + ".description"),
            ["amount"] = pw.Amount,
            // See enemy-power note above: DisplayAmount is the player-facing
            // (and scaling) value for Counter powers; Amount is the stack count.
            ["display_amount"] = pw.DisplayAmount,
            ["vars"] = PowerVars(pw),
        }).ToList();

        // Serialize draw/discard/exhaust piles. Cards in piles use the same
        // shape as hand cards but without can_play/target_type (not applicable).
        // Listing draw pile in actual order matches what set_draw_order expects;
        // it's a deliberate design choice for testing/AI use cases.
        var drawPile = pcs?.DrawPile?.Cards?.Select((c, i) =>
            SerializeCombatCard(c, i, pcs, includePlayability: false)).ToList() ?? new();
        var discardPile = pcs?.DiscardPile?.Cards?.Select((c, i) =>
            SerializeCombatCard(c, i, pcs, includePlayability: false)).ToList() ?? new();
        // The card that is MID-PLAY. C# parks it in its own `PlayPile` "so
        // that it isn't counted towards your hand or your discard pile"
        // (PileType.Play), and moves it to the discard only once the play
        // finishes — which, for a card whose OnPlay awaits a selection screen,
        // is after the player picks.
        //
        // A `card_select` board is therefore ALWAYS serialized mid-play, and
        // without this pile the played card appears in none of them. A state
        // rebuilt from that board has nothing to flush when the choice
        // resolves, so the card silently fails to reach the discard: every
        // Headbutt / Snap / Photon Cut that opened a screen showed up as one
        // missing discard card on the v10 sweep (173 world-only HEADBUTT
        // entries alone). The engine has carried a `play_pile` on the wire
        // since the oracle found the same gap on its own side.
        List<Dictionary<string, object?>> playPile = new();
        try
        {
            var inPlay = pcs?.PlayPile?.Cards;
            if (inPlay != null)
                playPile = inPlay.Select((c, i) =>
                    SerializeCombatCard(c, i, pcs, includePlayability: false)).ToList();
        }
        catch { }
        List<Dictionary<string, object?>> exhaustPile = new();
        try
        {
            var exhaust = pcs?.ExhaustPile?.Cards;
            if (exhaust != null)
                exhaustPile = exhaust.Select((c, i) =>
                    SerializeCombatCard(c, i, pcs, includePlayability: false)).ToList();
        }
        catch { }

        // Per-turn play state the mid-turn ShouldPlay gates need (Ringing /
        // Normality / Sloth buckets + ChainsOfBinding's once-per-turn Bound
        // gate). Same fields the SpireMentor mod emits; without them a
        // dict-reconstructed search state believes zero cards were played
        // this turn and re-offers plays the engine vetoes.
        var playerSummary = PlayerSummary(player);
        try
        {
            playerSummary["cards_played_this_turn"] = CountCardsPlayedThisTurn(combatState, player.Creature);
            if (BoundCardPlayedThisTurn(combatState, player.Creature))
                playerSummary["bound_card_played_this_turn"] = true;
        }
        catch { }

        var result = new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "combat_play",
            ["context"] = RunContext(),
            ["round"] = combatState?.RoundNumber ?? 0,
            ["energy"] = pcs?.Energy ?? 0,
            ["max_energy"] = pcs?.MaxEnergy ?? 0,
            ["hand"] = hand,
            ["enemies"] = enemies,
            ["dead_enemies"] = deadEnemies.Count > 0 ? deadEnemies : null,
            ["player"] = playerSummary,
            ["player_powers"] = playerPowers?.Count > 0 ? playerPowers : null,
            ["draw_pile"] = drawPile,
            ["discard_pile"] = discardPile,
            ["exhaust_pile"] = exhaustPile,
            // Only present when a card is actually mid-play — see above.
            ["play_pile"] = playPile.Count > 0 ? playPile : null,
            ["draw_pile_count"] = drawPile.Count,
            ["discard_pile_count"] = discardPile.Count,
            ["exhaust_pile_count"] = exhaustPile.Count,
            // The run's RNG position. `run_seed` is RunRngSet.Seed — the
            // once-hashed master seed every per-stream salt mixes into, which
            // is exactly what sts2-rs's `RunRngSet::new` takes — and
            // `counters` is each stream's read count.
            //
            // Without them a state reconstruction has no RNG lineage at all:
            // it seeds from the SEARCH seed, so its monster_ai and shuffle
            // streams have nothing to do with the game's. Every enemy intent
            // and every reshuffle is then a guess, and the resulting
            // divergence says nothing about fidelity because the value was
            // never knowable. With them the mirror rolls the same dice the
            // game does, and an intent mismatch becomes what it should be: a
            // real disagreement about what the engine does with a given roll.
            //
            // This is ground truth for the MIRROR, not for the search — the
            // search clone is blindfolded at its root and is provably
            // invariant to the cursor (sts2-mcts
            // `root_scramble_invariant_to_pile_order_and_rng_cursor`).
            // Published as a PAIR or not at all — see `MirrorRngBlock`.
            ["rng"] = MirrorRngBlock(),
        };

        // Character-specific mechanics
        try
        {
            // Defect: Orbs. Emit whenever the queue has slots (Defect) so the
            // workbench shows the orb belt — including its empty slots — from
            // turn 1, not only once an orb is channelled. ``orbs`` is the list
            // of channelled orbs (empty when none); ``orb_slots`` is the belt
            // capacity the frontend pads with empty slots.
            var orbQueue = pcs?.OrbQueue;
            if (orbQueue != null && orbQueue.Capacity > 0)
            {
                result["orbs"] = orbQueue.Orbs.Select((orb, i) => new Dictionary<string, object?>
                {
                    ["index"] = i,
                    ["name"] = _loc.Bilingual("orbs", orb.Id.Entry + ".title"),
                    ["type"] = orb.GetType().Name.Replace("Orb", ""),
                    ["passive"] = (int)orb.PassiveVal,
                    ["evoke"] = (int)orb.EvokeVal,
                }).ToList();
                result["orb_slots"] = orbQueue.Capacity;
            }

            // Regent: Stars
            // sts2-agent fork: for every character in agent mode (Shining Strike gives an
            // Ironclad stars; a foreign star-cost card needs them).
            if (pcs != null && pcs.Stars >= 0 && (_agentMode || player.Character?.Id.Entry == "REGENT"))
            {
                result["stars"] = pcs.Stars;
            }

            // Necrobinder: Osty (minion)
            var osty = player.Osty;
            if (osty != null)
            {
                result["osty"] = new Dictionary<string, object?>
                {
                    ["name"] = _loc.Monster(osty.Monster?.Id.Entry ?? "OSTY"),
                    ["hp"] = osty.CurrentHp,
                    ["max_hp"] = osty.MaxHp,
                    ["block"] = osty.Block,
                    ["alive"] = osty.IsAlive,
                };
                // sts2-agent fork: Osty's powers (Die For You takes the player's unblocked damage).
                if (_agentMode)
                    ((Dictionary<string, object?>)result["osty"]!)["powers"] = osty.Powers?.Select(pw => new Dictionary<string, object?>
                    {
                        ["id"] = pw.Id.Entry,
                        ["amount"] = pw.Amount,
                        ["display_amount"] = pw.DisplayAmount,
                        ["vars"] = PowerVars(pw),
                    }).ToList();
            }
            else if (player.Character?.Id.Entry == "NECROBINDER")
            {
                result["osty"] = new Dictionary<string, object?> { ["alive"] = false };
            }
        }
        catch (Exception ex)
        {
            LogException("Character-specific data", ex);
        }

        return result;
    }

    private Dictionary<string, object?> DetectPostCombatState(Player player, CombatRoom combatRoom)
    {
        Log($"Post-combat: RoomType={combatRoom.RoomType}, IsPreFinished={combatRoom.IsPreFinished}");
        // Drain any queued end-of-combat actions (e.g. Shrinker Beetle's
        // permanent card removal) before snapshotting the deck for rewards.
        WaitForActionExecutor();

        // Bead sts2-rs-ck9z6 (GH #166): an event-nested combat that should resume
        // its parent event (BattlewornDummy — ShouldResumeParentEventAfterCombat)
        // grants its reward via the event's Resume(), NOT the combat-room reward
        // set. Skip combat-reward generation entirely and route straight back into
        // the event: ProceedFromTerminalRewardsScreen detects the resume flag and
        // runs ResumePreviousRoom → EventRoom.Resume → EventSynchronizer.ResumeEvents
        // → the event's Resume() (e.g. BattlewornDummy.Resume rolls the per-setting
        // potion/upgrade/relic, or grants nothing on RanOutOfTime). No-resume event
        // combats (TheLanternKey / FakeMerchant) fall through to the normal reward
        // path below — their extra rewards come via WithRewardsFromRoom.
        // Gate on a real parent room on the stack (CurrentRoomCount > 1), mirroring
        // RunManager.ProceedFromTerminalRewardsScreen (RunManager.cs:1337): a normal
        // map combat enters via EnterRoom -> ExitCurrentRooms first (stack depth 1),
        // whereas an event-nested combat enters via EnterRoomWithoutExitingCurrentRoom
        // (depth 2). ShouldResumeParentEventAfterCombat defaults to true, so without
        // the depth check normal combats wrongly take the resume path, which resets
        // the reward bookkeeping and regenerates the card reward on every detect --
        // an infinite card_reward loop with monotonically growing gold.
        if (combatRoom.ShouldResumeParentEventAfterCombat && (_runState?.CurrentRoomCount ?? 0) > 1)
        {
            Log("Event-nested combat: resuming parent event for its reward");
            try
            {
                RunManager.Instance.ProceedFromTerminalRewardsScreen().GetAwaiter().GetResult();
            }
            catch (Exception ex) { LogException("Resume parent event", ex); }
            // The event's Resume() is dispatched fire-and-forget
            // (EventSynchronizer.ResumeEvents → TaskHelper.RunSafely), so pump it
            // (and any reward commands it queues) to completion before re-detecting.
            _syncCtx.Pump();
            WaitForActionExecutor();
            // sts2-agent fork: the event's Resume() is kept and driven, so what it offers
            // reaches the client (RunSimulator.EventResume.cs).
            if (_agentMode) DriveAgentEventResume();
            // Reset the combat-reward bookkeeping so the resumed event's own
            // reward surfaces / drains cleanly on the next decision detect.
            _rewardsProcessed = false;
            _pendingRewards = null;
            _pendingCardReward = null;
            // Recursion guard: only re-detect if the resume actually advanced off
            // this finished combat room. A real event-spawned combat transitions
            // CurrentRoom to its parent EventRoom here; but a combat that merely
            // *reports* ShouldResumeParentEventAfterCombat with no real parent
            // event to resume into (e.g. the synthetic parity-harness arenas)
            // leaves CurrentRoom pinned to this same finished CombatRoom. Without
            // this check, DetectDecisionPoint() routes straight back into
            // DetectPostCombatState (combat still not in progress) with the flag
            // still set, and the two mutually recurse until the stack overflows.
            // If the resume did not move us, fall through to the normal
            // combat-reward path below instead of recursing.
            if (!ReferenceEquals(_runState?.CurrentRoom, combatRoom))
            {
                return DetectDecisionPoint();
            }
            Log("Event-nested resume did not advance room; falling through to combat reward");
        }

        // sts2-agent fork: a real rewards screen instead (RunSimulator.Rewards.cs).
        if (_agentMode && !_rewardsProcessed)
            return AgentRewardsState(player, combatRoom);

        // Generate rewards manually instead of using TestMode auto-accept
        if (_pendingRewards == null && !_rewardsProcessed)
        {
            _goldBeforeCombat = player.Gold;
            try
            {
                var rewardsSet = new RewardsSet(player).WithRewardsFromRoom(combatRoom);
                // v0.107.1: GenerateWithoutOffering() now returns Task (was
                // Task<List<Reward>>); the generated rewards live on .Rewards.
                rewardsSet.GenerateWithoutOffering().GetAwaiter().GetResult();
                var rewards = rewardsSet.Rewards;
                _syncCtx.Pump();

                // Auto-collect gold and potions, but present card choices to agent
                var cardRewards = new List<CardReward>();
                foreach (var reward in rewards)
                {
                    // SpecialCardReward (a Reward, NOT a CardReward): a specific
                    // card added straight to the deck — e.g. ThievingHopper's
                    // "stolen card is returned" on kill (SwipePower.BeforeDeath).
                    // Without this it fell through both branches and the stolen
                    // card was silently lost even after killing the thief.
                    if (reward is GoldReward || reward is MegaCrit.Sts2.Core.Rewards.RelicReward
                        || reward is MegaCrit.Sts2.Core.Rewards.PotionReward
                        || reward is MegaCrit.Sts2.Core.Rewards.SpecialCardReward)
                    {
                        try { reward.SelectUnsynchronized().GetAwaiter().GetResult(); _syncCtx.Pump(); }  // v0.107.1: OnSelectWrapper->SelectUnsynchronized
                        catch (Exception ex) { LogException("Auto-collect reward", ex); }
                    }
                    else if (reward is CardReward cr)
                    {
                        cardRewards.Add(cr);
                    }
                }

                if (cardRewards.Count > 0)
                {
                    _pendingCardReward = cardRewards[0];
                    _pendingRewards = rewards;
                    return CardRewardState(player, combatRoom);
                }

                _pendingRewards = null;
            }
            catch (Exception ex) { LogException("Generate rewards", ex); }
        }

        // No more pending rewards — proceed
        _pendingCardReward = null;
        _pendingRewards = null;
        _rewardsProcessed = true;

        // Boss → next act
        if (combatRoom.RoomType == RoomType.Boss)
        {
            // sts2-agent fork: A10 (Double Boss). With a second boss and the player on
            // the first boss's node, the game's Proceed goes back to the map
            // (NRewardsScreen.OnProceedButtonPressed → ProceedFromTerminalRewardsScreen),
            // not to the act change; the player then clicks the second boss's node, the
            // first boss node's only child, and RoomSet.NextBossEncounter fights it.
            // Nothing is forced: the map move is the client's.
            var map = _runState?.Map;
            if (_agentMode && map?.SecondBossMapPoint != null
                && _runState!.CurrentMapCoord == map.BossMapPoint.coord)
            {
                Log("First boss defeated, back to the map for the second boss");
                ForceToMap();
                return MapSelectState();
            }

            Log("Boss defeated, entering next act");
            try
            {
                var actBefore = _runState?.CurrentActIndex;
                RunManager.Instance.EnterNextAct().GetAwaiter().GetResult();
                _syncCtx.Pump();
                WaitForActionExecutor();
                // sts2-agent fork: the next act is reported with the decision it leads to
                // (after the last act's boss no act follows: TheArchitect's event comes next).
                if (_agentMode && _runState != null && _runState.CurrentActIndex != actBefore)
                    _agentForced.Add(new() { ["kind"] = "next_act", ["act"] = _runState.CurrentActIndex + 1 });

                // Each non-first act's map begins on an Ancient node whose event
                // offers the act-boss relic (a 1-of-3 choice). The shipped game
                // requires the player to travel to that starting node to take it;
                // EnterNextAct only lands on the MapRoom (RunManager.EnterAct), so
                // without this the boss relic is never surfaced and silently lost.
                // Auto-enter the starting Ancient node here — mirroring the act-0
                // Neow auto-entry in RunManager.EnterAct (RunManager.cs:1024) — so
                // the relic choice is SURFACED as an event_choice decision via the
                // existing EventChoiceState. The Ancient's offers come off a
                // per-event RNG isolated from up_front (EventModel.cs:193), so
                // entering it here does not perturb the global RNG cursor. This
                // matches the Rust engine, which hoists the same Ancient offer to
                // Phase::BossRelic immediately after the boss card reward.
                var nextMap = _runState?.Map;
                if (nextMap?.StartingMapPoint != null
                    && nextMap.StartingMapPoint.PointType == MapPointType.Ancient
                    && _runState!.CurrentRoom is MapRoom)
                {
                    Log("Auto-entering act-start Ancient node for boss-relic choice");
                    var start = nextMap.StartingMapPoint.coord;
                    RunManager.Instance.EnterMapCoord(start).GetAwaiter().GetResult();
                    _syncCtx.Pump();
                    WaitForActionExecutor();
                    // sts2-agent fork: the host entered the Ancient's node for the player
                    // (the game waits for a click on it), so the client is told.
                    if (_agentMode)
                        _agentForced.Add(new()
                        {
                            ["kind"] = "enter_ancient",
                            ["node"] = new Dictionary<string, object?> { ["col"] = (int)start.col, ["row"] = (int)start.row },
                        });
                }
            }
            catch (Exception ex) { LogException("EnterNextAct", ex); }
            return DetectDecisionPoint();
        }

        // Normal → go to map
        ForceToMap();
        return MapSelectState();
    }

    private Dictionary<string, object?> CardRewardState(Player player, CombatRoom? combatRoom)
    {
        if (_pendingCardReward == null)
            return DetectPostCombatState(player, combatRoom ?? (_runState?.CurrentRoom as CombatRoom)!);

        var cards = _pendingCardReward.Cards.Select((c, i) =>
        {
            var stats = new Dictionary<string, object?>();
            try { foreach (var dv in c.DynamicVars.Values) stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue; } catch { }
            return new Dictionary<string, object?>
            {
                ["index"] = i,
                ["id"] = c.Id.ToString(),
                ["name"] = _loc.Card(c.Id.Entry),
                ["cost"] = c.EnergyCost?.GetResolved() ?? 0,
                ["type"] = c.Type.ToString(),
                ["rarity"] = c.Rarity.ToString(),
                ["description"] = _loc.Bilingual("cards", c.Id.Entry + ".description"),
                ["stats"] = stats.Count > 0 ? stats : null,
                ["after_upgrade"] = GetUpgradedInfo(c),
            };
        }).ToList();

        // sts2-agent fork: what the offered cards are, as shown (RunSimulator.Shop.cs).
        if (_agentMode) AgentRewardCardExtras(cards, _pendingCardReward.Cards);
        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "card_reward",
            ["context"] = RunContext(),
            ["cards"] = cards,
            ["can_skip"] = _pendingCardReward.CanSkip,
            ["gold_earned"] = _runState!.Players[0].Gold - _goldBeforeCombat,
            ["player"] = PlayerSummary(_runState!.Players[0]),
        };
    }

    private void ForceToMap()
    {
        // A finished run must NOT navigate away from the current room: the
        // victory path (TheArchitect PROCEED → RunManager.WinRun → OnEnded →
        // GuaranteeKillAllPlayers) leaves all players dead while CurrentRoom
        // is still the victory EventRoom. Moving to a MapRoom here made the
        // dead-player check in DetectDecisionPoint report a WON run as defeat
        // (CurrentRoom.IsVictoryRoom was false by the time it ran).
        if (_runState != null && _runState.Players.Count > 0 &&
            _runState.Players.All(p => p.Creature?.IsDead ?? false))
        {
            Log("ForceToMap: all players dead (run ended), staying in current room");
            return;
        }

        try
        {
            RunManager.Instance.ProceedFromTerminalRewardsScreen().GetAwaiter().GetResult();
            _syncCtx.Pump();
        }
        catch (Exception ex) { LogException("ForceToMap proceed", ex); }

        if (_runState?.CurrentRoom is not MapRoom)
        {
            try { RunManager.Instance.EnterRoom(new MapRoom()).GetAwaiter().GetResult(); _syncCtx.Pump(); }
            catch (Exception ex) { LogException("ForceToMap", ex); }
        }
    }

    private Dictionary<string, object?> EventChoiceState(EventRoom eventRoom)
    {
        var localEvent = RunManager.Instance.EventSynchronizer?.GetLocalEvent();
        _syncCtx.Pump();

        // sts2-agent fork: the fake merchant's relics are a shop (RunSimulator.FakeMerchant.cs).
        if (_agentMode && AgentFakeMerchantState(_runState!.Players[0]) is { } fakeShop) return fakeShop;

        // If we already chose an event option and the event didn't advance, force-finish
        if (_eventOptionChosen && localEvent != null && !localEvent.IsFinished)
        {
            // Same measured signal as the guard in `DoChooseOption` (GH #316) —
            // this one is the backstop for legs that PARKED at a card selection
            // and resumed later, so their `SetEventState` lands after that guard
            // has already returned. Repeating option keys are NOT evidence of
            // being stuck (AbyssalBaths' Linger and SlipperyBridge's HOLD_ON_LOOP
            // page repeat theirs by design while the run keeps changing); no
            // `StateChanged` since the pick is.
            var currentOpts = localEvent.CurrentOptions;
            var sameEvent = ReferenceEquals(localEvent, _advanceTrackedEvent);
            var stuck = sameEvent && currentOpts != null && currentOpts.Count > 0 &&
                Volatile.Read(ref _eventAdvanceCount) == _lastEventAdvanceCount;
            if (stuck && _agentMode && _agentLastEventOption >= 0)
            {
                // sts2-agent fork: shown again with the option disabled (RunSimulator.Events.cs).
                AgentMarkNoEffect(localEvent, _agentLastEventOption);
            }
            else if (stuck)
            {
                Log($"Event {localEvent.GetType().Name}: same options after choice, force-finishing");
                _eventOptionChosen = false;
                ForceToMap();
                return MapSelectState();
            }
            // Options changed — event advanced to next page, show new options
            _eventOptionChosen = false;
        }

        // If event is finished, proceed to map
        if (localEvent == null || localEvent.IsFinished)
        {
            Log($"Event {localEvent?.GetType().Name ?? "null"} finished, proceeding");
            AgentEventFinished(localEvent);  // sts2-agent fork: reported as a forced step
            try
            {
                RunManager.Instance.ProceedFromTerminalRewardsScreen().GetAwaiter().GetResult();
                _syncCtx.Pump();
            }
            catch (Exception ex) { LogException("Finished event proceed", ex); }
            // Force to map if still in event room
            if (_runState?.CurrentRoom is EventRoom)
            {
                try { RunManager.Instance.EnterRoom(new MapRoom()).GetAwaiter().GetResult(); _syncCtx.Pump(); }
                catch (Exception ex) { LogException("Finished event to map", ex); }
            }
            return _runState?.CurrentRoom is MapRoom ? MapSelectState() : DetectDecisionPoint();
        }

        var currentOptions = localEvent.CurrentOptions;
        if (currentOptions == null || currentOptions.Count == 0)
        {
            // sts2-agent fork: a failure to diagnose, not a silent map (RunSimulator.Events.cs).
            if (_agentMode) return AgentEventWithoutOptions(localEvent);
            Log($"Event {localEvent.GetType().Name} has no options, auto-skipping");
            try { RunManager.Instance.EnterRoom(new MapRoom()).GetAwaiter().GetResult(); _syncCtx.Pump(); }
            catch (Exception ex) { LogException("Option-less event to map", ex); }
            return MapSelectState();
        }

        var options = currentOptions
            .Select((opt, i) =>
            {
                // Try to resolve title via loc tables
                string? title = null;
                if (opt.Title != null)
                {
                    var t = _loc.Bilingual(opt.Title.LocTable, opt.Title.LocEntryKey);
                    // Check if we actually found a translation (not just the key echoed back)
                    if (t != opt.Title.LocEntryKey)
                        title = t;
                }
                // Fallback: try to extract option ID from the key and look up as relic/card/potion
                if (title == null && opt.TextKey != null)
                {
                    // TextKey like "NEOW.pages.INITIAL.options.STONE_HUMIDIFIER" → extract "STONE_HUMIDIFIER"
                    var parts = opt.TextKey.Split('.');
                    var optionId = parts.Length > 0 ? parts[^1] : opt.TextKey;
                    // Try relic, then card, then just use the optionId
                    var relic = _loc.Relic(optionId);
                    if (relic != optionId + ".title")
                        title = relic;
                    else
                    {
                        var card = _loc.Card(optionId);
                        if (card != optionId + ".title")
                            title = card;
                        else
                            title = optionId.Replace("_", " ");
                    }
                }
                title ??= $"option_{i}";

                // Description: try loc table first
                string? optDesc = null;
                if (opt.Description != null && !string.IsNullOrEmpty(opt.Description.LocEntryKey))
                {
                    var d = _loc.Bilingual(opt.Description.LocTable, opt.Description.LocEntryKey);
                    if (d != opt.Description.LocEntryKey)
                        optDesc = d;
                }
                // Fallback: try relic/card description
                if (optDesc == null && opt.TextKey != null)
                {
                    var parts = opt.TextKey.Split('.');
                    var optionId = parts.Length > 0 ? parts[^1] : opt.TextKey;
                    var rd = _loc.Bilingual("relics", optionId + ".description");
                    if (rd != optionId + ".description")
                        optDesc = rd;
                }

                // Extract vars: try event's own DynamicVars first, then relic
                Dictionary<string, object?>? optVars = null;
                try
                {
                    // Event's DynamicVars (covers Gold, HpLoss, Heal, etc.)
                    if (localEvent.DynamicVars?.Values != null)
                    {
                        optVars = new Dictionary<string, object?>();
                        foreach (var dv in localEvent.DynamicVars.Values)
                            // sts2-agent fork: a text variable (CrystalSphere's CurseTitle) is
                            // its text, not a number, in agent mode.
                            optVars[dv.Name] = _agentMode && dv is MegaCrit.Sts2.Core.Localization.DynamicVars.StringVar sv
                                ? AgentLocText(sv.StringValue ?? "")
                                : (int)dv.BaseValue;
                    }
                }
                catch { }
                // Relic-offer options (event relic pools — e.g. Nonupeipe's
                // SignetRing — and Neow) carry their relic model directly on the
                // option. Pull its DynamicVars so {Gold}/{Heal}/… resolve to real
                // numbers instead of leaking the bare template placeholder
                // ("Upon pickup, gain Gold Gold."). Prefer the attached model;
                // fall back to a ModelDb lookup by the option id parsed from the
                // TextKey (GetByIdOrNull, never the throwing GetById — a miss must
                // not silently abort the whole vars block).
                RelicModel? optRelic = opt.Relic;
                if (optRelic == null && opt.TextKey != null)
                {
                    try
                    {
                        var parts = opt.TextKey.Split('.');
                        var optionId = parts.Length > 0 ? parts[^1] : opt.TextKey;
                        optRelic = ModelDb.GetByIdOrNull<RelicModel>(new ModelId("RELIC", optionId))?.ToMutable();
                    }
                    catch { }
                }
                if (optRelic != null)
                {
                    try
                    {
                        optVars ??= new Dictionary<string, object?>();
                        foreach (var dv in optRelic.DynamicVars.Values)
                            optVars[dv.Name] = (int)dv.BaseValue;
                    }
                    catch { }
                }

                // The game's OWN verdict on whether taking this option ends the run,
                // evaluated exactly as NEventOptionButton.WillKillPlayer() does. Without
                // it the Python guard can only PARSE an HP cost out of the description,
                // which misses every non-damage lethality: Trial.DoubleDown carries
                // `ThatWillKillPlayerIf(_ => true)` (it opens the abandon-run confirm)
                // and TabletOfTruth.Decipher keys on MAX HP, so neither shows an HP cost
                // to parse. The predicates are pure reads over the player, so evaluating
                // them per serialization is free of side effects.
                //
                // Tri-state, and the null case is load-bearing. A predicate-null or
                // Owner-null option really is NOT flagged — `EventRoomHandler.cs:97`
                // keeps exactly those — so `false` is faithful there. A probe that
                // THREW is a different thing: we do not know, and emitting `false`
                // would tell Python the world said "safe". Absent instead, so the
                // consumer falls back to its HP-cost parse rather than trusting a
                // verdict that was never computed. `TabletOfTruth`'s closure does a
                // `DynamicVars["DecipherMaxHpLoss"]` lookup and is the one predicate
                // in the pool that can realistically throw — and non-damage lethality
                // is precisely what the parse cannot see, so failing open here would
                // undo the whole point.
                bool? willKill = null;
                if (opt.WillKillPlayer == null || localEvent.Owner == null)
                {
                    willKill = false;
                }
                else
                {
                    try { willKill = opt.WillKillPlayer(localEvent.Owner); }
                    catch (Exception ex)
                    {
                        // `Log`, not `LogException`: this is a DIAGNOSTIC read the game
                        // never asked us to make (AutoSlay evaluates the predicate only
                        // when it is about to choose), and options are re-serialized on
                        // every state read — so routing it through the world-error
                        // channel would score N errors for one fault and pollute the
                        // throw corpus. The `null` above is how the fault is handled.
                        Log($"WillKillPlayer probe ({opt.TextKey}) failed, reporting unknown: "
                            + $"{ex.GetType().Name}: {ex.Message}");
                    }
                }

                var optDict = new Dictionary<string, object?>
                {
                    ["index"] = i,
                    ["title"] = title,
                    ["description"] = optDesc,
                    ["text_key"] = opt.TextKey,
                    ["is_locked"] = opt.IsLocked,
                    ["will_kill_player"] = willKill,
                    ["vars"] = optVars?.Count > 0 ? optVars : null,
                };
                // sts2-agent fork: disabled options and hover-tip cards (RunSimulator.Events.cs).
                if (_agentMode) AgentEventOptionFields(optDict, localEvent, opt, i);
                return optDict;
            }).ToList();

        // Resolve event name — try ancients table first (for Neow), then events
        var eventEntry = localEvent.Id?.Entry ?? localEvent.GetType().Name.ToUpperInvariant();
        var eventName = _loc.Bilingual("ancients", eventEntry + ".title");
        if (eventName == eventEntry + ".title")
            eventName = _loc.Event(eventEntry);

        // Resolve event description, suppress if key not found
        string? eventDesc = null;
        if (localEvent.Description != null)
        {
            var d = _loc.Bilingual(localEvent.Description.LocTable, localEvent.Description.LocEntryKey);
            if (d != localEvent.Description.LocEntryKey)
                eventDesc = d;
        }

        var page = new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "event_choice",
            ["context"] = RunContext(),
            ["event_name"] = eventName,
            ["description"] = eventDesc,
            ["options"] = options,
            ["player"] = PlayerSummary(_runState!.Players[0]),
        };
        // sts2-agent fork: the event's model id on every page (agent mode), so the client
        // needn't infer it from option keys (TheArchitect's last page has only PROCEED).
        if (_agentMode) page["event_id"] = eventEntry;
        return page;
    }

    private Dictionary<string, object?> RestSiteState(RestSiteRoom restRoom)
    {
        var options = restRoom.Options;
        var player = _runState!.Players[0];
        // sts2-agent fork: the room's own decision, Proceed included (RunSimulator.RestTreasure.cs).
        if (_agentMode) return AgentRestSiteState(restRoom, player);

        if (options == null || options.Count == 0)
        {
            // Options empty = choice already made (synchronizer cleared them), go to map
            Log("Rest site: options empty, proceeding to map");
            ForceToMap();
            return MapSelectState();
        }

        var optionList = options.Select((opt, i) => new Dictionary<string, object?>
        {
            ["index"] = i,
            ["option_id"] = opt.OptionId,
            ["name"] = opt.GetType().Name,
            ["is_enabled"] = opt.IsEnabled,
        }).ToList();

        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "rest_site",
            ["context"] = RunContext(),
            ["options"] = optionList,
            ["player"] = PlayerSummary(player),
        };
    }

    private Dictionary<string, object?> ShopState(MerchantRoom merchantRoom, Player player)
    {
        var inv = merchantRoom.GetLocalInventory();
        if (inv == null) { ForceToMap(); return MapSelectState(); }
        return ShopState(inv, player);
    }

    // sts2-agent fork: the shop over an inventory, so the fake merchant's (RunSimulator.FakeMerchant.cs) shares it.
    private Dictionary<string, object?> ShopState(MerchantInventory inv, Player player)
    {
        var cards = inv.CharacterCardEntries.Concat(inv.ColorlessCardEntries)
            .Select((e, i) =>
            {
                var card = e.CreationResult?.Card;
                var entry = card?.Id.Entry ?? "?";
                var stats = new Dictionary<string, object?>();
                int cardCost = 0;
                try
                {
                    if (card != null)
                    {
                        cardCost = card.EnergyCost?.GetResolved() ?? 0;
                        var mutable = card.ToMutable();
                        foreach (var dv in mutable.DynamicVars.Values)
                            stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue;
                    }
                }
                catch { }
                return new Dictionary<string, object?>
                {
                    ["index"] = i,
                    // Raw entry id, matching RelicOfferDict/PotionOfferDict —
                    // the AI bridge needs the id (the localized name alone
                    // leaves the net's card-identity token OOV).
                    ["id"] = entry,
                    ["name"] = _loc.Card(entry),
                    ["type"] = card?.Type.ToString() ?? "?",
                    ["card_cost"] = cardCost,
                    ["description"] = _loc.Bilingual("cards", entry + ".description"),
                    ["stats"] = stats.Count > 0 ? stats : null,
                    ["after_upgrade"] = card != null ? GetUpgradedInfo(card) : null,
                    ["cost"] = e.Cost,
                    ["is_stocked"] = e.IsStocked,
                    ["on_sale"] = e.IsOnSale,
                };
            }).ToList();

        var relics = inv.RelicEntries.Select((e, i) =>
        {
            var dict = RelicOfferDict(e.Model, i);
            dict["cost"] = e.Cost;
            dict["is_stocked"] = e.IsStocked;
            return dict;
        }).ToList();

        var potions = inv.PotionEntries.Select((e, i) =>
        {
            var dict = PotionOfferDict(e.Model, i);
            dict["cost"] = e.Cost;
            dict["is_stocked"] = e.IsStocked;
            return dict;
        }).ToList();

        var removal = inv.CardRemovalEntry;

        var state = new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "shop",
            ["context"] = RunContext(),
            ["cards"] = cards,
            ["relics"] = relics,
            ["potions"] = potions,
            ["card_removal_cost"] = removal?.Cost,
            ["player"] = PlayerSummary(player),
        };
        // sts2-agent fork: purchasability and reasons per slot, the removal's state (RunSimulator.Shop.cs).
        if (_agentMode) AgentShopFields(state, inv, player);
        return state;
    }

    private Dictionary<string, object?> TreasureState(TreasureRoom treasureRoom)
    {
        // sts2-agent fork: the chest's relic is a decision (RunSimulator.RestTreasure.cs).
        if (_agentMode) return AgentTreasureState(treasureRoom, _runState!.Players[0]);

        // Treasure rooms give relics via TreasureRoomRelicSynchronizer
        Log("Treasure room — collecting rewards");

        // BUG-013: Ensure any pending relic picking session is complete before starting new one
        WaitForActionExecutor();
        _syncCtx.Pump();

        try
        {
            treasureRoom.DoNormalRewards().GetAwaiter().GetResult();
            _syncCtx.Pump();
            CollectTreasureRelic();
            treasureRoom.DoExtraRewardsIfNeeded().GetAwaiter().GetResult();
            _syncCtx.Pump();
        }
        catch (InvalidOperationException ex) when (ex.Message.Contains("relic picking session"))
        {
            // BUG-013: Relic session conflict — wait for pending session then retry
            // NOT LogException: this is a recognised race the code RETRIES, not a
            // swallowed fault. Routing it through LogException would score a handled
            // condition as a world error on every sweep.
            Log($"Relic session conflict, waiting and retrying: {ex.Message}");
            WaitForActionExecutor();
            _syncCtx.Pump();
            try
            {
                treasureRoom.DoNormalRewards().GetAwaiter().GetResult();
                _syncCtx.Pump();
                CollectTreasureRelic();
                treasureRoom.DoExtraRewardsIfNeeded().GetAwaiter().GetResult();
                _syncCtx.Pump();
            }
            // The RETRY failing is not a handled race — it is an unhandled fault that
            // leaves the treasure room's rewards half-applied, so unlike the exemption
            // above it must reach the world-error counter.
            catch (Exception retryEx) { LogException("Treasure rewards retry", retryEx); }
        }
        catch (Exception ex) { LogException("Treasure rewards", ex); }

        ForceToMap();
        return MapSelectState();
    }

    /// <summary>
    /// BUG-016: actually take the chest relic. BeginRelicPicking (fired on
    /// treasure-room enter) already rolled it — RollRarity(TreasureRoomRelics)
    /// + SharedGrabBag.PullFromFront — but the headless never voted, so the
    /// relic evaporated while its RNG reads and grab-bag drain stayed. The
    /// game's own AutoSlay TreasureRoomHandler always opens the chest and
    /// grabs the relic, so the canonical automated play is take-it.
    /// PickRelicLocally(0) closes the voting session (single player: one vote
    /// → AwardRelics over an empty unpicked list, no extra RNG reads →
    /// EndRelicVoting); the RelicsAwarded event's only subscriber is the UI
    /// node, absent headlessly, so the Obtain runs here directly.
    /// </summary>
    private void CollectTreasureRelic()
    {
        try
        {
            var sync = MegaCrit.Sts2.Core.Runs.RunManager.Instance.TreasureRoomRelicSynchronizer;
            var relicsField = sync.GetType().GetField("_currentRelics", NonPublic);
            var relics = relicsField?.GetValue(sync) as List<RelicModel>;
            if (relics == null || relics.Count == 0)
            {
                Log("Treasure relic: no rolled relic to collect");
                return;
            }
            var relic = relics[0].ToMutable();
            sync.PickRelicLocally(0);
            WaitForActionExecutor();
            _syncCtx.Pump();
            RelicCmd.Obtain(relic, _runState!.Players[0]).GetAwaiter().GetResult();
            _syncCtx.Pump();
            Log($"Treasure relic collected: {relic.GetType().Name}");
        }
        catch (Exception ex) { LogException("Treasure relic pick failed", ex); }
    }

    private Dictionary<string, object?> GameOverState(bool isVictory)
    {
        var player = _runState!.Players[0];
        var summary = PlayerSummary(player);
        // BUG-005: When player died, the engine resets HP to max. Use last known HP instead.
        if (!isVictory)
            summary["hp"] = _lastKnownHp > 0 ? 0 : (player.Creature?.CurrentHp ?? 0);
        // sts2-agent fork: a won run also kills the player (WinRun → GuaranteeKillAllPlayers),
        // so its hp read 0; report the hp the run was won with (agent mode).
        else if (_agentMode && player.Creature is { IsDead: true } && _agentHpBeforeCommand > 0)
            summary["hp"] = _agentHpBeforeCommand;
        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "game_over",
            ["context"] = RunContext(),
            ["victory"] = isVictory,
            ["player"] = summary,
            ["act"] = _runState.CurrentActIndex + 1,
            ["floor"] = _runState.ActFloor,
            ["ascension"] = _runState.AscensionLevel,
        };
    }

    #endregion

    #region Helpers

    /// <summary>
    /// Drive a freshly-entered nested event combat (entered via
    /// <c>EventModel.EnterCombatWithoutExitingEvent</c>) to its play phase.
    /// The entry runs <c>RunManager.EnterRoomWithoutExitingCurrentRoom</c>
    /// fire-and-forget, then <c>CombatManager.AfterCombatRoomLoaded</c> kicks
    /// <c>StartCombatInternal</c> (also fire-and-forget) which sets
    /// <c>IsInProgress</c> then <c>StartTurn</c> sets <c>IsPlayPhase</c>. Pump
    /// the GameSyncContext until both are true (or a card selection / combat-end
    /// surfaces) so the existing combat loop can take over. Bead sts2-rs-ck9z6
    /// (GH #166).
    /// </summary>
    private void DriveNestedEventCombatToPlayPhase()
    {
        _syncCtx.Pump();
        WaitForActionExecutor();
        WaitFor(
            // v0.107.1: the global CombatManager.IsPlayPhase bool was removed;
            // use the per-player IsPlayPhase() helper (PlayerCombatState.Phase).
            () => (CombatManager.Instance.IsInProgress && IsPlayPhase())
                || _cardSelector.HasPending,
            5000,
            "nested event combat reaching play phase");
        WaitForActionExecutor();
    }

    private void WaitForActionExecutor()
    {
        try
        {
            // Sync context is already set by the GameThread; no need to set again here.
            _syncCtx.Pump();

            var executor = RunManager.Instance.ActionExecutor;
            // IsRunning can flap to false between phases (e.g. monster move →
            // queued post-combat action), so don't trust the first quiescent
            // moment. Require a few consecutive idle pumps before returning.
            int idleStreak = 0;
            const int idleStreakRequired = 5;
            const int maxPumps = 1000;
            for (int i = 0; i < maxPumps; i++)
            {
                _syncCtx.Pump();
                if (!executor.IsRunning)
                {
                    if (++idleStreak >= idleStreakRequired) break;
                }
                else
                {
                    idleStreak = 0;
                    // sts2-agent fork: an action paused on a player choice keeps the
                    // executor running (IsRunning is its queue task, not complete until
                    // the queue drains) until the client answers, so this ran to the cap:
                    // ~1.2 s per selection-opening card play. Only the client completes
                    // a pending choice; stop waiting once one is pending.
                    if (AnySelectionPending()) break;
                }
                Thread.Sleep(1);
            }
        }
        catch (Exception ex)
        {
            LogException("WaitForActionExecutor exception", ex);
        }
    }

    /// <summary>
    /// Poll <paramref name="condition"/> until it returns true, pumping the sync
    /// context each iteration. Mirrors the AutoSlay <c>WaitHelper.Until</c> pattern
    /// (see decompiled MegaCrit.Sts2.Core.AutoSlay.Helpers.WaitHelper) but adapted
    /// to our manual-pump GameSyncContext rather than Godot ProcessFrame signals.
    /// Returns true if the condition was met, false on timeout.
    /// </summary>
    private bool WaitFor(Func<bool> condition, int timeoutMs = 5000, string? reason = null)
    {
        try
        {
            _syncCtx.Pump();
            if (condition()) return true;
            int elapsed = 0;
            while (elapsed < timeoutMs)
            {
                _syncCtx.Pump();
                if (condition()) return true;
                Thread.Sleep(1);
                elapsed++;
            }
            if (reason != null) Log($"WaitFor timed out after {timeoutMs}ms: {reason}");
            return false;
        }
        catch (Exception ex)
        {
            LogException($"WaitFor exception ({reason})", ex);
            return false;
        }
    }

    /// <summary>
    /// Poll a Task to completion, pumping the sync context each iteration.
    /// Continuations of awaited Tasks are typically posted back to the
    /// GameSyncContext, so a passive Wait would deadlock — this drives them.
    /// Returns true if the task completed before the timeout.
    /// </summary>
    private bool WaitForTask(Task task, int timeoutMs = 5000, string? reason = null)
    {
        var completed = WaitFor(() => task.IsCompleted, timeoutMs, reason);
        // Surface task faults — WaitFor only checked completion, not success.
        if (completed && task.IsFaulted) task.GetAwaiter().GetResult();
        return completed;
    }

    /// <summary>True iff the harness is currently holding a player-facing card
    /// selection / reward / bundle that the client must resolve before the
    /// suspended event-option task can make further progress.</summary>
    private bool AnySelectionPending() =>
        _cardSelector.HasPending || _cardSelector.HasPendingReward || _pendingBundles != null
        || CustomRewardsPending  // sts2-agent fork: rewards offered by an event or relic
        || CrystalSpherePending;  // sts2-agent fork: the Crystal Sphere waiting for a move

    /// <summary>
    /// sts2-agent fork. After a combat selection resolves, the game resumes whatever
    /// awaited it (a card play, possibly played again by One-Two Punch; end-of-turn
    /// work; start-of-turn powers such as Tools of the Trade), and that work can open
    /// another selection. Pump until the next player choice is pending, or combat has
    /// settled (play phase with the executor idle, combat over, or the player dead).
    /// Returns false if neither happened within the cap: the caller must report a
    /// failure, not a decision.
    /// </summary>
    private bool WaitUntilCombatSettlesOrAsks(string reason)
    {
        const int idleStreakRequired = 50;  // pumps of play phase + idle executor
        var executor = RunManager.Instance.ActionExecutor;
        var player = _runState?.Players[0];
        var sw = System.Diagnostics.Stopwatch.StartNew();
        int idleStreak = 0;
        do
        {
            _syncCtx.Pump();
            if (AnySelectionPending()) return true;
            if (!CombatManager.Instance.IsInProgress || player?.Creature == null || player.Creature.IsDead) return true;
            if (IsPlayPhase() && !executor.IsRunning)
            {
                if (++idleStreak >= idleStreakRequired) return true;
            }
            else
            {
                idleStreak = 0;
            }
            Thread.Sleep(1);
        } while (sw.ElapsedMilliseconds < SettleCapMs);
        Log($"WaitUntilCombatSettlesOrAsks: '{reason}' did not settle within {SettleCapMs}ms");
        return false;
    }

    /// <summary>
    /// Deadlock guard for <see cref="WaitUntilCombatSettlesOrAsks"/>, not the expected
    /// exit. <c>STS2_SETTLE_CAP_MS</c> overrides it, for tests of the timeout path only.
    /// </summary>
    private static readonly int SettleCapMs =
        int.TryParse(Environment.GetEnvironmentVariable("STS2_SETTLE_CAP_MS"), out var ms) ? ms : 15000;

    /// <summary>
    /// Pump the sync context until a freshly-launched handler <paramref name="task"/>
    /// (an event option's <c>Chosen()</c>, a rest-site <c>ChooseLocalOption</c>, or a
    /// shop card-removal) reaches a stable point: it has either run to completion or
    /// SUSPENDED at a player decision (card selection / reward / bundle). A task that
    /// parks on a selection NEVER completes on its own, so the terminal condition is
    /// <c>task.IsCompleted || AnySelectionPending()</c> — not a fixed wall-clock
    /// budget.
    ///
    /// This replaces a `WaitFor(..., 1000)` whose short fixed timeout could fire while
    /// the task was still legitimately in flight (a multi-relic chain on the thread
    /// pool can take &gt;1 s before it opens its first card selection). When that
    /// happened, the caller mis-read the not-yet-pending task as "done with no
    /// selection", fell through to a `WaitForTask` that only watches `IsCompleted`,
    /// and the task — once it parked on the selection — was dropped instead of
    /// stashed, leaving its post-selection deck mutations to race the snapshot on the
    /// thread pool (the A5P441 deck-tail nondeterminism). Keeping the wait open while
    /// the task is still running guarantees the selection is observed and the task
    /// stashed/driven.
    ///
    /// The cap below is a deadlock guard only (a genuinely hung handler), not the
    /// expected exit: in the common case we return the instant the task settles.
    /// </summary>
    private void WaitUntilOptionTaskSettles(Task task, string reason, Func<bool>? asks = null)
    {
        const int settleCapMs = 30000;
        asks ??= AnySelectionPending;
        bool settled = WaitFor(() => task.IsCompleted || asks(), settleCapMs, reason);
        if (!settled)
        {
            Log($"WaitUntilOptionTaskSettles: '{reason}' did not settle within {settleCapMs}ms (task still running, no selection surfaced)");
            EngineErrors.Record("host", $"'{reason}' did not settle within {settleCapMs}ms");
        }
    }

    /// <summary>
    /// Drive the stashed event-option <c>Chosen()</c> task (e.g. NeowsBones'
    /// <c>AfterObtained</c>) forward after a card selection / reward / bundle the
    /// client just resolved. The task may re-suspend at a *further* selection
    /// (NeowsBones offers TWO relics via one <c>RewardsSet.Offer()</c>, and each
    /// can pop its own card-reward screen; the niche curse is only added AFTER the
    /// whole Offer await returns).
    ///
    /// Previously every drain site did `task = _pendingEventOptionTask;
    /// _pendingEventOptionTask = null; WaitForTask(task, 2000);` unconditionally —
    /// so when the task re-suspended at the second selection, the 2000ms
    /// `WaitForTask` timed out, the task reference was DROPPED, and the remaining
    /// continuation (second relic's effect + the DEBT curse add) ran free on the
    /// thread pool, racing the snapshot. That is the A5P189 nondeterminism: the
    /// settled deck flips between 11/12/13 (missing HOWL and/or DEBT) run-to-run.
    ///
    /// Fix: pump the task forward, but if it re-suspends at a NEW pending
    /// selection, RE-STASH it (`_pendingEventOptionTask = task`) so the next
    /// `DoSelectCards` / `DoSelectCardReward` / `DoSkipCardReward` / `DoSkipSelect`
    /// continues driving the SAME task. Only treat it as done — and drain to
    /// quiescence — once it actually completes.
    /// </summary>
    /// <summary>
    /// Keep the scoped NGame stub alive for exactly as long as an event option's
    /// <c>Chosen()</c> is still in flight (i.e. running now, or stashed mid-selection).
    /// </summary>
    private void SyncEventOptionInFlight() =>
        YieldPatches.EventOptionInFlight = _pendingEventOptionTask != null;

    /// <summary>
    /// The single condition under which a selection-resolving command drains the
    /// stashed event-option task. Called from all four resolution sites
    /// (<c>DoSelectCards</c> / <c>DoSelectCardReward</c> / <c>DoSkipCardReward</c> /
    /// <c>DoSkipSelect</c>).
    /// </summary>
    /// <remarks>
    /// The condition used to be `CurrentRoom is EventRoom` alone, which LEAKED the
    /// scoped NGame stub. An option can move the room out from under its own stashed
    /// task — <c>EventModel.EnterCombatWithoutExitingEvent</c> is the obvious way in —
    /// and once <c>CurrentRoom</c> is no longer an <c>EventRoom</c> no site drains,
    /// `_pendingEventOptionTask` stays non-null, and `EventOptionInFlight` stays true
    /// for the REST OF THE RUN. That is precisely the global stub
    /// `YieldPatches.EventOptionInFlight` exists to prevent, and it silently re-arms the
    /// `ProgressSaveManager.TryObtainEpochInternal` fault documented on that flag.
    ///
    /// A stashed task therefore drains wherever its selection resolves. The room check
    /// survives only for the task-less case, where `DriveEventOptionTask` degrades to a
    /// pump — worth doing inside an event, not worth doing on every combat card select.
    /// </remarks>
    private void DrainEventOptionTaskIfDue()
    {
        // sts2-agent fork: a reward claim suspended at a selection resumes at the same
        // points (RunSimulator.Rewards.cs). Claims first: they are inner to an event
        // option that offered the rewards being claimed, which resumes once its set
        // is completed or skipped.
        DriveRewardClaimTask();
        if (_pendingEventOptionTask != null || _runState?.CurrentRoom is EventRoom)
            DriveEventOptionTask();
    }

    /// <summary>
    /// Drop a stashed event-option task that can no longer be driven, and take the
    /// scoped NGame stub down with it.
    /// </summary>
    /// <remarks>
    /// The stash invariant is "stashed implies a selection is pending" — both the
    /// launch site and <c>DriveEventOptionTask</c>'s re-stash only park a task that is
    /// waiting on a player decision. If that invariant is broken there is no future
    /// command that will resume the task, so holding the reference buys nothing and
    /// holding the flag costs the whole rest of the run. Abandoning LOUDLY is the point:
    /// the task's post-await effects (deck writes, HP damage) are lost either way, and
    /// a silent leak is what made this class of bug expensive to find.
    /// </remarks>
    private void ReleaseStrandedEventOptionTask(string why)
    {
        if (_pendingEventOptionTask == null)
        {
            SyncEventOptionInFlight(); // cheap: also re-asserts the flag if it drifted
            return;
        }
        Log($"[WARN] Abandoning stashed event-option task ({why}); its post-await effects will not run");
        EngineErrors.Record("host", $"abandoned a stashed event-option task ({why}); its post-await effects will not run");
        _pendingEventOptionTask = null;
        SyncEventOptionInFlight();
    }

    private void DriveEventOptionTask()
    {
        var task = _pendingEventOptionTask;
        _pendingEventOptionTask = null;
        // The stashed continuation is about to run: re-arm the stub for its duration.
        YieldPatches.EventOptionInFlight = task != null;
        if (task == null)
        {
            WaitForActionExecutor();
            _syncCtx.Pump();
            WaitForActionExecutor();
            return;
        }

        // Pump until the task either completes or re-suspends at a fresh
        // selection. The selection check must come first: when the task hits the
        // next GetSelectedCards/GetSelectedCardReward/bundle, that pending state is
        // set BEFORE the await returns a task to the runtime, so it is observable
        // here while task.IsCompleted is still false. Use the same generous settle
        // cap as the initial launch (WaitUntilOptionTaskSettles): a slow continuation
        // between two relic selections must not be dropped on a short timeout.
        WaitUntilOptionTaskSettles(task, "event option task (until next selection or completion)");
        bool completed = task.IsCompleted;

        if (!task.IsCompleted && AnySelectionPending())
        {
            // Re-suspended at another player decision — keep the task so the next
            // resolution drives it. Do NOT block on full completion (that would
            // deadlock against the still-pending selection) and do NOT drop it.
            _pendingEventOptionTask = task;
            WaitForActionExecutor();
            return;
        }

        // Task ran to completion (or faulted) — surface faults, then drain.
        if (task.IsFaulted) { try { task.GetAwaiter().GetResult(); } catch (Exception ex) { LogException("Event option task fault", ex); } }
        if (!completed)
        {
            Log("Event option task did not settle within timeout");
            EngineErrors.Record("host", "event option task did not settle within timeout");
        }
        SyncEventOptionInFlight(); // leg is done — drop the stub before anything else runs
        WaitForActionExecutor();
        _syncCtx.Pump();
        WaitForActionExecutor();
    }

    /// <summary>Compute what a card would look like after upgrading (stats + cost + description).</summary>
    private Dictionary<string, object?>? GetUpgradedInfo(CardModel card)
    {
        if (!card.IsUpgradable) return null;
        try
        {
            var clone = ModelDb.GetById<CardModel>(card.Id).ToMutable();
            // Apply existing upgrades first
            for (int i = 0; i < card.CurrentUpgradeLevel; i++)
            {
                clone.UpgradeInternal();
                clone.FinalizeUpgradeInternal();
            }
            // Apply one more upgrade
            clone.UpgradeInternal();
            clone.FinalizeUpgradeInternal();

            var stats = new Dictionary<string, object?>();
            try { foreach (var dv in clone.DynamicVars.Values) stats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue; } catch { }

            // Compare keywords before/after upgrade
            var oldKws = card.Keywords?.Where(k => k != CardKeyword.None).Select(k => k.ToString()).ToHashSet() ?? new();
            var newKws = clone.Keywords?.Where(k => k != CardKeyword.None).Select(k => k.ToString()).ToHashSet() ?? new();
            var addedKws = newKws.Except(oldKws).ToList();
            var removedKws = oldKws.Except(newKws).ToList();

            return new Dictionary<string, object?>
            {
                ["cost"] = clone.EnergyCost?.GetResolved() ?? 0,
                ["stats"] = stats.Count > 0 ? stats : null,
                ["description"] = _loc.Bilingual("cards", card.Id.Entry + ".description"),
                ["added_keywords"] = addedKws.Count > 0 ? addedKws : null,
                ["removed_keywords"] = removedKws.Count > 0 ? removedKws : null,
            };
        }
        catch { return null; }
    }

    /// <summary>
    /// Player card plays started this turn, bucketed by card type in the
    /// Rust engine's canonical order (Attack=0, Skill=1, Power=2, Curse=3,
    /// Status=4 — sts2-core cards.rs CardType::index). Mirrors the scan
    /// RingingPower.ShouldPlay does over CombatManager history; identical
    /// to the SpireMentor mod's serializer so both wire dialects agree.
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
    /// ChainsOfBinding's once-per-turn gate, recomputed the way the power
    /// tracks it (ChainsOfBindingPower.BeforeCardPlayed sets a private
    /// flag when a non-dupe Bound card is played; afflictions clear only
    /// at turn end, so a history scan reproduces it exactly).
    /// </summary>
    private static bool BoundCardPlayedThisTurn(CombatState? combatState, Creature? playerCreature)
    {
        if (combatState == null || playerCreature == null)
            return false;
        foreach (var entry in CombatManager.Instance.History.CardPlaysStarted)
        {
            if (!entry.HappenedThisTurn(combatState))
                continue;
            if (entry.CardPlay.Card.Owner.Creature != playerCreature)
                continue;
            if (entry.CardPlay.Card.IsDupe)
                continue;
            if (entry.CardPlay.Card.Affliction is MegaCrit.Sts2.Core.Models.Afflictions.Bound)
                return true;
        }
        return false;
    }

    private Dictionary<string, object?> PlayerSummary(Player player)
    {
        var summary = new Dictionary<string, object?>
        {
            ["name"] = _loc.Bilingual("characters", (player.Character?.Id.Entry ?? "IRONCLAD") + ".title"),
            ["hp"] = player.Creature?.CurrentHp ?? 0,
            ["max_hp"] = player.Creature?.MaxHp ?? 0,
            ["block"] = player.Creature?.Block ?? 0,
            ["gold"] = player.Gold,
            // Total belt capacity (filled + empty). Without it a driver cannot
            // tell a full belt from a partially-filled one, and shop buy_potion
            // retries loop forever on TooFull (same failure the live bridge
            // fixed via !Player.HasOpenPotionSlots).
            ["potion_slots"] = player.MaxPotionCount,
            ["relics"] = player.Relics?.Select(r =>
            {
                var vars = new Dictionary<string, object?>();
                try { foreach (var dv in r.DynamicVars.Values) vars[dv.Name] = (int)dv.BaseValue; } catch { }
                return new Dictionary<string, object?>
                {
                    // Entry id alongside the display name: the parity harness
                    // (run_parity._relic_set) compares relic IDENTITY across
                    // engines; the Rust backend emits Id.Entry, so without this
                    // the C# side fell back to the loc TITLE and any relic
                    // whose title differs from its entry words (e.g.
                    // CHOSEN_CHEESE → "The Chosen Cheese") false-diverged.
                    ["id"] = r.Id.Entry,
                    ["name"] = _loc.Relic(r.Id.Entry),
                    ["description"] = _loc.Bilingual("relics", r.Id.Entry + ".description"),
                    ["vars"] = vars.Count > 0 ? vars : null,
                    // The relic's LIVE counter, for the relics that have one.
                    // `vars` above are the canonical thresholds (Nunchaku's
                    // "every 10 attacks"); this is the progress toward them.
                    // `ShowCounter` is the game's own declaration that the
                    // relic tracks something, and `DisplayAmount` is what it
                    // tracks — the private tally is not otherwise reachable.
                    //
                    // Without it every reconstruction restarted the tally at
                    // zero and the mirror skipped whatever it was about to
                    // trigger. On the v10 sweep the counter relics were the
                    // top lift over baseline in the single-field
                    // `$.player.energy` class (188 records where the mirror
                    // sat exactly 1 energy low): Nunchaku 3.0x, Kusarigama
                    // 3.3x, HappyFlower 2.3x.
                    ["counter"] = SafeRelicCounter(r),
                    // ToyBox wax/melt state. A melted relic STAYS in
                    // Player.Relics (so it serializes like any other) but is
                    // filtered out of every hook dispatch
                    // (CombatState.cs:133, RunState.cs:148). Without these
                    // flags the mirror keeps applying relics the world has
                    // disabled: StrikeDummy +3, RuinedHelmet doubling and
                    // IntimidatingHelmet block were all one melt bug across
                    // the ToyBox seeds (1001/1018/1019). Field names match
                    // the live bridge (SimStateExtractor.cs).
                    ["is_wax"] = r.IsWax,
                    ["is_melted"] = r.IsMelted,
                };
            }).ToList(),
            ["potions"] = player.Potions?.Select((p, i) =>
            {
                if (p == null) return null;
                var pvars = new Dictionary<string, object?>();
                try { foreach (var dv in p.DynamicVars.Values) pvars[dv.Name] = (int)dv.BaseValue; } catch { }
                return new Dictionary<string, object?>
                {
                    ["index"] = i,
                    // Registry entry alongside the display name, for the same
                    // reason relics carry `id`: a trace that compares potion
                    // IDENTITY across worlds cannot use the localized title
                    // (the engine names the same potion `AttackPotion`, this
                    // side renders "Attack Potion", and titles that differ
                    // from their entry words diverge outright).
                    //
                    // Deliberately NOT called `id`: `_potion_obs_id` prefers
                    // an `id` key, so adding one there would swap the token
                    // the PPO vocab sees for every potion in the real-DLL arm
                    // and silently re-base every measured sweep. This key is
                    // read only by the run tracer.
                    ["entry"] = p.Id.Entry,
                    ["name"] = _loc.Potion(p.Id.Entry),
                    ["description"] = _loc.Bilingual("potions", p.Id.Entry + ".description"),
                    ["vars"] = pvars.Count > 0 ? pvars : null,
                    ["target_type"] = p.TargetType.ToString(),
                };
            }).Where(x => x != null).ToList(),
            ["deck_size"] = player.Deck?.Cards?.Count(c => c != null) ?? 0,
            ["deck"] = player.Deck?.Cards?.Where(c => c != null).Select(DeckCardDict).ToList(),
        };
        // sts2-agent fork: which potions the game lets you drink now (RunSimulator.Agent.cs).
        if (_agentMode) AgentPotionFields(summary, player);
        return summary;
    }

    /// <summary>A deck card's JSON (the player summary's deck).</summary>
    private Dictionary<string, object?> DeckCardDict(CardModel c)
    {
        var dstats = new Dictionary<string, object?>();
        try { foreach (var dv in c.DynamicVars.Values) dstats[dv.Name.ToLowerInvariant()] = (int)dv.BaseValue; } catch { }
        var dkws = c.Keywords?.Where(k => k != CardKeyword.None).Select(k => k.ToString()).ToList();
        return new Dictionary<string, object?>
        {
            ["id"] = c.Id.ToString(),
            ["name"] = _loc.Card(c.Id.Entry),
            ["cost"] = c.EnergyCost?.GetResolved() ?? 0,
            ["type"] = c.Type.ToString(),
            ["upgraded"] = c.IsUpgraded,
            ["description"] = _loc.Bilingual("cards", c.Id.Entry + ".description"),
            ["stats"] = dstats.Count > 0 ? dstats : null,
            ["keywords"] = dkws?.Count > 0 ? dkws : null,
            ["tinker_time_rider"] = MadScienceRider(c),
            ["after_upgrade"] = GetUpgradedInfo(c),
        };
    }

    /// <summary>Common context added to every decision point.</summary>
    private Dictionary<string, object?> RunContext()
    {
        if (_runState == null) return new();
        var ctx = new Dictionary<string, object?>
        {
            ["act"] = _runState.CurrentActIndex + 1,
            ["act_name"] = _loc.Act(_runState.Act?.Id.Entry ?? "OVERGROWTH"),
            ["floor"] = _runState.ActFloor,
            // Run-wide floor: `floor` above is act-LOCAL (resets each act),
            // but the AI bridge envelope needs the live mod's TotalFloor
            // semantics (SimStateExtractor.cs) for the net's total_floor
            // feature.
            ["total_floor"] = _runState.TotalFloor,
            ["room_type"] = _runState.CurrentRoom?.RoomType.ToString(),
        };

        // Boss encounter info — use BossEncounter?.Id?.Entry
        try
        {
            var bossIdEntry = _runState.Act?.BossEncounter?.Id?.Entry;
            if (!string.IsNullOrEmpty(bossIdEntry))
            {
                var monsterKey = bossIdEntry.EndsWith("_BOSS") ? bossIdEntry[..^5] : bossIdEntry;
                // Handle special mappings
                if (monsterKey == "THE_KIN") monsterKey = "KIN_PRIEST";
                ctx["boss"] = new Dictionary<string, object?>
                {
                    ["id"] = bossIdEntry,
                    ["name"] = _loc.Monster(monsterKey),
                };
            }
        }
        catch { }

        return ctx;
    }

    private static void EnsureModelDbInitialized()
    {
        if (_modelDbInitialized) return;
        _modelDbInitialized = true;

        TestMode.IsOn = true;

        // sts2-agent fork: collect engine errors for agent-mode failure reports, and
        // install a test-only fault if STS2_TEST_FAULT asks for one.
        EngineErrors.Hook();
        TestFaults.Install();
        InstallShopPatches();
        InstallEventResumePatch();
        InstallFoulPotionPatch();

        // v0.107.1: ReflectionHelper.ModTypes now throws if ModManager hasn't been
        // initialized (ModManager.State == None), and the run-start path reads it.
        // ModManager.Initialize short-circuits to State=Skipped (the documented
        // test/nomods path) when TestMode.IsOn — no file IO, returns synchronously
        // before any await — leaving zero mods loaded so ModTypes resolves to empty.
        try
        {
            MegaCrit.Sts2.Core.Modding.ModManager.Initialize(null!, null, null)
                .GetAwaiter().GetResult();
        }
        catch (Exception ex) { Console.Error.WriteLine($"[WARN] ModManager.Initialize: {ex.Message}"); }

        // Sync context is set on the GameThread (not the calling thread).
        // Bootstrap runs on the game thread because it's invoked from
        // _gameThread.Submit(...) at the top of StartRunImpl.

        // Initialize PlatformServices before anything touches PlatformUtil
        try
        {
            // Try to access PlatformUtil to trigger its static init
            // If it fails, it won't be available but most code checks SteamInitializer.Initialized
            var _ = MegaCrit.Sts2.Core.Platform.PlatformUtil.PrimaryPlatform;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] PlatformUtil init: {ex.Message}");
        }

        // Initialize SaveManager with a dummy profile for save/load support
        try { SaveManager.Instance.InitProfileId(0); }
        catch (Exception ex) { Console.Error.WriteLine($"[WARN] SaveManager.InitProfileId: {ex.Message}"); }

        // Initialize progress data for epoch/timeline tracking
        try { SaveManager.Instance.InitProgressData(); }
        catch (Exception ex) { Console.Error.WriteLine($"[WARN] InitProgressData: {ex.Message}"); }

        // Initialize the prefs save with a fresh default (InitPrefsData would
        // LoadPrefs from a disk path that doesn't exist in headless). Without it
        // `SaveManager.Instance.PrefsSave` (=> _prefsSaveManager.Prefs) is NULL,
        // so any card OnPlay / power hook that reads `PrefsSave.FastMode` for
        // animation pacing (Suppress, StormOfSteel's CustomScaledWait, many
        // others) NREs synchronously and aborts mid-play — the card sticks in the
        // PlayPile with its effect unapplied (a headless parity artifact distinct
        // from the VFX-node derefs). Force FastMode=Instant too, so Cmd.Wait /
        // CustomScaledWait short-circuit to no-ops natively (belt-and-suspenders
        // with the Harmony Cmd.Wait patch) and no real-time timers are created.
        try
        {
            SaveManager.Instance.InitPrefsDataForTest();
            SaveManager.Instance.PrefsSave.FastMode = MegaCrit.Sts2.Core.Settings.FastModeType.Instant;
        }
        catch (Exception ex) { Console.Error.WriteLine($"[WARN] InitPrefsDataForTest: {ex.Message}"); }

        // Install the Task.Yield patch but keep SuppressYield=false by default.
        // SuppressYield is toggled to true only during EndTurn to prevent boss fight deadlocks.
        PatchTaskYield();

        // Patch Cmd.Wait to be a no-op in headless mode.
        // Cmd.Wait(duration) is used for UI animations (e.g., PreviewCardPileAdd during
        // Vantom's Dismember move adding Wounds). In headless mode, these never complete
        // because there's no Godot scene tree, causing the ActionExecutor to deadlock.
        PatchCmdWait();

        // Capture-only prefix on the CardSelectCmd entry points: records which
        // entry point + CardSelectorPrefs prompt loc key opened the next card
        // selection (the ICardSelector signature drops both), so card_select
        // decisions carry their semantics ("TO_UPGRADE" vs "TO_DISCARD" ...).
        // Mirrors the SpireMentor mod's CardSelectContextPatch — without it every
        // mid-combat selection the MCTS replay can't answer falls to the
        // random-legal picker instead of the prompt-keyed quality heuristic.
        PatchCardSelectContext();

        // Patch event-leg VFX/audio entry points that crash in headless
        // (PlayerFullscreenHealVfx.Play missing-method, NDebugAudioManager.Instance
        // null-deref) — they abort an event option's `Chosen()` task before its state
        // effect runs, silently dropping it from the recorded oracle trajectory.
        PatchEventVfxAudio();

        // Same failure mode, different singleton: event legs that deref
        // `NGame.Instance` (screen shake/rumble) unguarded. DenseVegetation loses its
        // ambush fight, Amalgamator eats two basics without granting the Ultimate,
        // PunchOff adds the curse without offering the relic.
        PatchEventScreenShake();

        // Patch DecimillipedeSegment.ReattachMove to a headless-safe
        // reimplementation: the original heals via DoReattach() and then
        // calls ChangePhobiaModeTexture, which derefs NCombatRoom.Instance
        // WITHOUT `?.` — an NRE in headless that aborts ExecuteEnemyTurn at
        // the reviving segment's slot (siblings skipped, EndEnemyTurn and the
        // next player-turn start lost). Source of the DECIMILLIPEDE sweep
        // divergence family (seeds 3771/5577/5765/2796/4256/5398).
        PatchDecimillipedeReattach();

        // Patch Vantom.DismemberMove to a headless-safe reimplementation.
        // The real move (Vantom.cs:164) calls `NGame.Instance.DoHitStop(...)`
        // WITHOUT the null-conditional `?.` that every other Vantom VFX call
        // uses — so in headless mode (NGame.Instance == null) it throws an
        // unhandled NullReferenceException mid-enemy-turn, which TaskHelper
        // swallows and leaves combat wedged on the Enemy side (manifesting as
        // the "EndTurn stuck" Vantom artifact). The replacement applies the two
        // STATE effects faithfully (deal DismemberDamage + add 3 Wounds to
        // discard) and drops only the VFX. Bead sts2-rs-34zix.
        PatchVantomDismember();

        // Patch SlumberingBeetle.AfterAddedToRoom: the original derefs
        // NCombatRoom.Instance for sleep VFX after applying Plating+Slumber;
        // null in headless → swallowed NRE wedges combat-start (the
        // SLUMBERING_BEETLE_NORMAL combat never reaches PlayPhase). Bead
        // sts2-rs-ybtmu.
        PatchSlumberingBeetle();

        // Patch TalkCmd.Play to no-op. It spawns a speech-bubble VFX bound to the
        // speaker's scene node, which doesn't exist in headless mode — the deref
        // throws NRE inside the enemy turn (e.g., Kin Priest's RitualMove) and
        // leaves combat stuck mid-Enemy-side, unrecoverable by EndTurn retries.
        PatchTalkCmd();

        // Patch the Kaiser Crab boss background plumbing. Crusher and Rocket
        // route EVERY move + their AfterCurrentHpChanged / BeforeDeath hooks
        // through a private `Background` getter that derefs
        // `NCombatRoom.Instance.Background` WITHOUT a null guard — null in
        // headless, so the FIRST player attack NREs inside
        // Crusher.AfterCurrentHpChanged (swallowed by TaskHelper), aborting
        // OnPlayWrapper after the damage but BEFORE the discard: the played
        // card vanishes into the Play pile forever (coverage-sweep
        // PILE_COUNT: encounter:KAISER_CRAB_BOSS, discard cs=0 rs=1), and
        // every crab move would wedge the same way on its opening
        // `await Background.PlayAttackAnim(...)`. Instead of reimplementing
        // ten moves (the Vantom pattern), stub the single choke point: the
        // two getters return an uninitialized NKaiserCrabBossBackground and
        // its seven anim methods are no-opped, so each move's real STATE
        // effects (damage / Weak+Frail / Strength / block) run unmodified.
        // Also skips the arms' BeforeDeath, which derefs NAudioManager.Instance
        // DIRECTLY (bypassing SfxCmd's NonInteractiveMode guard) — the throw
        // lands inside CreatureCmd.Kill, so killing an arm wedged the whole
        // fight until the driver's stall breaker aborted the run.
        PatchKaiserCrabBackground();

        // Same family again, one level deeper: the Trial event derefs
        // `NEventRoom.Instance.Layout` on the FIRST line of Accept, so the whole event is
        // dead in headless. Also neutralises DoubleDown's abandon-run confirm popup.
        PatchTrialEventRoom();

        // The last event that still threw: CrystalSphere blocks on an interactive Godot
        // minigame screen. Everything behind the screen is plain state, so play it
        // headlessly instead of losing the reward the player already paid for.
        PatchCrystalSphereMinigame();

        // Not a Godot deref at all: TestMode's auto-taker asserts every offered reward was
        // taken, which is false whenever one legitimately declines (full potion belt).
        // Kills 4 of 60 runs' worth of samples across a rest-site and an event call site.
        InstallRewardsSetAutoSelector();

        // Exactly the same death-hook wedge on a different monster: SoulNexus derefs
        // NCombatRoom.Instance unguarded from its Died handler, so KILLING one abandons
        // the card play inside CreatureCmd.Kill before the win-condition check.
        PatchSoulNexusDeathAnim();

        // Patch Whirlwind.OnPlay + FlashOfSteel.OnPlay to headless-safe
        // reimplementations. Both deref combat-room / target scene nodes for
        // VFX BEFORE the damage resolves (Whirlwind: NHorizontalLinesVfx /
        // NSmokyVignetteVfx; FlashOfSteel: NThinSliceVfx.Create(target)) —
        // null in headless → swallowed NRE that aborts OnPlay, so the card
        // deals no damage / draws nothing and vanishes into limbo. The
        // replacements reproduce only the STATE effects (the multi-hit /
        // single attack + draw). Coverage-sweep PILE_COUNT: WHIRLWIND,
        // FLASH_OF_STEEL, CALAMITY_CARD (sts2-rs-q6bix).
        PatchCardOnPlay(
            "MegaCrit.Sts2.Core.Models.Cards.Whirlwind",
            nameof(YieldPatches.WhirlwindOnPlayPrefix),
            "Whirlwind.OnPlay");
        PatchCardOnPlay(
            "MegaCrit.Sts2.Core.Models.Cards.FlashOfSteel",
            nameof(YieldPatches.FlashOfSteelOnPlayPrefix),
            "FlashOfSteel.OnPlay");
        // Slice.OnPlay uses the same NThinSliceVfx.Create(target) deref as
        // FlashOfSteel (null NCombatRoom.Instance in headless), aborting before
        // the 6 damage. Coverage-sweep PILE_COUNT: SLICE, CALAMITY_CARD (which
        // plays a generated Slice). Bead sts2-rs-q6bix.
        PatchCardOnPlay(
            "MegaCrit.Sts2.Core.Models.Cards.Slice",
            nameof(YieldPatches.SliceOnPlayPrefix),
            "Slice.OnPlay");

        // Patch RollingBoulderPower.AfterPlayerTurnStart to the headless-safe
        // DoDamage-direct branch. The real hook (RollingBoulderPower.cs) only
        // takes the direct branch when TestMode.IsOn; otherwise it builds an
        // NRollingBoulderVfx and calls GodotObject.Connect(StringName, Callable,
        // uint) — a method absent from the headless GodotStubs, so it throws
        // MissingMethodException → swallowed → boulder damage never lands AND
        // the enemy turn wedges (nuclear fallback re-runs moves, inflating
        // damage/SLIMED). The replacement reproduces the TestMode branch:
        // DoDamage(HittableEnemies) then SetAmount(Amount + DamageVar(5)).
        // Pairs with the Rust impl in combat_loop::start_player_turn
        // (sts2-rs-ioty0, q6bix PILE_COUNT: ROLLING_BOULDER).
        PatchPowerAfterPlayerTurnStart(
            "MegaCrit.Sts2.Core.Models.Powers.RollingBoulderPower",
            nameof(YieldPatches.RollingBoulderAfterPlayerTurnStartPrefix),
            "RollingBoulderPower.AfterPlayerTurnStart");

        // Patch EntropyPower.AfterPlayerTurnStart to a deterministic,
        // non-interactive transform. The real hook opens an INTERACTIVE
        // CardSelectCmd.FromHand(prefs Amount) to pick which cards to
        // transform — the headless harness can't drive a mid-turn-start
        // card-select, so it wedges EndTurn (nuclear fallback re-runs enemy
        // moves, inflating player damage + SLIMED). The replacement resolves
        // the select deterministically as "the first Amount cards in hand
        // pile order" and TransformToRandom's each off CombatCardSelection —
        // matching the Rust impl in combat_loop::start_player_turn so card
        // identities + RNG stay in lockstep (sts2-rs-isooo, q6bix
        // PILE_COUNT: ENTROPY).
        PatchPowerAfterPlayerTurnStart(
            "MegaCrit.Sts2.Core.Models.Powers.EntropyPower",
            nameof(YieldPatches.EntropyAfterPlayerTurnStartPrefix),
            "EntropyPower.AfterPlayerTurnStart");

        // Per-read RNG tracer (sts2-rs-adsld / shuffle-op cascade diagnosis).
        // Gated on the STS2_TRACE_RNG env var — the Harmony patch is only
        // APPLIED when tracing is requested, so normal sweeps pay ZERO
        // overhead on the hot Rng.NextInt path.
        PatchRngTrace();

        // Initialize localization system (needed for events, cards, etc.)
        InitLocManager();

        var subtypes = MegaCrit.Sts2.Core.Models.AbstractModelSubtypes.All;
        int registered = 0, failed = 0;
        for (int i = 0; i < subtypes.Count; i++)
        {
            try
            {
                ModelDb.Inject(subtypes[i]);
                registered++;
            }
            catch (Exception ex)
            {
                failed++;
                // Only log first few failures to reduce noise
                if (failed <= 5)
                    Console.Error.WriteLine($"[WARN] Failed to register {subtypes[i].Name}: {ex.GetType().Name}: {ex.Message}");
            }
        }
        Console.Error.WriteLine($"[INFO] ModelDb: {registered} registered, {failed} failed out of {subtypes.Count}");

        // Initialize net ID serialization cache (needed for combat actions)
        try
        {
            ModelIdSerializationCache.Init();
            Console.Error.WriteLine("[INFO] ModelIdSerializationCache initialized");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] ModelIdSerializationCache.Init: {ex.Message}");
        }
    }

    private Player? CreatePlayer(string characterName, bool playedSave = false)
    {
        var unlockState = BuildUnlockState(playedSave);
        return characterName.ToLowerInvariant() switch
        {
            "ironclad" => Player.CreateForNewRun<Ironclad>(unlockState, 1uL),
            "silent" => Player.CreateForNewRun<Silent>(unlockState, 1uL),
            "defect" => Player.CreateForNewRun<Defect>(unlockState, 1uL),
            "regent" => Player.CreateForNewRun<Regent>(unlockState, 1uL),
            "necrobinder" => Player.CreateForNewRun<Necrobinder>(unlockState, 1uL),
            _ => null
        };
    }

    /// <summary>
    /// Build the player UnlockState used at run creation.
    ///
    /// Default (<c>playedSave=false</c>): <see cref="UnlockState.all"/> -- every
    /// epoch/encounter unlocked, <c>NumberOfRuns=999999999</c>. Since NEOW_EPOCH's
    /// id is included, <c>IsEpochRevealed&lt;NeowEpoch&gt;()</c> is true, which is
    /// what makes a brand-new headless save open on the Neow blessing altar
    /// (matches a real brand-new player's very first run). This branch is
    /// unchanged from before the played-save opt-in existed -- default behavior
    /// stays byte-identical.
    ///
    /// Opt-in (<c>playedSave=true</c>): mimics a "played" save -- a real save
    /// with many completed runs where the player earned NEOW_EPOCH long ago but
    /// never clicked "Reveal" on it in the Timeline screen. That is a real,
    /// reachable, and apparently common save state: SaveManager.cs:664's
    /// `TryObtainEpochInternal` logs "Player already has Epoch: NEOW_EPOCH" for
    /// any epoch at EpochState &gt;= ObtainedNoSlot (SaveManager.cs:662),
    /// which is satisfied by Obtained/ObtainedNoSlot *and* Revealed alike -- so
    /// that log line alone (seen on the live save used for the 2026-07-11 seed
    /// 12345 parity gate) does not prove NEOW_EPOCH is Revealed. The live run's
    /// OBSERVED behavior (opens directly on map_select, no Neow altar) only
    /// happens when it is NOT Revealed (RunManager.cs:508's
    /// `State.UnlockState.IsEpochRevealed&lt;NeowEpoch&gt;()` requires the state
    /// to be *exactly* Revealed -- UnlockState.cs:150 filters
    /// `e.State == EpochState.Revealed`) -- i.e. it is stuck at Obtained.
    ///
    /// We reproduce that with <see cref="SaveManager.ObtainEpochOverride"/> +
    /// <see cref="SaveManager.GenerateUnlockStateFromProgress"/> -- literally the
    /// same two calls as the debug "Reset Progress" button
    /// (SaveManager.cs:1262 `ResetTimelineProgress`: clear all epochs, then
    /// `ObtainEpochOverride(NEOW_EPOCH, Obtained)`), i.e. the MINIMAL unlock
    /// state a save can have while still satisfying the observed "already has
    /// NEOW_EPOCH but it's not revealed" fact: nothing else obtained,
    /// NumberOfRuns=0 (irrelevant here -- see below). This was verified
    /// empirically, not just derived: an earlier attempt kept every OTHER
    /// epoch revealed (== <c>UnlockState.all</c> minus NEOW_EPOCH alone, on the
    /// theory that "many runs played" should mean "everything else unlocked")
    /// and reproduced the map_select branch + no-Neow-relic deck correctly, but
    /// desynced the RNG stream one step further in: act 1's first encounter
    /// came out as Fuzzy Wurm Crawler instead of the recorded real trail's Twig
    /// Slime(S)/Leaf Slime(M)/Leaf Slime(S). Root cause: RelicGrabBag.Populate
    /// (RelicGrabBag.cs:70, called from RunManager.InitializeNewRun ->
    /// InitializeNewRun's `State.SharedRelicGrabBag.Populate(...)`, BEFORE
    /// GenerateRooms) buckets `SharedRelicPool.GetUnlockedRelics(unlockState)`
    /// by rarity and `UnstableShuffle`s each bucket -- a shuffle's RNG-draw
    /// count scales with bucket size, and bucket size scales with how many
    /// Relic1Epoch..Relic5Epoch are revealed (SharedRelicPool.cs:138-182). A
    /// "keep everything else revealed" UnlockState has a much bigger pool (118
    /// relics) than a save that only ever revealed NEOW_EPOCH's requirement,
    /// so it burns a different number of `State.Rng.UpFront` draws before
    /// act 1's `GenerateRooms` ever runs, desyncing every subsequent encounter
    /// draw. Switching to the minimal (only-Neow-obtained) UnlockState made
    /// the recorded trail match exactly (map node, enemies, and hand -- see
    /// the task's final report). NumberOfRuns lands at 0 for this UnlockState,
    /// which would normally trip Overgrowth.ApplyActDiscoveryOrderModifications'
    /// "first run ever" branch (Overgrowth.cs:112) -- but
    /// EnsureModelDbInitialized sets TestMode.IsOn=true, and
    /// RunManager.ShouldApplyTutorialModifications() short-circuits false
    /// whenever TestMode.IsOn (RunManager.cs:706-708), so headless never
    /// reaches that branch regardless.
    ///
    /// This must be baked into the UnlockState used to *construct the player*
    /// (not just RunState.ExtraFields.StartedWithNeow after the fact): both
    /// RunManager.SetStartedWithNeowFlag() (called from SetUpTest/
    /// InitializeNewRun, RunManager.cs:454) AND Overgrowth.GetUnlockedAncients
    /// (called from RunManager.GenerateRooms -> ActModel.GenerateRooms,
    /// Overgrowth.cs:100-108) read State.UnlockState -- which is the union of
    /// each player's UnlockState (RunState.cs:616) -- so the ancient-room RNG
    /// draw for _rooms.Ancient (ActModel.cs:404) needs to see the SAME "Neow
    /// not revealed" fact GenerateRooms runs on, or its `Rng.NextItem` draw
    /// consumes RNG state that the real save's empty-pool draw does not
    /// (Rng.cs:256-266: NextItem short-circuits WITHOUT calling NextInt when
    /// the source collection is empty), desyncing every RNG draw after it.
    ///
    /// Residual/limitation (documented, not silently assumed away): this is
    /// the MINIMAL save state consistent with what's directly observable from
    /// the run's structure (map branch + deck size). It is not proven to be
    /// the real save's EXACT epoch/relic/card/potion unlock set -- that isn't
    /// recoverable from outside the live save (no equivalent "already has
    /// Epoch X" log line exists for every epoch, and the parity trail can't
    /// distinguish "epoch Y unrevealed" from "epoch Y revealed but its content
    /// never happened to get RNG-picked"). It happens to reproduce the
    /// recorded seed-12345 reference trail's first map node, first encounter,
    /// and opening hand exactly; runs that reach content gated by some OTHER
    /// epoch (e.g. a Relic5Epoch-only relic, or a second character) could
    /// still diverge from a real save that has more actually unlocked.
    /// </summary>
    private static UnlockState BuildUnlockState(bool playedSave)
    {
        if (!playedSave)
            return UnlockState.all;

        // Mirrors SaveManager.cs:1262 ResetTimelineProgress's own two calls
        // (minus the ResetEpochs(), since a fresh InitProgressData() already
        // has no epochs).
        SaveManager.Instance.ObtainEpochOverride(EpochModel.GetId<NeowEpoch>(), EpochState.Obtained);
        return SaveManager.Instance.GenerateUnlockStateFromProgress();
    }

    /// <summary>
    /// Installs a per-read RNG tracer when STS2_TRACE_RNG is set. Postfixes
    /// both <c>Rng.NextInt</c> overloads (emit
    /// <c>RNGTRACE seed=&lt;Seed&gt; ctr=&lt;Counter&gt; max=&lt;max&gt; r=&lt;result&gt;</c>)
    /// and the <c>Rng(uint, string)</c> ctor (emit
    /// <c>RNGMAP seed=&lt;Seed&gt; name=&lt;name&gt;</c> so each Seed can be
    /// mapped back to its named RunRngType stream). The patch is applied ONLY
    /// when the env var is set, keeping the un-traced hot path untouched.
    /// A run of descending-max reads on one Seed reconstructs an
    /// UnstableShuffle of that length, so this captures both per-step read
    /// counts AND shuffle structure for diffing vs the Rust engine.
    /// </summary>
    private static void PatchRngTrace()
    {
        if (string.IsNullOrEmpty(Environment.GetEnvironmentVariable("STS2_TRACE_RNG")))
            return;
        try
        {
            var harmony = new Harmony("sts2headless.rngtrace");
            var rngType = typeof(MegaCrit.Sts2.Core.Random.Rng);

            var next1 = rngType.GetMethod("NextInt", new[] { typeof(int) });
            var post1 = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.NextInt1Postfix));
            if (next1 != null && post1 != null)
                harmony.Patch(next1, postfix: new HarmonyMethod(post1));

            var next2 = rngType.GetMethod("NextInt", new[] { typeof(int), typeof(int) });
            var post2 = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.NextInt2Postfix));
            if (next2 != null && post2 != null)
                harmony.Patch(next2, postfix: new HarmonyMethod(post2));

            var ctor = rngType.GetConstructor(new[] { typeof(uint), typeof(string) });
            var postC = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.RngNameCtorPostfix));
            if (ctor != null && postC != null)
                harmony.Patch(ctor, postfix: new HarmonyMethod(postC));

            // GrabBag.GrabAndRemove (encounter selection in GenerateRooms) uses
            // NextDouble(); shuffles/NextItem use NextInt (above). Trace the
            // float/double/bool paths too so the full run-setup up_front read
            // sequence is visible for run-RNG parity (run_parity M1).
            void PatchPost(string method, Type[] sig, string postName)
            {
                var m = rngType.GetMethod(method, sig);
                var p = typeof(RngTracePatches).GetMethod(postName);
                if (m != null && p != null)
                    harmony.Patch(m, postfix: new HarmonyMethod(p));
            }
            PatchPost("NextBool", Type.EmptyTypes, nameof(RngTracePatches.NextBoolPostfix));
            PatchPost("NextDouble", Type.EmptyTypes, nameof(RngTracePatches.NextDoublePostfix));
            PatchPost("NextDouble", new[] { typeof(double), typeof(double) }, nameof(RngTracePatches.NextDouble2Postfix));
            PatchPost("NextFloat", new[] { typeof(float) }, nameof(RngTracePatches.NextFloatPostfix));
            PatchPost("NextFloat", new[] { typeof(float), typeof(float) }, nameof(RngTracePatches.NextFloat2Postfix));

            // Card-rarity pity tracer (PARITY132). Postfix the three roll
            // entry points so the per-roll pity offset + rolled rarity are
            // visible for diffing vs the Rust `CardRarityOdds.current_value`.
            //   - Roll(type)                       -> pity-updating (Encounter)
            //   - RollWithoutChangingFutureOdds(t) -> shop (no update, honors offset)
            //   - RollWithBaseOdds(type)           -> Other source (no offset/update)
            try
            {
                var rarityType = typeof(MegaCrit.Sts2.Core.Odds.CardRarityOdds);
                var rRoll = rarityType.GetMethod("Roll");
                var preRoll = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.RarityRollPrefix));
                var pRoll = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.RarityRollPostfix));
                if (rRoll != null && preRoll != null && pRoll != null)
                    harmony.Patch(rRoll, prefix: new HarmonyMethod(preRoll), postfix: new HarmonyMethod(pRoll));

                var rBase = rarityType.GetMethod("RollWithBaseOdds");
                var pBase = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.RarityBasePostfix));
                if (rBase != null && pBase != null)
                    harmony.Patch(rBase, postfix: new HarmonyMethod(pBase));

                var rShop = rarityType.GetMethod(
                    "RollWithoutChangingFutureOdds",
                    new[] { typeof(MegaCrit.Sts2.Core.Runs.CardRarityOddsType) });
                var pShop = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.RarityShopPostfix));
                if (rShop != null && pShop != null)
                    harmony.Patch(rShop, postfix: new HarmonyMethod(pShop));
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] Failed to patch rarity tracer: {ex.Message}");
            }

            // Pile tracer (STS2_TRACE_PILES): snapshot piles at each AutoPlay
            // and each discard->draw Shuffle, for the auto-play-from-draw
            // pool-divergence family (#184). Separate gate so plain RNG traces
            // stay uncluttered; reuses the same Harmony instance.
            if (!string.IsNullOrEmpty(Environment.GetEnvironmentVariable("STS2_TRACE_PILES")))
            {
                try
                {
                    var cardCmd = typeof(MegaCrit.Sts2.Core.Commands.CardCmd);
                    var ap = cardCmd.GetMethod("AutoPlay");
                    var apPre = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.AutoPlayPilePrefix));
                    if (ap != null && apPre != null)
                        harmony.Patch(ap, prefix: new HarmonyMethod(apPre));

                    var pileCmd = typeof(MegaCrit.Sts2.Core.Commands.CardPileCmd);
                    var sh = pileCmd.GetMethod("Shuffle");
                    var shPre = typeof(RngTracePatches).GetMethod(nameof(RngTracePatches.ShufflePilePrefix));
                    if (sh != null && shPre != null)
                        harmony.Patch(sh, prefix: new HarmonyMethod(shPre));

                    Console.Error.WriteLine($"[INFO] Pile tracer enabled (AutoPlay:{ap != null}, Shuffle:{sh != null})");
                }
                catch (Exception ex)
                {
                    Console.Error.WriteLine($"[WARN] Failed to patch pile tracer: {ex.Message}");
                }
            }

            Console.Error.WriteLine(
                $"[INFO] RNG tracer enabled (NextInt:{next1 != null && next2 != null}, ctor:{ctor != null}, +double/float/bool, +rarity)");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch RNG tracer: {ex.Message}");
        }
    }

    internal static class RngTracePatches
    {
        public static void NextInt1Postfix(
            MegaCrit.Sts2.Core.Random.Rng __instance, int maxExclusive, int __result)
        {
            Console.Error.WriteLine(
                $"RNGTRACE seed={__instance.Seed} ctr={__instance.Counter} max={maxExclusive} r={__result}");
        }

        public static void NextInt2Postfix(
            MegaCrit.Sts2.Core.Random.Rng __instance, int minInclusive, int maxExclusive, int __result)
        {
            Console.Error.WriteLine(
                $"RNGTRACE seed={__instance.Seed} ctr={__instance.Counter} max={minInclusive}:{maxExclusive} r={__result}");
        }

        public static void RngNameCtorPostfix(
            MegaCrit.Sts2.Core.Random.Rng __instance, string name)
        {
            Console.Error.WriteLine($"RNGMAP seed={__instance.Seed} name={name}");
        }

        // Non-int draws. `max` carries a kind tag (bool/dbl/flt); `r` is the
        // exact IEEE bit pattern so values compare across engines and still
        // parse as an int. The Rust side emits the same shape.
        public static void NextBoolPostfix(
            MegaCrit.Sts2.Core.Random.Rng __instance, bool __result)
        {
            Console.Error.WriteLine(
                $"RNGTRACE seed={__instance.Seed} ctr={__instance.Counter} max=bool r={(__result ? 1 : 0)}");
        }

        public static void NextDoublePostfix(
            MegaCrit.Sts2.Core.Random.Rng __instance, double __result)
        {
            Console.Error.WriteLine(
                $"RNGTRACE seed={__instance.Seed} ctr={__instance.Counter} max=dbl r={BitConverter.DoubleToInt64Bits(__result)}");
        }

        public static void NextDouble2Postfix(
            MegaCrit.Sts2.Core.Random.Rng __instance, double __result)
        {
            Console.Error.WriteLine(
                $"RNGTRACE seed={__instance.Seed} ctr={__instance.Counter} max=dbl2 r={BitConverter.DoubleToInt64Bits(__result)}");
        }

        public static void NextFloatPostfix(
            MegaCrit.Sts2.Core.Random.Rng __instance, float __result)
        {
            Console.Error.WriteLine(
                $"RNGTRACE seed={__instance.Seed} ctr={__instance.Counter} max=flt r={BitConverter.SingleToInt32Bits(__result)}");
        }

        public static void NextFloat2Postfix(
            MegaCrit.Sts2.Core.Random.Rng __instance, float __result)
        {
            Console.Error.WriteLine(
                $"RNGTRACE seed={__instance.Seed} ctr={__instance.Counter} max=flt2 r={BitConverter.SingleToInt32Bits(__result)}");
        }

        // ── Card-rarity pity tracer (PARITY132) ──────────────────────────
        // `RARITYTRACE method=<roll|base|shop> type=<oddsType> offset=<pity>
        //  r=<rarity> after=<pity'>`. `offset` is the pity value going INTO the
        // roll; `after` is the value after the (possible) update. base/shop
        // never mutate pity, so after==offset there.
        public static void RarityRollPrefix(
            MegaCrit.Sts2.Core.Odds.CardRarityOdds __instance, out float __state)
        {
            __state = __instance.CurrentValue;
        }

        public static void RarityRollPostfix(
            MegaCrit.Sts2.Core.Odds.CardRarityOdds __instance,
            MegaCrit.Sts2.Core.Runs.CardRarityOddsType type,
            MegaCrit.Sts2.Core.Entities.Cards.CardRarity __result,
            float __state)
        {
            Console.Error.WriteLine(
                $"RARITYTRACE method=roll type={type} offset={__state} r={__result} after={__instance.CurrentValue}");
        }

        public static void RarityBasePostfix(
            MegaCrit.Sts2.Core.Odds.CardRarityOdds __instance,
            MegaCrit.Sts2.Core.Runs.CardRarityOddsType type,
            MegaCrit.Sts2.Core.Entities.Cards.CardRarity __result)
        {
            Console.Error.WriteLine(
                $"RARITYTRACE method=base type={type} offset={__instance.CurrentValue} r={__result} after={__instance.CurrentValue}");
        }

        public static void RarityShopPostfix(
            MegaCrit.Sts2.Core.Odds.CardRarityOdds __instance,
            MegaCrit.Sts2.Core.Runs.CardRarityOddsType oddsType,
            MegaCrit.Sts2.Core.Entities.Cards.CardRarity __result)
        {
            Console.Error.WriteLine(
                $"RARITYTRACE method=shop type={oddsType} offset={__instance.CurrentValue} r={__result} after={__instance.CurrentValue}");
        }

        // ── Pile tracer (STS2_TRACE_PILES) ───────────────────────────────
        // Emits a per-event pile snapshot keyed by the player's `shuffle`-stream
        // counter so the Rust side can align by the SAME (shuffle ctr, event)
        // ordinal the RNG trace uses. Built to diagnose the auto-play-from-draw
        // family (Cascade / Catastrophe / Havoc / Mayhem / DistilledChaos),
        // where the RNG stream can match yet the draw-pile *pool contents*
        // diverge — invisible to `RNGTRACE` alone (#184). Lines:
        //   PILETRACE evt=ap shufctr=<n> card=<Id[+]> draw=[..] hand=[..] discard=[..] exhaust=[..] play=[..]
        //   PILETRACE evt=shuf shufctr=<n> draw=[..] discard=[..]
        // Card ids use `Id.ToString()` (the folded uppercase name) + a `+`
        // suffix when upgraded — identical to the harness's `*_ids` lists.
        private static string PileStr(
            MegaCrit.Sts2.Core.Entities.Players.Player p,
            MegaCrit.Sts2.Core.Entities.Cards.PileType pt)
        {
            var cards = pt.GetPile(p).Cards;
            return "[" + string.Join(",", cards.Select(c => c.Id.ToString() + (c.IsUpgraded ? "+" : ""))) + "]";
        }

        public static void AutoPlayPilePrefix(CardModel card)
        {
            var p = card.Owner;
            if (p == null)
                return;
            long ctr = p.RunState.Rng.Shuffle.Counter;
            Console.Error.WriteLine(
                $"PILETRACE evt=ap shufctr={ctr} card={card.Id}{(card.IsUpgraded ? "+" : "")} " +
                $"draw={PileStr(p, MegaCrit.Sts2.Core.Entities.Cards.PileType.Draw)} " +
                $"hand={PileStr(p, MegaCrit.Sts2.Core.Entities.Cards.PileType.Hand)} " +
                $"discard={PileStr(p, MegaCrit.Sts2.Core.Entities.Cards.PileType.Discard)} " +
                $"exhaust={PileStr(p, MegaCrit.Sts2.Core.Entities.Cards.PileType.Exhaust)} " +
                $"play={PileStr(p, MegaCrit.Sts2.Core.Entities.Cards.PileType.Play)}");
        }

        public static void ShufflePilePrefix(MegaCrit.Sts2.Core.Entities.Players.Player player)
        {
            if (player == null)
                return;
            long ctr = player.RunState.Rng.Shuffle.Counter;
            Console.Error.WriteLine(
                $"PILETRACE evt=shuf shufctr={ctr} " +
                $"draw={PileStr(player, MegaCrit.Sts2.Core.Entities.Cards.PileType.Draw)} " +
                $"discard={PileStr(player, MegaCrit.Sts2.Core.Entities.Cards.PileType.Discard)}");
        }
    }

    private static void PatchCardSelectContext()
    {
        try
        {
            var harmony = new Harmony("sts2headless.cardselectcontext");
            var t = typeof(MegaCrit.Sts2.Core.Commands.CardSelectCmd);
            var prefsType = typeof(MegaCrit.Sts2.Core.CardSelection.CardSelectorPrefs);
            var prefix = typeof(HeadlessCardSelectContext).GetMethod(
                nameof(HeadlessCardSelectContext.CapturePrefix),
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static);
            var noPrefsPrefix = typeof(HeadlessCardSelectContext).GetMethod(
                nameof(HeadlessCardSelectContext.CaptureNoPrefsPrefix),
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static);
            int patched = 0;
            int failed = 0;
            // One try per patch, not one around both loops: a single failure
            // used to abort every remaining entry point, so one renamed method
            // silently downgraded the whole surface to the random fallback
            // while the banner still printed a plausible-looking count.
            foreach (var m in t.GetMethods(
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static))
            {
                // Harmony binds the `prefs` prefix arg by NAME — v0.107.1
                // verified every prefs-taking CardSelectCmd method declares it
                // as `prefs` (same audit as the SpireMentor mod's patch).
                if (!m.GetParameters().Any(p => p.ParameterType == prefsType))
                    continue;
                try
                {
                    harmony.Patch(m, prefix: new HarmonyMethod(prefix));
                    patched++;
                }
                catch (Exception ex)
                {
                    failed++;
                    Console.Error.WriteLine(
                        $"[WARN] CardSelectCmd.{m.Name} prefs-capture patch failed: {ex.Message}");
                }
            }
            // The two prefs-less entry points with fixed semantics:
            // FromHandForUpgrade (Armaments-style) and FromChooseACardScreen
            // ("choose a card to obtain" screens).
            foreach (var name in new[] { "FromHandForUpgrade", "FromChooseACardScreen" })
            {
                var m = t.GetMethod(name,
                    System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static);
                if (m == null)
                {
                    failed++;
                    // Loud: this is the rename case. A missing fixed-semantics
                    // entry point is not "no selections happen", it is "these
                    // selections are answered at random from now on".
                    Console.Error.WriteLine(
                        $"[WARN] Could not find CardSelectCmd.{name} - its selections will fall back to random");
                    continue;
                }
                try
                {
                    harmony.Patch(m, prefix: new HarmonyMethod(noPrefsPrefix));
                    patched++;
                }
                catch (Exception ex)
                {
                    failed++;
                    Console.Error.WriteLine(
                        $"[WARN] CardSelectCmd.{name} patch failed: {ex.Message}");
                }
            }
            Console.Error.WriteLine(
                $"[INFO] CardSelectCmd context capture patched on {patched} entry points ({failed} failed)");
        }
        catch (Exception ex)
        {
            // Capture-only: a miss just means prompt_key stays null and the
            // Python side keeps its random fallback — never fail startup.
            Console.Error.WriteLine($"[WARN] PatchCardSelectContext: {ex.Message}");
        }
    }

    private static void PatchCmdWait()
    {
        try
        {
            var harmony = new Harmony("sts2headless.cmdwait");
            // Find Cmd.Wait(float) — it's in MegaCrit.Sts2.Core.Commands namespace
            // Find Cmd type via CardPileCmd's assembly (both are in same namespace)
            var cmdPileType = typeof(MegaCrit.Sts2.Core.Commands.CardPileCmd);
            var cmdAsm = cmdPileType.Assembly;
            Type? cmdType = cmdAsm.GetType("MegaCrit.Sts2.Core.Commands.Cmd");
            // If not found by exact name, search by namespace + "Wait" method
            if (cmdType == null)
            {
                foreach (var t in cmdAsm.GetTypes())
                {
                    if (t.Namespace == "MegaCrit.Sts2.Core.Commands")
                    {
                        var waitM = t.GetMethods(System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.DeclaredOnly)
                            .Where(m => m.Name == "Wait").ToList();
                        if (waitM.Count > 0)
                        {
                            cmdType = t;
                            Console.Error.WriteLine($"[INFO] Found Wait() in {t.FullName}");
                            break;
                        }
                    }
                }
            }
            if (cmdType != null)
            {
                var waitMethod = cmdType.GetMethod("Wait",
                    System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static,
                    null, new[] { typeof(float) }, null);
                if (waitMethod != null)
                {
                    var prefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.CmdWaitPrefix),
                        System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                    if (prefix != null)
                    {
                        harmony.Patch(waitMethod, new HarmonyMethod(prefix));
                        Console.Error.WriteLine("[INFO] Patched Cmd.Wait() to no-op (prevents boss fight deadlocks)");
                    }
                }
                else
                {
                    // Try to find any Wait method
                    var methods = cmdType.GetMethods(System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static)
                        .Where(m => m.Name == "Wait").ToList();
                    foreach (var m in methods)
                    {
                        Console.Error.WriteLine($"[INFO] Found Cmd.Wait({string.Join(",", m.GetParameters().Select(p => p.ParameterType.Name))})");
                        var prefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.CmdWaitPrefix),
                            System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                        if (prefix != null)
                        {
                            harmony.Patch(m, new HarmonyMethod(prefix));
                            Console.Error.WriteLine($"[INFO] Patched Cmd.Wait variant");
                        }
                    }
                }

                // Cmd.CustomScaledWait(float, float, bool, CancellationToken) does NOT
                // route through the Wait(float) overload patched above — it awaits the
                // Wait(float, CancellationToken, bool) overload, which is left unpatched
                // and never completes in headless when FastMode != Instant. The event
                // option's `Chosen()` task then hangs at that await, so the continuation
                // AFTER the wait (e.g. JungleMaze "Join Forces" GainGold, UnrestSite
                // "Rest Anyways" AddCursesToDeck(PoorSleep)) is silently dropped from the
                // recorded oracle trajectory — surfaced by the run-parity sweep as Defect
                // RP31 (deck_size cs=11 rs=12) / RP36 (gold cs=276 rs=325), where Rust is
                // correct. No-op CustomScaledWait to Task.CompletedTask like Cmd.Wait so
                // the continuation runs synchronously.
                foreach (var csw in cmdType.GetMethods(
                             System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static)
                         .Where(mm => mm.Name == "CustomScaledWait"))
                {
                    var cswPrefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.CmdWaitPrefix),
                        System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                    if (cswPrefix != null)
                    {
                        harmony.Patch(csw, new HarmonyMethod(cswPrefix));
                        Console.Error.WriteLine("[INFO] Patched Cmd.CustomScaledWait() to no-op (event-option continuation drain)");
                    }
                }
            }
            else
            {
                Console.Error.WriteLine("[WARN] Could not find MegaCrit.Sts2.Core.Commands.Cmd type");
            }
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch Cmd.Wait: {ex.Message}");
        }
    }

    // Several event-option legs run pure-presentation VFX/audio that derefs
    // Godot singletons/methods absent in headless, throwing BEFORE the leg's
    // state effect. The exception is swallowed by DoChooseOption's catch and the
    // event is force-finished, so the effect is silently dropped from the
    // recorded oracle trajectory (the run-parity sweep then flags Rust — which
    // is correct — as the diverging side). No-op the offending VFX/audio.
    private static void PatchEventVfxAudio()
    {
        // NDebugAudioManager.Instance (=> NGame.Instance?.DebugAudio) is null in
        // headless. 15 debug-audio call sites across the game forget the `?.` and do
        // `Instance.Play(...)` UNGUARDED (e.g. JungleMazeAdventure.SafetyInNumbers'
        // "hey.mp3", DollRoom, DenseVegetation), so they NRE and abort the event
        // option's `Chosen()` task before its state effect runs — DoChooseOption
        // swallows the exception and force-finishes the event, silently dropping the
        // effect from the recorded oracle (Defect RP36: GainGold(JoinForcesGold) lost,
        // gold cs=276 rs=325, Rust correct). Play/Stop are pure audio (no game state),
        // so: return a non-null stub from the Instance getter and no-op Play/Stop/
        // StopAll. The 37 already-guarded `Instance?.Play(...)` sites then call the
        // no-op'd Play instead of short-circuiting on null — same observable result,
        // zero state change. (This needs Play to be Harmony-processable, which is why
        // the GodotStubs gained AudioStreamPlayer.VolumeLinear/PitchScale/Bus +
        // GodotObject.Connect(StringName,Callable,uint) — every token in Play's IL must
        // resolve. The sibling heal-VFX crash, Defect RP31, is fixed separately by the
        // GpuParticles2D.Amount stub so PlayerFullscreenHealVfx.Play JIT-compiles; its
        // body is already skipped at runtime by TestMode.IsOn.)
        try
        {
            var harmony = new Harmony("sts2headless.eventvfxaudio");
            var gameAsm = typeof(MegaCrit.Sts2.Core.Commands.CardPileCmd).Assembly;
            var noOp = typeof(YieldPatches).GetMethod(nameof(YieldPatches.NoOpPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);

            var ndType = gameAsm.GetType("MegaCrit.Sts2.Core.Audio.Debug.NDebugAudioManager");
            if (ndType != null && noOp != null)
            {
                // GetUninitializedObject: never runs the ctor (which derefs Godot); only
                // ever a non-null receiver for the no-op'd Play/Stop below.
                YieldPatches.NDebugAudioStub =
                    (MegaCrit.Sts2.Core.Audio.Debug.NDebugAudioManager)
                        System.Runtime.CompilerServices.RuntimeHelpers.GetUninitializedObject(ndType);
                var getInstance = ndType.GetProperty("Instance",
                    System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static)?.GetGetMethod();
                var instPrefix = typeof(YieldPatches).GetMethod(
                    nameof(YieldPatches.NDebugAudioInstancePrefix),
                    System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                if (getInstance != null && instPrefix != null)
                {
                    harmony.Patch(getInstance, new HarmonyMethod(instPrefix));
                }
                foreach (var m in ndType.GetMethods(
                             System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Instance)
                         .Where(m => m.Name is "Play" or "Stop" or "StopAll"))
                {
                    harmony.Patch(m, new HarmonyMethod(noOp));
                }
                Console.Error.WriteLine("[INFO] Patched NDebugAudioManager.Instance/Play/Stop/StopAll (event debug-audio)");
            }
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch event VFX/audio: {ex.Message}");
        }
    }

    // Event legs also reach `NGame.Instance` — the root scene node, null in headless —
    // WITHOUT the `?.` the debug-audio sites (mostly) have, so they NRE mid-`Chosen()`
    // and the leg's remaining state effect is silently dropped. Measured on the 2026-07-28
    // rung-3.5 sweep once stderr was finally being read: this is the single largest
    // source of world throws in the corpus.
    //
    // Four legs in `MegaCrit.Sts2.Core.Models.Events` deref it unguarded, and in every
    // one the throw lands BETWEEN two state effects, so the run continues with half the
    // option applied:
    //
    //   DenseVegetation.Rest     ScreenRumble       heal applied, then the AMBUSH FIGHT
    //                                               is never offered (event force-finished)
    //   Amalgamator.CombineStrikes/Defends
    //                            ScreenShakeTrauma  2 basics REMOVED from the deck, then
    //                                               UltimateStrike/Defend never added
    //   PunchOff.Nab             ScreenShakeTrauma  Injury curse added, then the RELIC
    //                                               REWARD is never offered
    //
    // A no-op prefix on the shake methods alone CANNOT fix this: `NGame.Instance.X(...)`
    // compiles to `call get_Instance` + `callvirt X`, and callvirt null-checks the
    // receiver before entering the (patched) body. The receiver has to be non-null, so —
    // exactly as PatchEventVfxAudio does for NDebugAudioManager — hand out an
    // uninitialized stub and neutralise the surface anything can reach through it.
    private static void PatchEventScreenShake()
    {
        try
        {
            var harmony = new Harmony("sts2headless.eventscreenshake");
            var noOp = typeof(YieldPatches).GetMethod(nameof(YieldPatches.NoOpPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var instPrefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.NGameInstancePrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            if (noOp == null || instPrefix == null)
            {
                Console.Error.WriteLine(
                    "[WARN] Could not find event screen-shake prefix(es) in YieldPatches - NGame.Instance left null");
                return;
            }

            var ngType = typeof(MegaCrit.Sts2.Core.Nodes.NGame);
            // GetUninitializedObject: never runs the ctor (which derefs Godot). Only ever
            // a receiver for the no-op'd members below, so its null fields are never read.
            YieldPatches.NGameStub = (MegaCrit.Sts2.Core.Nodes.NGame)
                System.Runtime.CompilerServices.RuntimeHelpers.GetUninitializedObject(ngType);

            var getInstance = ngType.GetProperty("Instance",
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static)?.GetGetMethod();
            if (getInstance == null)
            {
                Console.Error.WriteLine(
                    "[WARN] Could not find NGame.Instance getter - event screen-shake left null");
                return;
            }

            // Order matters: neutralise the surface BEFORE the getter starts handing out
            // the stub, so no in-flight call can reach an un-patched member.
            //
            // And it is ALL-OR-NOTHING. A partial patch is strictly worse than no patch:
            // if MegaCrit renames one of these members the name filter silently yields
            // fewer of them, and if `get_Instance` were still patched the renamed
            // method's REAL body would run against a GetUninitializedObject receiver
            // (`_screenShake` null) instead of being short-circuited by a null Instance.
            // The banner would have printed a smaller number and looked fine.
            var patched = 0;

            // The screen-shake/rumble family: 7 void presentation methods, no game state.
            // Covers both the unguarded event sites above and the 34 `Instance?.ScreenShake`
            // sites, which previously short-circuited and now call a no-op instead.
            var shakeNames = new[]
            {
                "ScreenShake", "ScreenShakeTrauma", "ScreenRumble", "DoHitStop",
                "SetScreenShakeTarget", "ClearScreenShakeTarget", "SetScreenshakeMultiplier",
            };
            // The computed getters — see the note on NoOpPrefix in YieldPatches. Every
            // OTHER NGame member reachable via `Instance` is an auto-property, which
            // already returns null/default on an uninitialized object, i.e. exactly what
            // the `?.` sites observe today.
            var computedGetterNames = new[]
            {
                "MainMenu", "CurrentRunNode", "LogoAnimation", "ScreenshakeTarget",
            };

            var shakeMethods = ngType.GetMethods(
                    System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Instance)
                .Where(m => shakeNames.Contains(m.Name))
                .ToList();
            var missingShake = shakeNames.Where(n => !shakeMethods.Any(m => m.Name == n)).ToList();

            var computedGetters = computedGetterNames
                .Select(n => (name: n, getter: ngType.GetProperty(n,
                    System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Instance)?.GetGetMethod()))
                .ToList();
            var missingGetters = computedGetters.Where(g => g.getter == null).Select(g => g.name).ToList();

            if (missingShake.Count > 0 || missingGetters.Count > 0)
            {
                Console.Error.WriteLine(
                    "[WARN] Could not find the whole NGame presentation surface (missing methods: "
                    + $"{string.Join(",", missingShake)}; missing getters: {string.Join(",", missingGetters)}) "
                    + "— refusing to patch NGame.Instance, leaving it null");
                return;
            }

            foreach (var m in shakeMethods)
            {
                harmony.Patch(m, new HarmonyMethod(noOp));
                patched++;
            }
            foreach (var (_, getter) in computedGetters)
            {
                harmony.Patch(getter!, new HarmonyMethod(noOp));
                patched++;
            }

            harmony.Patch(getInstance, new HarmonyMethod(instPrefix));
            Console.Error.WriteLine(
                $"[INFO] Patched NGame.Instance + {patched} presentation member(s) (event screen-shake)");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch event screen-shake: {ex.Message}");
        }
    }

    private static void PatchDecimillipedeReattach()
    {
        try
        {
            var harmony = new Harmony("sts2headless.decimillipedereattach");
            // Private instance async method on the shared base class
            // (covers Front/Middle/Back):
            // Task ReattachMove(IReadOnlyList<Creature>).
            var m = typeof(MegaCrit.Sts2.Core.Models.Monsters.DecimillipedeSegment).GetMethod(
                "ReattachMove",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            if (m != null)
            {
                var prefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.DecimillipedeReattachPrefix),
                    System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                if (prefix != null)
                {
                    harmony.Patch(m, new HarmonyMethod(prefix));
                    Console.Error.WriteLine("[INFO] Patched DecimillipedeSegment.ReattachMove (headless-safe: DoReattach heal only, skips unguarded NCombatRoom phobia-texture NRE, sts2-rs-8lwnq)");
                }
            }
            else
            {
                Console.Error.WriteLine("[WARN] Could not find DecimillipedeSegment.ReattachMove");
            }
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch DecimillipedeSegment.ReattachMove: {ex.Message}");
        }
    }

    private static void PatchVantomDismember()
    {
        try
        {
            var harmony = new Harmony("sts2headless.vantomdismember");
            // Private instance async method: Task DismemberMove(IReadOnlyList<Creature>).
            var m = typeof(MegaCrit.Sts2.Core.Models.Monsters.Vantom).GetMethod(
                "DismemberMove",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            if (m != null)
            {
                var prefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.VantomDismemberPrefix),
                    System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                if (prefix != null)
                {
                    harmony.Patch(m, new HarmonyMethod(prefix));
                    Console.Error.WriteLine("[INFO] Patched Vantom.DismemberMove (headless-safe: skips unguarded NGame.Instance.DoHitStop NRE, sts2-rs-34zix)");
                }
            }
            else
            {
                Console.Error.WriteLine("[WARN] Could not find Vantom.DismemberMove");
            }
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch Vantom.DismemberMove: {ex.Message}");
        }
    }

    private static void PatchSlumberingBeetle()
    {
        try
        {
            var harmony = new Harmony("sts2headless.slumberingbeetle");
            // public override async Task AfterAddedToRoom().
            var m = typeof(MegaCrit.Sts2.Core.Models.Monsters.SlumberingBeetle).GetMethod(
                "AfterAddedToRoom",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Public);
            if (m != null)
            {
                var prefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.SlumberingBeetleAfterAddedPrefix),
                    System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                if (prefix != null)
                {
                    harmony.Patch(m, new HarmonyMethod(prefix));
                    Console.Error.WriteLine("[INFO] Patched SlumberingBeetle.AfterAddedToRoom (headless-safe: applies Plating+Slumber, skips NCombatRoom sleep-VFX NRE, sts2-rs-ybtmu)");
                }
            }
            else
            {
                Console.Error.WriteLine("[WARN] Could not find SlumberingBeetle.AfterAddedToRoom");
            }
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch SlumberingBeetle: {ex.Message}");
        }
    }

    /// <summary>
    /// Patch a card's `protected override async Task OnPlay(PlayerChoiceContext,
    /// CardPlay)` with a headless-safe Harmony prefix (see
    /// YieldPatches.*OnPlayPrefix). Generic over the card type so the
    /// VFX-NRE card patches (Whirlwind / FlashOfSteel) share one installer.
    /// </summary>
    private static void PatchCardOnPlay(string cardTypeName, string prefixName, string label)
    {
        try
        {
            var harmony = new Harmony("sts2headless.cardonplay." + label);
            var cardType = Type.GetType(cardTypeName + ", " + typeof(MegaCrit.Sts2.Core.Models.CardModel).Assembly.GetName().Name)
                ?? typeof(MegaCrit.Sts2.Core.Models.CardModel).Assembly.GetType(cardTypeName);
            if (cardType == null)
            {
                Console.Error.WriteLine($"[WARN] Could not find card type {cardTypeName}");
                return;
            }
            var m = cardType.GetMethod(
                "OnPlay",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            if (m == null)
            {
                Console.Error.WriteLine($"[WARN] Could not find {label}");
                return;
            }
            var prefix = typeof(YieldPatches).GetMethod(prefixName,
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            if (prefix != null)
            {
                harmony.Patch(m, new HarmonyMethod(prefix));
                Console.Error.WriteLine($"[INFO] Patched {label} (headless-safe: skips card-VFX node-deref NRE, sts2-rs-q6bix)");
            }
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch {label}: {ex.Message}");
        }
    }

    /// <summary>
    /// Patch a power's `public override async Task AfterPlayerTurnStart(
    /// PlayerChoiceContext, Player)` with a headless-safe Harmony prefix.
    /// Sibling to PatchCardOnPlay for turn-start power hooks whose real
    /// bodies deref Godot scene/signal nodes that are absent in headless
    /// (RollingBoulderPower's NRollingBoulderVfx Connect; EntropyPower's
    /// interactive CardSelectCmd). sts2-rs-ioty0 / sts2-rs-isooo.
    /// </summary>
    private static void PatchPowerAfterPlayerTurnStart(string powerTypeName, string prefixName, string label)
    {
        try
        {
            var harmony = new Harmony("sts2headless.powerturnstart." + label);
            var powerType = typeof(MegaCrit.Sts2.Core.Models.PowerModel).Assembly.GetType(powerTypeName);
            if (powerType == null)
            {
                Console.Error.WriteLine($"[WARN] Could not find power type {powerTypeName}");
                return;
            }
            var m = powerType.GetMethod(
                "AfterPlayerTurnStart",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Public);
            if (m == null)
            {
                Console.Error.WriteLine($"[WARN] Could not find {label}");
                return;
            }
            var prefix = typeof(YieldPatches).GetMethod(prefixName,
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            if (prefix != null)
            {
                harmony.Patch(m, new HarmonyMethod(prefix));
                Console.Error.WriteLine($"[INFO] Patched {label} (headless-safe turn-start power hook, sts2-rs-q6bix)");
            }
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch {label}: {ex.Message}");
        }
    }

    private static void ClearCardSelector()
    {
        // CardSelectCmd.UseSelector throws "A card selector is already active" if its
        // static _selectorStack isn't empty — happens when reloading mid-run.
        try
        {
            var t = typeof(MegaCrit.Sts2.Core.Commands.CardSelectCmd);
            var bf = System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.NonPublic;
            var stack = t.GetField("_selectorStack", bf)?.GetValue(null);
            stack?.GetType().GetMethod("Clear")?.Invoke(stack, null);
        }
        catch (Exception ex) { Console.Error.WriteLine($"[WARN] ClearCardSelector: {ex.Message}"); }
    }


    /// <summary>
    /// Headless-safe Kaiser Crab boss background: stub the private
    /// `Crusher.Background` / `Rocket.Background` getters (the real ones
    /// deref NCombatRoom.Instance — null headless) and no-op every
    /// NKaiserCrabBossBackground anim method invoked on the stub. See the
    /// installer call site comment for the wedge this clears.
    /// </summary>
    // The Kaiser Crab bug is not unique to the Kaiser Crab. `SoulNexus.AfterDeath`
    // (SoulNexus.cs:50-53) is subscribed to `Creature.Died` in `AfterAddedToRoom`, and
    // its body is one unguarded presentation line:
    //
    //     NCombatRoom.Instance.GetCreatureNode(base.Creature)?.SpineAnimation
    //         .SetAnimation("tracks/empty", loop: true, 1);
    //
    // `NCombatRoom.Instance` is null in headless — and note the class's own
    // `BeforeRemovedFromRoom` two lines below writes `NCombatRoom.Instance?.`, so the
    // missing `?.` here is simply an oversight. Unlike the bot monsters
    // (Guardbot/Noisebot/Stabbot/Zapbot/ToughEgg) and SandpitPower, whose identical
    // derefs sit behind the game's own `TestMode.IsOff`, this one has no guard at all.
    //
    // The throw lands in `Creature.InvokeDiedEvent`, i.e. INSIDE
    // `CreatureCmd.KillWithoutCheckingWinCondition` — so the kill is abandoned before
    // `CreatureCmd.Kill` gets to check the win condition, and the whole enclosing
    // `PlayCardAction` unwinds into `TaskHelper.LogTaskExceptions`. Same shape as
    // Rocket/Crusher `BeforeDeath`, same consequence: the card play never resolves and
    // the run continues from a half-applied combat state. Observed on seeds 1011, 1030
    // and 1051 of the 2026-07-28 rung-3.5 corpus, several times per run.
    //
    // Skipping the body also skips its `Died -= AfterDeath` unsubscribe. That is safe:
    // the handler only ever runs presentation, and the creature it is bound to is dead.
    private static void PatchSoulNexusDeathAnim()
    {
        try
        {
            var harmony = new Harmony("sts2headless.soulnexusdeath");
            var noOp = typeof(YieldPatches).GetMethod(nameof(YieldPatches.NoOpPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var m = typeof(MegaCrit.Sts2.Core.Models.Monsters.SoulNexus).GetMethod(
                "AfterDeath",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic,
                null, new[] { typeof(MegaCrit.Sts2.Core.Entities.Creatures.Creature) }, null);
            if (m == null || noOp == null)
            {
                Console.Error.WriteLine("[WARN] Could not find SoulNexus.AfterDeath - death-hook NRE not patched");
                return;
            }
            harmony.Patch(m, new HarmonyMethod(noOp));
            Console.Error.WriteLine("[INFO] Patched SoulNexus.AfterDeath (unguarded NCombatRoom deref in death hook)");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch SoulNexus.AfterDeath: {ex.Message}");
        }
    }

    // CrystalSphere is the one remaining event whose block is a real MINIGAME, not a
    // missing `?.`:
    //
    //     public async Task PlayMinigame() {
    //         if (LocalContext.IsMe(_owner)) {
    //             NCrystalSphereScreen.ShowScreen(this);   // instantiates a .tscn -> throws
    //             await _completionSource.Task;            // blocks on the player
    //             await CompleteMinigame();
    //         }
    //     }
    //
    // Only `ShowScreen` is Godot. Everything else is plain state: the 11x11 grid and its
    // items are built by the CONSTRUCTOR off the event Rng (already run by the time we get
    // here), `CellClicked` -> `ClearCell` -> `item.RevealItem` touch no scene tree, and
    // `CompleteMinigame` just calls `OneOffSynchronizer.DoLocalCrystalSphereRewards`.
    // So the minigame can be played headlessly; it only needs someone to click.
    //
    // Today it throws instead, and — like the NGame event legs — it throws AFTER the price
    // is paid: `PaymentPlan` has already added the Debt curse and `UncoverFuture` has
    // already spent the gold, so the player pays and receives nothing.
    //
    // >>> THE CELL-SELECTION POLICY BELOW IS A MODELLING CHOICE, NOT A FAITHFUL PORT. <<<
    // There is no "correct" answer to record from the DLL: live, a human decides where to
    // dig. What the policy has to get right is the SHAPE of the reward, because items are
    // only revealed once EVERY cell they occupy is clear (`AreAllOccupiedCellsClear`).
    // The fifteen items span six shapes, not two: relic 4x4, rare potion / card reward /
    // curse 2x2, common potion 1x3, big gold 2x1, small gold 1x1. Scattered single clicks
    // would still reveal almost nothing, which is no more faithful than the current throw.
    //
    // So: select the Big tool (the 3x3 sweep the real screen offers) and clear a growing
    // CONTIGUOUS block, ordered so the region stays compact. That reproduces the event's
    // design intent — 3 divinations (UncoverFuture, 50 gold) clear ~26 cells and can never
    // turn up the relic (three 3x3 squares cannot cover a 4x4: each of the target's four
    // columns needs 4 rows, one square supplies 3, so the two outer columns need two
    // squares each); 6 divinations (PaymentPlan, a Debt curse) clear a 9x6 region, which
    // holds 6*3 = 18 axis-aligned 4x4 windows, i.e. the relic is what the curse is buying.
    // Cell choice
    // consumes NO Rng, so the shared streams stay aligned; the number of items revealed —
    // and the ORDER they are revealed in, which drives the sequential `ToReward` calls in
    // `OneOffSynchronizer.OfferCrystalSphereRewards` — does move `Rng` inside
    // DoLocalCrystalSphereRewards, exactly as it does live (`IsDeterministic => false`).
    //
    // NOT anchored on the corners, which is the whole reason for the lattice's offset.
    // The constructor pre-clears a Manhattan-radius-2 diamond around each of the four
    // corners — 6 cells each, 24 total — BEFORE placing items
    // (`CrystalSphereMinigame.cs:117-138`), and `CrystalSphereItem.CanPlaceHere` refuses
    // any already-cleared cell, so those 24 are guaranteed EMPTY. A 3x3 centred on (1,1)
    // therefore clears 3 fresh cells out of 9 and spends 6 clicks where an item can never
    // be. The earlier list started on (1,1), so it spent that dud FIRST, in every run and
    // at both divination counts. A 20k-trial Monte-Carlo replicating PlaceItem /
    // CanPlaceHere / AreAllOccupiedCellsClear scored it at 21 fresh cells and a 27.3%
    // relic rate at 6 divinations, against 53 / 42.5% for this lattice — a ~36% relative
    // under-delivery of the relic on the option the player PAID a curse for, i.e. a
    // systematic bias against PaymentPlan in the one direction that matters.
    //
    // Six entries is the whole list, not a truncation: `CrystalSphere.cs:70,78` are the
    // only two constructions and pass 3 and 6, so a seventh centre is unreachable. If a
    // future build raises the count the sweep simply stops early and warns.
    private static readonly (int X, int Y)[] CrystalSphereSweepCenters =
    {
        // Two rows of three 3x3 blocks: x in 0..8, y in 2..7. Contiguous, corner-free.
        (1, 3), (4, 3), (7, 3),
        (1, 6), (4, 6), (7, 6),
    };

    private static System.Reflection.MethodInfo? CrystalSphereCompleteMinigame;
    private static System.Reflection.FieldInfo? CrystalSphereOwnerField;

    private static void PatchCrystalSphereMinigame()
    {
        try
        {
            var harmony = new Harmony("sts2headless.crystalsphere");
            var gameType = typeof(MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereMinigame);
            var m = gameType.GetMethod("PlayMinigame",
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Instance);
            var prefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.CrystalSpherePlayPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            CrystalSphereCompleteMinigame = gameType.GetMethod("CompleteMinigame",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            CrystalSphereOwnerField = gameType.GetField("_owner",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            // sts2-agent fork: agent mode waits on it as the game's screen does.
            CrystalSphereCompletionField = gameType.GetField("_completionSource",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            if (CrystalSphereCompletionField == null)
                Console.Error.WriteLine("[WARN] Could not find CrystalSphereMinigame._completionSource - agent mode auto-plays it");
            if (m == null || prefix == null)
            {
                Console.Error.WriteLine("[WARN] Could not find CrystalSphereMinigame.PlayMinigame - event left throwing");
                return;
            }
            if (CrystalSphereOwnerField == null)
            {
                // Same refusal as CompleteMinigame below: without the owner field the
                // local-player guard cannot be evaluated, and playing anyway would grant
                // rewards we have not established are ours to grant.
                Console.Error.WriteLine(
                    "[WARN] Could not find CrystalSphereMinigame._owner - auto-play NOT installed");
                return;
            }
            if (CrystalSphereCompleteMinigame == null)
            {
                // Do NOT install the prefix: replacing PlayMinigame without being able
                // to call CompleteMinigame would clear the grid and grant nothing,
                // which is worse than the throw it replaces.
                Console.Error.WriteLine(
                    "[WARN] Could not find CrystalSphereMinigame.CompleteMinigame - auto-play NOT installed");
                return;
            }
            harmony.Patch(m, new HarmonyMethod(prefix));
            Console.Error.WriteLine(
                "[INFO] Patched CrystalSphereMinigame.PlayMinigame (headless auto-play, Big-tool contiguous sweep)");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch CrystalSphere minigame: {ex.Message}");
        }
    }

    internal static async Task PlayCrystalSphereHeadless(
        MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereMinigame game)
    {
        // Preserve the original's guard: PlayMinigame is a no-op for a non-local player
        // (the rewards are granted on that player's own machine). Always true in this
        // single-player harness, but granting another player's rewards would be worse
        // than the bug being fixed.
        //
        // Resolved through the field that was pinned at PATCH time. An unresolvable
        // `_owner` used to leave `owner` null and SKIP the check — the same "silently
        // proceed past a guard we could not evaluate" shape the CompleteMinigame
        // resolution below is careful to refuse. Refuse it here too.
        if (CrystalSphereOwnerField!.GetValue(game) is not MegaCrit.Sts2.Core.Entities.Players.Player owner)
        {
            Console.Error.WriteLine(
                "[WARN] CrystalSphere: _owner did not resolve to a Player - minigame left unplayed");
            return;
        }
        if (!MegaCrit.Sts2.Core.Context.LocalContext.IsMe(owner)) return;

        // sts2-agent fork: in agent mode the client makes the moves, as the player does on
        // the game's screen (RunSimulator.CrystalSphere.cs); then the game completes it.
        if (AgentModeActive && CrystalSphereCompletionField != null)
        {
            await WaitForClientCrystalSphere(game);
            if (CrystalSphereCompleteMinigame!.Invoke(game, null) is Task done) await done;
            return;
        }

        game.SetTool(MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent
            .CrystalSphereMinigame.CrystalSphereToolType.Big);

        var size = game.GridSize;
        foreach (var (cx, cy) in CrystalSphereSweepCenters)
        {
            if (game.DivinationCount <= 0) break;
            if (cx >= size.X || cy >= size.Y) continue;
            // CellClicked decrements DivinationCount and completes the minigame's
            // TaskCompletionSource on the last one.
            await game.CellClicked(game.cells[cx, cy]);
        }

        if (game.DivinationCount > 0)
        {
            // Unreachable in v0.107.1 (the only two counts are 3 and 6). If a future
            // build raises it, say so rather than quietly under-digging: the sweep is
            // sized to the counts the content actually uses.
            Console.Error.WriteLine(
                $"[WARN] CrystalSphere: {game.DivinationCount} divination(s) unspent - "
                + "sweep lattice is shorter than DivinationCount");
        }

        // Resolved at PATCH time, not here: if this ever stops resolving, the sweep
        // would silently uncover the grid and grant nothing — a fresh silent
        // state-drop of exactly the kind this patch exists to remove. Failing to
        // resolve it aborts the patch instead, which the banner test catches.
        if (CrystalSphereCompleteMinigame!.Invoke(game, null) is Task t) await t;
    }

    // The Trial event is entirely DEAD in headless: `Accept` throws on its FIRST line.
    //
    //     private Task Accept() {
    //         if (LocalContext.IsMe(Owner)) NEventRoom.Instance.Layout.RemoveNodesOnPortrait();
    //         ... rng roll, three trial variants, SetEventState(...)      // never reached
    //     }
    //
    // `LocalContext.IsMe` is TRUE in headless (we ARE the local player) — it is not a
    // headless guard, only `TestMode` / `NonInteractiveMode` are. So both `Accept` and
    // `AddVfxAnchoredToPortrait` deref a null `NEventRoom.Instance`, and `Layout` itself
    // is a COMPUTED getter (`_eventContainer.CurrentScene as NEventLayout`) over a null
    // field, so it would throw even with a non-null room. Two levels have to be stubbed.
    //
    // Blast radius is contained the same way PatchEventScreenShake contains NGame's:
    // the `Instance` getter only hands out the stub while `EventOptionInFlight` is set.
    // That matters more here than for NGame, because three sites use `NEventRoom.Instance`
    // as a CONTROL-FLOW predicate (`NCombatRoom` line 365, `NAncientEventLayout` 621,
    // `NGameOverScreen` 1104) — a globally non-null Instance would silently flip them.
    //
    // The argument-evaluation trap that the global NGame stub fell into does NOT apply to
    // the `?.` sites: every `NEventRoom.Instance?.X` reads `VfxContainer`, a plain
    // auto-property (`NEventRoom.cs:148`) and therefore still null on an uninitialized
    // stub — so
    // `((Node)(object)NEventRoom.Instance?.VfxContainer)?.AddChildSafely(NMirrorVfx.Create())`
    // (Reflections, SlipperyBridge) keeps short-circuiting and keeps NOT evaluating its
    // argument, exactly as today.
    //
    // There is one NON-`?.` site whose argument therefore evaluates unconditionally under
    // the stub — `Trial.cs:108`:
    //
    //     NEventRoom.Instance.SetPortrait(PreloadManager.Cache.GetTexture2D(TrialStartedPath))
    //
    // No-opping `SetPortrait` does not stop the argument running. It is safe anyway, and
    // for a reason worth writing down rather than rediscovering: `GodotStubs/Core.cs:296`
    // defines `ResourceLoader.Load<T>(...) => null`, so `AssetCache.GetAsset` yields null
    // instead of throwing. That is load-bearing — if the stub ever gained a real loader,
    // this line would start dereferencing the scene tree. `AddVfxAnchoredToPortrait` is
    // the case where the same shape is NOT safe (`GetScene(path).Instantiate<Node2D>()`
    // would NRE on the null `PackedScene`), which is why it is no-op'd wholesale above.
    private static void PatchTrialEventRoom()
    {
        try
        {
            var harmony = new Harmony("sts2headless.trialeventroom");
            var noOp = typeof(YieldPatches).GetMethod(nameof(YieldPatches.NoOpPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var roomPrefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.NEventRoomInstancePrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var layoutPrefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.NEventRoomLayoutPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            if (noOp == null || roomPrefix == null || layoutPrefix == null)
            {
                Console.Error.WriteLine(
                    "[WARN] Could not find Trial patch prefix(es) in YieldPatches - NEventRoom.Instance left null");
                return;
            }

            var roomType = typeof(MegaCrit.Sts2.Core.Nodes.Rooms.NEventRoom);
            var layoutType = typeof(MegaCrit.Sts2.Core.Nodes.Events.NEventLayout);

            YieldPatches.NEventRoomStub = (MegaCrit.Sts2.Core.Nodes.Rooms.NEventRoom)
                System.Runtime.CompilerServices.RuntimeHelpers.GetUninitializedObject(roomType);
            YieldPatches.NEventLayoutStub = (MegaCrit.Sts2.Core.Nodes.Events.NEventLayout)
                System.Runtime.CompilerServices.RuntimeHelpers.GetUninitializedObject(layoutType);

            var getInstance = roomType.GetProperty("Instance",
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static)?.GetGetMethod();
            var getLayout = roomType.GetProperty("Layout",
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Instance)?.GetGetMethod();
            if (getInstance == null || getLayout == null)
            {
                Console.Error.WriteLine(
                    "[WARN] Could not find NEventRoom.Instance/.Layout getter - Trial left null");
                return;
            }

            // Order matters: neutralise the reachable surface BEFORE either getter starts
            // handing out a stub — and it is ALL-OR-NOTHING for the same reason
            // PatchEventScreenShake is: a member that silently fails to resolve leaves its
            // REAL body running against a GetUninitializedObject receiver, which is
            // strictly worse than the null-Instance short-circuit it replaced. Every
            // member is resolved up front; one missing name refuses the whole patch.
            var targets = new List<(string Label, System.Reflection.MethodBase? Method)>();

            // The only NEventLayout members Trial reaches. All pure presentation.
            foreach (var name in new[] { "RemoveNodesOnPortrait", "AddVfxAnchoredToPortrait", "SetPortrait" })
            {
                targets.Add(($"NEventLayout.{name}", layoutType.GetMethod(name,
                    System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Instance)));
            }

            // NEventRoom.SetPortrait forwards to Layout.SetPortrait. No-op it directly so
            // the forward never happens either.
            targets.Add(("NEventRoom.SetPortrait", roomType.GetMethod("SetPortrait",
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Instance)));

            // Trial.AddVfxAnchoredToPortrait instantiates a VFX SCENE
            // (`PreloadManager.Cache.GetScene(path).Instantiate<Node2D>()`) as the ARGUMENT
            // to the layout call, so no-opping the layout method alone would leave the
            // scene instantiation running. The whole private helper is presentation.
            targets.Add(("Trial.AddVfxAnchoredToPortrait",
                typeof(MegaCrit.Sts2.Core.Models.Events.Trial).GetMethod(
                    "AddVfxAnchoredToPortrait",
                    System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic)));

            // Trial.DoubleDown's ENTIRE body is
            //     NModalContainer.Instance.Add(NAbandonRunConfirmPopup.Create(null));
            // i.e. open the abandon-run CONFIRM dialog — pure UI, zero state effect, and
            // both the receiver and the argument deref the scene tree. Skipping it models
            // the player opening that confirm and cancelling.
            //
            // NOTE this alone does NOT make DoubleDown faithful — it only stops the
            // throw. Live, cancelling the confirm leaves the player ON the Trial REJECT
            // page, still able to take Accept; our harness sees an option that didn't
            // advance the event and force-finishes it, so the run skips the whole event
            // for free — a survival the live game would not have granted.
            //
            // AS OF THIS COMMIT the option is therefore FREE: no-op'd, the leg does not
            // advance the event, the harness force-finishes, and the run skips a Trial
            // that always costs a curse — at zero cost. Nothing here fixes that; it is
            // only not observed because the option is flagged
            // `ThatWillKillPlayerIf(_ => true)` and the game's own auto-player
            // (`AutoSlay.EventRoomHandler`) filters such options out, so no real player
            // reaches it. Serializing `will_kill_player` onto every event option and
            // teaching both Python choosers to withhold flagged ones is the actual fix,
            // and it is NOT in this commit — see the experiment doc's "Still open".
            var doubleDownTaskPrefix = typeof(YieldPatches).GetMethod(
                nameof(YieldPatches.CompletedTaskPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var doubleDown = typeof(MegaCrit.Sts2.Core.Models.Events.Trial).GetMethod(
                "DoubleDown",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);

            var missing = targets.Where(t => t.Method == null).Select(t => t.Label).ToList();
            if (doubleDown == null) missing.Add("Trial.DoubleDown");
            if (doubleDownTaskPrefix == null) missing.Add("YieldPatches.CompletedTaskPrefix");
            if (missing.Count > 0)
            {
                Console.Error.WriteLine(
                    $"[WARN] Could not find Trial patch member(s) ({string.Join(",", missing)}) "
                    + "- refusing to patch NEventRoom.Instance, leaving it null");
                return;
            }

            var patched = 0;
            foreach (var (_, method) in targets)
            {
                harmony.Patch(method!, new HarmonyMethod(noOp));
                patched++;
            }
            harmony.Patch(doubleDown!, new HarmonyMethod(doubleDownTaskPrefix));
            patched++;

            harmony.Patch(getLayout, new HarmonyMethod(layoutPrefix));
            harmony.Patch(getInstance, new HarmonyMethod(roomPrefix));
            Console.Error.WriteLine(
                $"[INFO] Patched NEventRoom.Instance/.Layout + {patched} presentation member(s) (Trial)");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch Trial event-room derefs: {ex.Message}");
        }
    }

    // `RewardsSet.Offer()` in TestMode has no rewards SCREEN, so it auto-takes every
    // reward and then ASSERTS that the set completed:
    //
    //     foreach (var r in Rewards) await sync.SelectLocalReward(r);
    //     if (!sync.IsRewardsSetCompleted(this) && ThrowInTestIfRewardsNotTaken)
    //         throw new InvalidOperationException("The RewardsSet is not complete ...");
    //
    // That assertion is simply WRONG whenever a reward legitimately declines to be
    // taken. `Reward.OnSelect` returns false by design in exactly those cases — the
    // documented one being `PotionReward` with a FULL POTION BELT
    // (PotionProcureFailureReason.TooFull), and `CardReward` / `CardRemovalReward`
    // when the selection is dismissed. A declined reward never sets
    // `SuccessfullySelected`, so `AllRewardsSuccessfullySelected` stays false, the set
    // is never completed, and Offer() throws.
    //
    // Live, this is not an error at all: the player sees the screen, cannot fit the
    // potion, dismisses it, and the set completes as *Skipped*. Only the headless
    // auto-taker turns "declined" into "throw".
    //
    // Measured on the 2026-07-28 rung-3.5 corpus (4 of 60 runs, two call sites, one bug):
    //   seed 1013  TheLegendsWereTrue.SlowlyFindAnExit -> RewardsCmd.OfferCustom
    //              (offers ONE PotionReward; belt full => throw). The throw lands
    //              BEFORE SetEventFinished, so the player takes the event's HP damage
    //              and the event is then force-finished — half-applied, same class as
    //              the NGame event legs.
    //   seeds 1006/1009/1018  HealRestSiteOption.ExecuteRestSiteHeal -> the
    //              Hook.ModifyRestSiteHealRewards reward (DreamCatcher / TinyMailbox)
    //              -> RewardsCmd.OfferCustom, aborting the rest-site option task.
    //
    // The game exposes the seam for precisely this: `RewardsSet.testSelector`. Install
    // one that reproduces the default loop and then SKIPS whatever is left, which is
    // what dismissing the screen does live (`RewardsSetSynchronizer.SkipLocalRewardsSet`,
    // the same call `BeforeLeavingRoom` makes). Behaviour is identical to the default
    // in every non-throwing case.
    private static void InstallRewardsSetAutoSelector()
    {
        try
        {
            MegaCrit.Sts2.Core.Rewards.RewardsSet.testSelector = async set =>
            {
                // sts2-agent fork: in agent mode, a set offered by an event option or a
                // reward claim goes to the client (RunSimulator.Rewards.cs).
                if (SurfaceCustomRewards.Value && AgentCustomRewardsOffer is { } offer)
                {
                    await offer(set);
                    return;
                }

                var sync = MegaCrit.Sts2.Core.Runs.RunManager.Instance?.RewardsSetSynchronizer;
                if (sync == null) return;

                // `.ToList()` is defensive only, and deliberately not justified by
                // `LinkedRewardSet`: its `RemoveReward` mutates its OWN `_rewards`
                // (`LinkedRewardSet.cs:48-51`), not `RewardsSet.Rewards` — the linked set
                // is itself the single entry in `Rewards` — and `new LinkedRewardSet(`
                // appears nowhere in v0.107.1, so `Reward.ParentRewardSet` is always null
                // and that branch never runs. The snapshot is order-identical and costs
                // nothing; it is the line BELOW that earns its keep.
                foreach (var reward in set.Rewards.ToList())
                {
                    // THIS is the load-bearing guard. `CompleteRewardsSet` pops the set
                    // off the synchronizer's stack the moment the last reward is taken
                    // (`RewardsSetSynchronizer.cs:342-345`, `:500-514`), so a further
                    // `SelectLocalReward` throws "not currently viewing any reward set" —
                    // a real throw, in the code path installed to stop a throw.
                    if (sync.IsRewardsSetCompleted(set)) break;
                    await sync.SelectLocalReward(reward);
                }

                if (sync.IsRewardsSetCompleted(set)) return;

                // Something declined (full potion belt is the common case). Skip the
                // remainder exactly as dismissing the rewards screen would.
                sync.SkipLocalRewardsSet();
                if (!sync.IsRewardsSetCompleted(set))
                {
                    // Only reachable if a NESTED set was left open on top of the stack,
                    // in which case we skipped the wrong one. Never observed; log rather
                    // than loop, so it cannot spin.
                    Console.Error.WriteLine(
                        "[WARN] RewardsSet still incomplete after skipping - nested set on stack?");
                }
            };
            Console.Error.WriteLine(
                "[INFO] Installed RewardsSet.testSelector (declined rewards are skipped, not thrown on)");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to install RewardsSet auto-selector: {ex.Message}");
        }
    }

    private static void PatchKaiserCrabBackground()
    {
        try
        {
            var harmony = new Harmony("sts2headless.kaisercrab");
            var flags = System.Reflection.BindingFlags.Instance
                | System.Reflection.BindingFlags.NonPublic
                | System.Reflection.BindingFlags.Public;

            var getterPrefix = typeof(YieldPatches).GetMethod(
                nameof(YieldPatches.KaiserCrabBackgroundGetterPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var deathPrefix = typeof(YieldPatches).GetMethod(
                nameof(YieldPatches.KaiserCrabArmDeathPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            int patched = 0;
            foreach (var owner in new[]
            {
                typeof(MegaCrit.Sts2.Core.Models.Monsters.Crusher),
                typeof(MegaCrit.Sts2.Core.Models.Monsters.Rocket),
            })
            {
                var prop = owner.GetProperty("Background", flags);
                var getter = prop?.GetGetMethod(true);
                if (getter != null && getterPrefix != null)
                {
                    harmony.Patch(getter, new HarmonyMethod(getterPrefix));
                    patched++;
                }
                else
                {
                    Console.Error.WriteLine($"[WARN] Could not find {owner.Name}.Background getter");
                }

                // BeforeDeath bypasses SfxCmd's NonInteractiveMode guard and
                // derefs NAudioManager.Instance directly — the one line in the
                // arm kit the Background stub above does NOT cover, and fatal
                // because it throws inside CreatureCmd.Kill. Pure presentation,
                // so skip it wholesale. See KaiserCrabArmDeathPrefix.
                var beforeDeath = owner.GetMethod(
                    "BeforeDeath",
                    flags,
                    null,
                    new[] { typeof(MegaCrit.Sts2.Core.Entities.Creatures.Creature) },
                    null);
                if (beforeDeath != null && deathPrefix != null)
                {
                    harmony.Patch(beforeDeath, new HarmonyMethod(deathPrefix));
                    patched++;
                }
                else
                {
                    Console.Error.WriteLine($"[WARN] Could not find {owner.Name}.BeforeDeath");
                }
            }

            var bgType = typeof(MegaCrit.Sts2.Core.Nodes.Vfx.Backgrounds.NKaiserCrabBossBackground);
            var taskNoop = typeof(YieldPatches).GetMethod(
                nameof(YieldPatches.KaiserCrabAnimTaskNoopPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var voidNoop = typeof(YieldPatches).GetMethod(
                nameof(YieldPatches.KaiserCrabAnimVoidNoopPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            foreach (var (name, isTask) in new (string, bool)[]
            {
                ("PlayAttackAnim", true),
                ("PlayRightSideChargeUpAnim", true),
                ("PlayRightSideHeavy", true),
                ("PlayRightRecharge", true),
                ("PlayHurtAnim", false),
                ("PlayArmDeathAnim", false),
                ("PlayBodyDeathAnim", false),
            })
            {
                var m = bgType.GetMethod(name, flags);
                var prefix = isTask ? taskNoop : voidNoop;
                if (m != null && prefix != null)
                {
                    harmony.Patch(m, new HarmonyMethod(prefix));
                    patched++;
                }
                else
                {
                    Console.Error.WriteLine($"[WARN] Could not find NKaiserCrabBossBackground.{name}");
                }
            }
            Console.Error.WriteLine($"[INFO] Patched Kaiser Crab background plumbing + arm BeforeDeath ({patched} methods, headless-safe; sts2-rs-01ygn)");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch Kaiser Crab background: {ex.Message}");
        }
    }

    private static void PatchTalkCmd()
    {
        try
        {
            var harmony = new Harmony("sts2headless.talkcmd");
            var asm = typeof(MegaCrit.Sts2.Core.Commands.CardPileCmd).Assembly;
            var talkType = asm.GetType("MegaCrit.Sts2.Core.Commands.TalkCmd");
            if (talkType == null)
            {
                Console.Error.WriteLine("[WARN] Could not find TalkCmd type");
                return;
            }
            var voidPrefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.NoOpPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var taskPrefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.CmdWaitPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            int patched = 0;
            foreach (var m in talkType.GetMethods(
                System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.NonPublic |
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Instance |
                System.Reflection.BindingFlags.DeclaredOnly))
            {
                if (m.Name != "Play") continue;
                var prefix = m.ReturnType == typeof(Task) ? taskPrefix : voidPrefix;
                if (prefix == null) continue;
                try
                {
                    harmony.Patch(m, new HarmonyMethod(prefix));
                    patched++;
                }
                catch (Exception ex) { Console.Error.WriteLine($"[WARN] TalkCmd.Play patch: {ex.Message}"); }
            }
            Console.Error.WriteLine($"[INFO] Patched TalkCmd.Play ({patched} overload(s)) to no-op");
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] PatchTalkCmd: {ex.Message}");
        }
    }

    private static void PatchTaskYield()
    {
        try
        {
            var harmony = new Harmony("sts2headless.yieldpatch");

            // Patch YieldAwaitable.YieldAwaiter.IsCompleted to return true
            // This makes `await Task.Yield()` execute synchronously (continuation runs inline)
            var yieldAwaiterType = typeof(System.Runtime.CompilerServices.YieldAwaitable)
                .GetNestedType("YieldAwaiter");
            if (yieldAwaiterType != null)
            {
                var isCompletedProp = yieldAwaiterType.GetProperty("IsCompleted");
                if (isCompletedProp != null)
                {
                    var getter = isCompletedProp.GetGetMethod();
                    var prefix = typeof(YieldPatches).GetMethod(nameof(YieldPatches.IsCompletedPrefix),
                        System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                    if (getter != null && prefix != null)
                    {
                        harmony.Patch(getter, new HarmonyMethod(prefix));
                        Console.Error.WriteLine("[INFO] Patched Task.Yield() to be synchronous");
                    }
                }
            }
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] Failed to patch Task.Yield: {ex.Message}");
        }
    }

    /// <summary>
    /// Latest CardSelectCmd entry-point context, captured just before the game
    /// calls ICardSelector.GetSelectedCards (whose signature drops it). Written
    /// by the Harmony prefixes below (PatchCardSelectContext); consumed-and-
    /// cleared by HeadlessCardSelector at the top of GetSelectedCards. Game
    /// logic is single-threaded per selection and nested CardSelectCmd calls
    /// share one prompt, so last-write-wins is correct (same reasoning as the
    /// SpireMentor mod's SimCardSelectContext).
    /// </summary>
    internal static class HeadlessCardSelectContext
    {
        public static string? Source;
        public static string? PromptKey;
        public static string? SourceModel;
        /// <summary>
        /// `CardSelectorPrefs.Cancelable` (CardSelectorPrefs.cs:31), captured
        /// verbatim instead of inferred from `MinSelect <= 0`.
        ///
        /// That inference was wrong in both directions. Three sites set
        /// `Cancelable = true` with a non-zero MinSelect (Cook/Smith rest-site
        /// options, RewardSynchronizer), so a skippable smith read as forced;
        /// and `FromChooseACardScreen` passes min=0 regardless of its
        /// `canSkip` flag, so a MANDATORY obtain read as skippable and the
        /// agent could decline a card the game requires it to take.
        /// </summary>
        public static bool? Cancelable;
        /// <summary>
        /// sts2-agent fork: `FromChooseACardScreen`'s `canSkip` (whether the screen shows
        /// its Skip button); null for every other entry point. Agent mode reports it as
        /// `can_skip`, apart from `Cancelable` (the deck screens' Close button), which the
        /// upstream field above merges it into.
        /// </summary>
        public static bool? CanSkip;
        /// <summary>sts2-agent fork: `FromCombatPile`'s pile (`CardPile.Type`); null for every other entry point.</summary>
        public static string? Pile;

        public static (string? source, string? promptKey, string? sourceModel, bool? cancelable, bool? canSkip)
            Consume()
        {
            (string?, string?, string?, bool?, bool?) v = (Source, PromptKey, SourceModel, Cancelable, CanSkip);
            Source = null;
            PromptKey = null;
            SourceModel = null;
            Cancelable = null;
            CanSkip = null;
            Pile = null;
            return v;
        }

        /// <summary>
        /// The `AbstractModel source` argument — "Model that kicked off the
        /// selection" (CardSelectCmd.cs:695). Located by scanning `__args`
        /// rather than bound by name, because only some overloads declare it
        /// and a named prefix parameter that the target lacks fails the patch
        /// outright.
        ///
        /// This is the signal that makes mid-combat selections answerable.
        /// `prompt_key` alone cannot do it: 23 of the 23 combat cards that
        /// open a selection pass `base.SelectionScreenPrompt`, which is the
        /// PER-CARD loc key `cards/&lt;ENTRY&gt;.selectionScreenPrompt`
        /// (CardModel.cs:129) — not one of the fixed `TO_*` keys. So the
        /// semantic tier recognised none of them and they all fell through to
        /// the random picker.
        /// </summary>
        private static string? ModelEntryFrom(object?[]? args)
        {
            if (args == null) return null;
            foreach (var a in args)
            {
                if (a is MegaCrit.Sts2.Core.Models.AbstractModel m)
                {
                    try { return m.Id.Entry; } catch { return null; }
                }
            }
            return null;
        }

        public static void CapturePrefix(
            MegaCrit.Sts2.Core.CardSelection.CardSelectorPrefs prefs,
            object?[]? __args,
            System.Reflection.MethodBase __originalMethod)
        {
            try
            {
                Source = __originalMethod.Name;
                PromptKey = prefs.Prompt.LocEntryKey;
                SourceModel = ModelEntryFrom(__args);
                Cancelable = prefs.Cancelable;
                CanSkip = null;
                Pile = __args?.OfType<MegaCrit.Sts2.Core.Entities.Cards.CardPile>().FirstOrDefault()?.Type.ToString();
            }
            catch { /* capture-only: never break the game's selection flow */ }
        }

        public static void CaptureNoPrefsPrefix(
            object?[]? __args,
            System.Reflection.MethodBase __originalMethod)
        {
            try
            {
                Source = __originalMethod.Name;
                PromptKey = __originalMethod.Name == "FromHandForUpgrade" ? "TO_UPGRADE" : "TO_OBTAIN";
                SourceModel = ModelEntryFrom(__args);
                // FromChooseACardScreen(context, cards, player, bool canSkip)
                // — the only bool in its signature. FromHandForUpgrade has no
                // skip at all.
                Cancelable = __originalMethod.Name == "FromChooseACardScreen"
                    ? (__args?.OfType<bool>().Cast<bool?>().FirstOrDefault() ?? false)
                    : false;
                CanSkip = __originalMethod.Name == "FromChooseACardScreen" ? Cancelable : null;
                Pile = null;
            }
            catch { }
        }
    }

    /// <summary>
    /// Card selector for headless mode — picks first available card for any selection prompt.
    /// Used by cards like Headbutt, Armaments, etc. that need player to choose a card.
    /// </summary>
    /// <summary>
    /// Card selector that creates a pending selection decision point.
    /// When the game needs the player to choose cards (upgrade, remove, transform, bundle pick),
    /// this stores the options and waits for the main loop to provide the answer.
    /// </summary>
    internal class HeadlessCardSelector : MegaCrit.Sts2.Core.TestSupport.ICardSelector
    {
        // Pending card selection — set by game engine, read by main loop
        public List<CardModel>? PendingOptions { get; private set; }
        public int PendingMinSelect { get; private set; }
        public int PendingMaxSelect { get; private set; }
        public string PendingPrompt { get; private set; } = "";
        // Selection semantics from HeadlessCardSelectContext (the CardSelectCmd
        // entry-point capture — the ICardSelector signature itself drops them).
        public string? PendingPromptKey { get; private set; }
        public string? PendingSource { get; private set; }
        // Entry id of the model that opened the selection (the card being
        // played). The only signal that identifies mid-combat selection
        // semantics — see HeadlessCardSelectContext.ModelEntryFrom.
        public string? PendingSourceModel { get; private set; }
        // CardSelectorPrefs.Cancelable verbatim; null when the entry point was
        // not patched (the driver then falls back to the min<=0 guess).
        public bool? PendingCancelable { get; private set; }
        // sts2-agent fork: the choose-a-card screen's canSkip; null for any other entry point.
        public bool? PendingCanSkip { get; private set; }
        // sts2-agent fork: the combat pile a FromCombatPile selection picks from; null for any other.
        public string? PendingPile { get; private set; }
        private TaskCompletionSource<IEnumerable<CardModel>>? _pendingTcs;

        public bool HasPending => _pendingTcs != null && !_pendingTcs.Task.IsCompleted;

        // When set, GetSelectedCards resolves INLINE (returns a completed task)
        // by picking the same option-0 prefix the parity drain loop would,
        // instead of going pending. Used while a combat card-play is being
        // resumed after a prior choice resolution: a chained auto-play
        // (Catastrophe/Havoc/Mayhem) can open a FURTHER choice deep inside the
        // resumed play that the manual pump loop cannot surface in time, so its
        // TCS strands and the play deadlocks with cards stuck in the PlayPile
        // (seed 1279: auto-played Splash's generated-card screen). Resolving it
        // inline matches both the harness's own option-0 draining policy and
        // the Rust engine's inline auto-play choice resolution. Bead to file.
        public bool AutoResolveInline;

        // sts2-agent fork (agent mode): a cancelable prompt with a single candidate stays
        // pending instead of being answered with that card (GetSelectedCards).
        public bool KeepCancelableSingletons;

        public Task<IEnumerable<CardModel>> GetSelectedCards(
            IEnumerable<CardModel> options, int minSelect, int maxSelect)
        {
            // Consume the entry-point context unconditionally: it belongs to
            // THIS selection, so even the early-return paths (empty/single/
            // inline) must clear it or a later unrelated pending selection
            // would carry a stale prompt_key.
            var ctxPile = HeadlessCardSelectContext.Pile;
            (string? ctxSource, string? ctxPromptKey, string? ctxSourceModel, bool? ctxCancelable, bool? ctxCanSkip) =
                HeadlessCardSelectContext.Consume();
            var optList = options.ToList();
            // Pile tracer (STS2_TRACE_PILES): emit the option list IN PRESENTATION
            // ORDER — for a Draw-pile select `CardSelectCmd.FromCombatPile` has
            // already re-sorted it by `orderby c.Rarity, c.Id`, so option[0] is
            // exactly the card the harness's option-0 drain (or AutoResolveInline)
            // picks. Diff vs the Rust select order to nail the draw-select pick
            // key (#184). Card ids use `Id.ToString()` + `+` for upgraded.
            if (!string.IsNullOrEmpty(Environment.GetEnvironmentVariable("STS2_TRACE_PILES")))
            {
                var rendered = string.Join(
                    ",", optList.Select(c => c.Id.ToString() + (c.IsUpgraded ? "+" : "")));
                Console.Error.WriteLine(
                    $"SELECTTRACE options=[{rendered}] min={minSelect} max={maxSelect} inline={AutoResolveInline}");
            }
            if (optList.Count == 0)
                return Task.FromResult<IEnumerable<CardModel>>(Array.Empty<CardModel>());

            // If only one option and minSelect requires it, auto-select
            // sts2-agent fork: not a cancelable prompt in agent mode. The game itself
            // resolves a prompt with no more cards than MinSelect unless it requires
            // manual confirmation (CardSelectCmd.FromDeckForUpgrade and the like), so a
            // singleton that reaches the selector is a screen the player sees; for Smith
            // with one upgradable card (Cancelable, RequireManualConfirmation) its Close
            // button is a real choice, and picking for the player would spend the rest.
            if (optList.Count == 1 && minSelect >= 1 && !(KeepCancelableSingletons && ctxCancelable == true))
                return Task.FromResult<IEnumerable<CardModel>>(optList);

            // Nested-choice inline resolution (see AutoResolveInline above):
            // pick the option-0 prefix [0 .. max(minSelect,1)) — identical to
            // `_drain_csharp_choices`'s `range(max(min_select, 1))` — so a
            // chained auto-play's follow-on choice resolves without stranding.
            if (AutoResolveInline)
            {
                int count = Math.Min(Math.Max(minSelect, 1), optList.Count);
                return Task.FromResult<IEnumerable<CardModel>>(optList.Take(count).ToList());
            }

            // Store pending selection and wait
            PendingOptions = optList;
            PendingMinSelect = minSelect;
            PendingMaxSelect = maxSelect;
            PendingPromptKey = ctxPromptKey;
            PendingSource = ctxSource;
            PendingSourceModel = ctxSourceModel;
            PendingCancelable = ctxCancelable;
            PendingCanSkip = ctxCanSkip;
            PendingPile = ctxPile;
            _pendingTcs = new TaskCompletionSource<IEnumerable<CardModel>>();

            Console.Error.WriteLine($"[SIM] Card selection pending: {optList.Count} options, select {minSelect}-{maxSelect}");

            // Return the task — the main loop will complete it
            return _pendingTcs.Task;
        }

        // sts2-agent fork: clear the pending state BEFORE completing the task.
        // TrySetResult can run the game's continuation inline, and that continuation
        // may open the next selection (a card played again by One-Two Punch, a
        // start-of-turn power after an end-of-turn retain). Clearing afterwards
        // wiped that new prompt out and orphaned its task, and the run wedged.
        public void ResolvePending(IEnumerable<CardModel> selected)
        {
            var tcs = _pendingTcs;
            PendingOptions = null;
            _pendingTcs = null;
            tcs?.TrySetResult(selected);
        }

        public void ResolvePendingByIndices(int[] indices)
        {
            if (PendingOptions == null) return;
            var selected = indices
                .Where(i => i >= 0 && i < PendingOptions.Count)
                .Select(i => PendingOptions[i])
                .ToList();
            ResolvePending(selected);
        }

        public void CancelPending()
        {
            var tcs = _pendingTcs;
            PendingOptions = null;
            _pendingTcs = null;
            tcs?.TrySetResult(Array.Empty<CardModel>());
        }

        // Pending card reward from events (GetSelectedCardReward blocks until resolved)
        public List<MegaCrit.Sts2.Core.Entities.Cards.CardCreationResult>? PendingRewardCards { get; private set; }
        private ManualResetEventSlim? _rewardWait;
        private int _rewardChoice = -1;

        // v0.107.1: ICardSelector.GetSelectedCardReward now returns
        // CardRewardSelection { CardModel? card; CardRewardAlternative? alternative; }
        // instead of CardModel?. The consumer (CardReward.cs) reads `alternative`
        // first, then `card`; both null == skip (num resolves to null). We mirror
        // the old card-or-skip behavior by wrapping the selected card; choosing a
        // CardRewardAlternative is a new 0.107.1 surface left as a follow-up (the
        // headless resolves rewards by card index only).
        public MegaCrit.Sts2.Core.TestSupport.CardRewardSelection GetSelectedCardReward(
            IReadOnlyList<MegaCrit.Sts2.Core.Entities.Cards.CardCreationResult> options,
            IReadOnlyList<CardRewardAlternative> alternatives)
        {
            if (options.Count == 0) return default;  // skip (card=null, alternative=null)

            // Store pending and block until main loop resolves
            PendingRewardCards = options.ToList();
            // sts2-agent fork: CardReward.OnSelect passes the same alternatives list on
            // every pass of its loop, but after a REROLL the game's screen regenerates
            // them without REROLL (CanReroll is now false); do the same.
            PendingRewardAlternatives = alternatives
                .Where(a => !(a.OptionId == "REROLL" && ReferenceEquals(alternatives, _rerolledAlternatives)))
                .ToList();
            _rewardChoice = -1;
            _rewardAlternative = null;
            var wait = _rewardWait = new ManualResetEventSlim(false);

            Console.Error.WriteLine($"[SIM] Card reward pending: {options.Count} cards (blocking)");
            // sts2-agent fork: in agent mode wait for the client's answer however long it
            // takes, instead of turning a slow answer into a silent skip.
            if (WaitForever) wait.Wait();
            else wait.Wait(TimeSpan.FromSeconds(300)); // Wait up to 5 min

            var choice = _rewardChoice;
            var alternative = _rewardAlternative;
            // The resolving thread has already cleared the pending state (see
            // ClearPendingReward); only a timeout leaves it set.
            if (ReferenceEquals(_rewardWait, wait)) ClearPendingReward();

            if (alternative != null)
            {
                if (alternative.OptionId == "REROLL") _rerolledAlternatives = alternatives;
                return new MegaCrit.Sts2.Core.TestSupport.CardRewardSelection { alternative = alternative };
            }
            if (choice >= 0 && choice < options.Count)
                return new MegaCrit.Sts2.Core.TestSupport.CardRewardSelection { card = options[choice].Card };
            return default;  // Skip (both fields null)
        }

        public bool HasPendingReward => PendingRewardCards != null && _rewardWait != null;

        // sts2-agent fork: the card-reward alternatives (Skip, REROLL, SACRIFICE)
        // offered with the pending card reward, chosen by OptionId.
        public List<CardRewardAlternative>? PendingRewardAlternatives { get; private set; }
        private CardRewardAlternative? _rewardAlternative;
        private IReadOnlyList<CardRewardAlternative>? _rerolledAlternatives;
        public bool WaitForever;

        public void ResolveAlternative(string optionId)
        {
            _rewardAlternative = PendingRewardAlternatives?.FirstOrDefault(a => a.OptionId == optionId);
            ClearPendingReward()?.Set();
        }

        public void ResolveReward(int index)
        {
            _rewardChoice = index;
            ClearPendingReward()?.Set();
        }

        public void SkipReward()
        {
            _rewardChoice = -1;
            ClearPendingReward()?.Set();
        }

        // sts2-agent fork: clear the pending reward BEFORE waking the waiting thread
        // (same reason as ResolvePending): otherwise the main thread can still see the
        // answered reward as pending while the game moves on, and report it again.
        private ManualResetEventSlim? ClearPendingReward()
        {
            var wait = _rewardWait;
            PendingRewardCards = null;
            PendingRewardAlternatives = null;
            _rewardWait = null;
            return wait;
        }
    }

    internal static class YieldPatches
    {
        /// <summary>
        /// Replaces Crusher/Rocket's private `Background` getter: returns an
        /// uninitialized NKaiserCrabBossBackground so the callers' anim
        /// invocations dispatch onto the no-opped methods below instead of
        /// NRE-ing on `NCombatRoom.Instance.Background` (sts2-rs-01ygn).
        /// </summary>
        public static bool KaiserCrabBackgroundGetterPrefix(
            ref MegaCrit.Sts2.Core.Nodes.Vfx.Backgrounds.NKaiserCrabBossBackground __result)
        {
            __result = (MegaCrit.Sts2.Core.Nodes.Vfx.Backgrounds.NKaiserCrabBossBackground)
                System.Runtime.CompilerServices.RuntimeHelpers.GetUninitializedObject(
                    typeof(MegaCrit.Sts2.Core.Nodes.Vfx.Backgrounds.NKaiserCrabBossBackground));
            return false; // skip original (would deref null NCombatRoom.Instance)
        }

        public static bool KaiserCrabAnimTaskNoopPrefix(ref Task __result)
        {
            __result = Task.CompletedTask;
            return false;
        }

        public static bool KaiserCrabAnimVoidNoopPrefix()
        {
            return false; // skip original anim body
        }

        /// <summary>
        /// Replaces Crusher/Rocket.BeforeDeath (the Kaiser Crab arms).
        ///
        /// The body is `NAudioManager.Instance.PlayOneShot(DeathSfx)` +
        /// PlayArmDeathAnim/PlayBodyDeathAnim + a music parameter — pure
        /// presentation, ZERO game state, so skipping it outright is exactly
        /// faithful. The anims and the `Background` getter are already no-opped
        /// by PatchKaiserCrabBackground; the surviving NRE is the audio call,
        /// which these two methods make DIRECTLY on `NAudioManager.Instance`
        /// (null headless) instead of going through `SfxCmd`, whose every
        /// overload guards on `NonInteractiveMode.IsActive`. Crusher/Rocket are
        /// the only reachable unguarded `NAudioManager.Instance.` derefs in the
        /// build — the rest sit behind that guard, behind `TestMode.IsOff`, or
        /// on Bestiary screens that never instantiate in a run.
        ///
        /// Landing inside `CreatureCmd.KillWithoutCheckingWinCondition` is what
        /// made this fatal rather than cosmetic: TaskHelper.LogTaskExceptions
        /// swallows the throw, the kill never completes, and combat wedges —
        /// every card reads back "still in hand after action" and end_turn
        /// no-ops until the driver's cyclic-stall breaker aborts the run
        /// (seeds 1038/1045, docs/experiments/2026-07-27-rung35-winrate-v20.md).
        /// </summary>
        public static bool KaiserCrabArmDeathPrefix(ref Task __result)
        {
            __result = Task.CompletedTask;
            return false; // skip original (unguarded NAudioManager.Instance deref)
        }

        // Non-null stub returned by the patched NDebugAudioManager.Instance getter so
        // the 15 unguarded `Instance.Play(...)` debug-audio calls in event legs don't
        // NRE in headless (real Instance => NGame.Instance?.DebugAudio == null). Built
        // via GetUninitializedObject in PatchEventVfxAudio; only ever a receiver for the
        // no-op Play/Stop/StopAll prefixes, so its uninitialized fields are never read.
        public static MegaCrit.Sts2.Core.Audio.Debug.NDebugAudioManager NDebugAudioStub = null!;

        public static bool NDebugAudioInstancePrefix(
            ref MegaCrit.Sts2.Core.Audio.Debug.NDebugAudioManager __result)
        {
            __result = NDebugAudioStub;
            return false; // skip original (null in headless)
        }

        // Same trick for NGame: a non-null receiver so the UNGUARDED
        // `NGame.Instance.ScreenShakeTrauma/ScreenRumble(...)` calls in event legs
        // don't NRE. See PatchEventScreenShake for why a no-op prefix on the shake
        // methods alone is not enough.
        public static MegaCrit.Sts2.Core.Nodes.NGame NGameStub = null!;

        /// <summary>
        /// Set only while an event option's <c>Chosen()</c> is being driven.
        /// </summary>
        /// <remarks>
        /// A non-null <c>NGame.Instance</c> cannot be handed out globally, because it
        /// stops `?.` sites from SHORT-CIRCUITING — and in C# that also stops their
        /// ARGUMENTS from being evaluated. `ProgressSaveManager.TryObtainEpochInternal`
        /// (line 668) is the proof:
        ///     ((Node)(object)NGame.Instance)?.AddChildSafely(NGainEpochVfx.Create(epoch));
        /// With a null Instance the whole expression is skipped. With a stub, the
        /// argument runs, `NGainEpochVfx.Create` derefs the scene tree, and the throw
        /// abandons `UpdateAfterCombatWon` mid-way — a fault the stub INTRODUCED, caught
        /// on the 2026-07-28 re-run of seeds 1011/1030/1051. Scoping the stub to event
        /// resolution keeps every other `?.` site behaving exactly as it does today.
        /// </remarks>
        public static volatile bool EventOptionInFlight;

        public static bool NGameInstancePrefix(ref MegaCrit.Sts2.Core.Nodes.NGame? __result)
        {
            if (!EventOptionInFlight) return true; // outside events: keep the real (null) Instance
            __result = NGameStub;
            return false;
        }

        // Two-level stub for Trial: `NEventRoom.Instance.Layout.RemoveNodesOnPortrait()`
        // needs BOTH a non-null room and a non-null layout, because `Layout` is a computed
        // getter over a null `_eventContainer`. See PatchTrialEventRoom.
        public static MegaCrit.Sts2.Core.Nodes.Rooms.NEventRoom NEventRoomStub = null!;
        public static MegaCrit.Sts2.Core.Nodes.Events.NEventLayout NEventLayoutStub = null!;

        /// <summary>
        /// Scoped exactly like <see cref="NGameInstancePrefix"/>, and for a sharper reason:
        /// three sites branch on <c>NEventRoom.Instance == null</c> as a control-flow
        /// predicate, so a globally non-null Instance would silently change behaviour
        /// outside events.
        /// </summary>
        public static bool NEventRoomInstancePrefix(
            ref MegaCrit.Sts2.Core.Nodes.Rooms.NEventRoom? __result)
        {
            if (!EventOptionInFlight) return true; // outside events: keep the real (null) Instance
            __result = NEventRoomStub;
            return false;
        }

        public static bool NEventRoomLayoutPrefix(
            ref MegaCrit.Sts2.Core.Nodes.Events.NEventLayout? __result)
        {
            if (!EventOptionInFlight) return true;
            __result = NEventLayoutStub;
            return false;
        }

        /// <summary>
        /// Replaces <c>CrystalSphereMinigame.PlayMinigame</c>, whose only headless-hostile
        /// line is <c>NCrystalSphereScreen.ShowScreen</c>. See PatchCrystalSphereMinigame
        /// for why the cell-selection policy is a modelling choice.
        /// </summary>
        public static bool CrystalSpherePlayPrefix(
            MegaCrit.Sts2.Core.Events.Custom.CrystalSphereEvent.CrystalSphereMinigame __instance,
            ref Task __result)
        {
            __result = RunSimulator.PlayCrystalSphereHeadless(__instance);
            return false;
        }

        /// <summary>
        /// Prefix for a <c>Task</c>-returning method whose body is pure presentation:
        /// skip it and hand back a completed task. <see cref="NoOpPrefix"/> would leave
        /// <c>__result</c> null, and the caller <c>await</c>s it.
        /// </summary>
        public static bool CompletedTaskPrefix(ref Task __result)
        {
            __result = Task.CompletedTask;
            return false;
        }

        // NGame's COMPUTED getters (MainMenu / CurrentRunNode / LogoAnimation /
        // ScreenshakeTarget) dereference RootSceneContainer / _screenShake, both null on
        // the stub, so handing out a non-null Instance would turn today's safe
        // `NGame.Instance?.MainMenu` short-circuit into a FRESH NRE. They are patched
        // with NoOpPrefix, which is enough on its own: Harmony initialises `__result` to
        // default(T) before the prefix runs, so skipping the original yields null —
        // exactly what the `?.` sites see today.

        // Only suppress Task.Yield() when this flag is set (during end_turn processing)
        public static volatile bool SuppressYield;

        public static bool IsCompletedPrefix(ref bool __result)
        {
            if (SuppressYield)
            {
                __result = true;
                return false;
            }
            return true; // Let normal Yield behavior run
        }

        /// <summary>Harmony prefix: make Cmd.Wait() return completed task immediately (no-op in headless).</summary>
        public static bool CmdWaitPrefix(ref Task __result)
        {
            __result = Task.CompletedTask;
            return false; // Skip original method
        }

        /// <summary>Harmony prefix for void-returning methods: skip the original.</summary>
        public static bool NoOpPrefix() => false;

        /// <summary>
        /// Harmony prefix: replace Vantom.DismemberMove with a headless-safe
        /// version. The original derefs `NGame.Instance.DoHitStop(...)` without
        /// `?.` (Vantom.cs:164); NGame.Instance is null in headless → NRE that
        /// wedges the enemy turn. We reproduce only the STATE effects.
        /// Bead sts2-rs-34zix.
        /// </summary>
        /// <summary>
        /// Harmony prefix: replace DecimillipedeSegment.ReattachMove with a
        /// headless-safe version. The original awaits ReattachPower.DoReattach
        /// (the revive heal — the only STATE effect) and then calls
        /// ChangePhobiaModeTexture, which derefs NCombatRoom.Instance without
        /// a null guard — NRE in headless that aborts the enemy walk at the
        /// reviving segment's slot. Reproduce only the heal. sts2-rs-8lwnq.
        /// </summary>
        public static bool DecimillipedeReattachPrefix(
            object __instance,
            System.Collections.Generic.IReadOnlyList<MegaCrit.Sts2.Core.Entities.Creatures.Creature> targets,
            ref Task __result)
        {
            __result = DecimillipedeReattachSafe(__instance);
            return false; // skip original
        }

        private static async Task DecimillipedeReattachSafe(object segment)
        {
            try
            {
                var creature = ((MegaCrit.Sts2.Core.Models.MonsterModel)segment).Creature;
                var power = creature.GetPower<MegaCrit.Sts2.Core.Models.Powers.ReattachPower>();
                if (power != null)
                {
                    await power.DoReattach();
                }
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] Decimillipede reattach safe: {ex.Message}");
            }
        }

        public static bool VantomDismemberPrefix(
            object __instance,
            System.Collections.Generic.IReadOnlyList<MegaCrit.Sts2.Core.Entities.Creatures.Creature> targets,
            ref Task __result)
        {
            __result = VantomDismemberSafe(__instance, targets);
            return false; // skip original
        }

        private static async Task VantomDismemberSafe(
            object vantom,
            System.Collections.Generic.IReadOnlyList<MegaCrit.Sts2.Core.Entities.Creatures.Creature> targets)
        {
            try
            {
                // Real (ascension-aware) DismemberDamage from the instance.
                var dmgProp = vantom.GetType().GetProperty("DismemberDamage",
                    System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
                int dmg = Convert.ToInt32(dmgProp!.GetValue(vantom));
                // Faithful damage: monster attack for DismemberDamage (no VFX).
                await MegaCrit.Sts2.Core.Commands.DamageCmd.Attack(dmg)
                    .FromMonster((MegaCrit.Sts2.Core.Models.MonsterModel)vantom)
                    .Execute(null);
                // Add 3 Wounds to the player's discard (addedByPlayer: false),
                // matching Vantom.cs:166.
                await MegaCrit.Sts2.Core.Commands.CardPileCmd
                    .AddToCombatAndPreview<MegaCrit.Sts2.Core.Models.Cards.Wound>(
                        targets,
                        MegaCrit.Sts2.Core.Entities.Cards.PileType.Discard,
                        3,
                        creator: null);  // v0.107.1: param renamed addedByPlayer(bool)->creator(Player?)
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] Vantom dismember safe: {ex.Message}");
            }
        }

        /// <summary>
        /// Harmony prefix: replace SlumberingBeetle.AfterAddedToRoom with a
        /// headless-safe version. The original applies Plating + Slumber, then
        /// derefs `NCombatRoom.Instance.GetCreatureNode(...)` for the sleep
        /// VFX (SlumberingBeetle.cs:78); NCombatRoom.Instance is null in
        /// headless → NRE that wedges combat-start (the player turn never
        /// reaches PlayPhase, so DetectDecisionPoint mis-returns card_reward).
        /// We reproduce only the STATE effects (the two power applies).
        /// Bead sts2-rs-ybtmu.
        /// </summary>
        public static bool SlumberingBeetleAfterAddedPrefix(object __instance, ref Task __result)
        {
            __result = SlumberingBeetleAfterAddedSafe(__instance);
            return false; // skip original
        }

        private static async Task SlumberingBeetleAfterAddedSafe(object beetle)
        {
            try
            {
                var creature = ((MegaCrit.Sts2.Core.Models.MonsterModel)beetle).Creature;
                // Ascension-aware PlatingAmount (private prop): 18 @ ToughEnemies else 15.
                var platingProp = beetle.GetType().GetProperty("PlatingAmount",
                    System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
                int plating = platingProp != null ? Convert.ToInt32(platingProp.GetValue(beetle)) : 15;
                // v0.107.1: PowerCmd.Apply gained a leading PlayerChoiceContext.
                // These spawn-power applications take no player choices, so use a
                // ThrowingPlayerChoiceContext (mirrors the game's AutoSlay usage).
                var spawnCtx = new MegaCrit.Sts2.Core.GameActions.Multiplayer.ThrowingPlayerChoiceContext();
                await MegaCrit.Sts2.Core.Commands.PowerCmd
                    .Apply<MegaCrit.Sts2.Core.Models.Powers.PlatingPower>(spawnCtx, creature, plating, creature, null);
                await MegaCrit.Sts2.Core.Commands.PowerCmd
                    .Apply<MegaCrit.Sts2.Core.Models.Powers.SlumberPower>(spawnCtx, creature, 3m, creature, null);
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] SlumberingBeetle AfterAddedToRoom safe: {ex.Message}");
            }
        }

        /// <summary>
        /// Harmony prefix: headless-safe Whirlwind.OnPlay. The original
        /// (Whirlwind.cs) builds NHorizontalLinesVfx / NSmokyVignetteVfx and
        /// adds them to NCombatRoom.Instance / NRun.Instance scene nodes
        /// BEFORE the DamageCmd — null in headless → swallowed NRE, so the
        /// X-cost multi-hit never lands and the card vanishes. We reproduce
        /// only the attack. Bead sts2-rs-q6bix (PILE_COUNT: WHIRLWIND,
        /// CALAMITY_CARD).
        /// </summary>
        public static bool WhirlwindOnPlayPrefix(
            object __instance,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext,
            MegaCrit.Sts2.Core.Entities.Cards.CardPlay cardPlay,
            ref Task __result)
        {
            __result = WhirlwindOnPlaySafe(__instance, choiceContext);
            return false;
        }

        private static async Task WhirlwindOnPlaySafe(
            object cardObj,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext)
        {
            try
            {
                var card = (MegaCrit.Sts2.Core.Models.CardModel)cardObj;
                int num = card.ResolveEnergyXValue();
                if (num > 0 && card.CombatState != null)
                {
                    await MegaCrit.Sts2.Core.Commands.DamageCmd
                        .Attack(card.DynamicVars.Damage.BaseValue)
                        .WithHitCount(num)
                        .FromCard(card)
                        .TargetingAllOpponents(card.CombatState)
                        .Execute(choiceContext);
                }
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] Whirlwind OnPlay safe: {ex.Message}");
            }
        }

        /// <summary>
        /// Harmony prefix: headless-safe FlashOfSteel.OnPlay. The original
        /// (FlashOfSteel.cs) adds NThinSliceVfx.Create(cardPlay.Target) to the
        /// combat-room VFX container BEFORE the DamageCmd + Draw — the target
        /// node deref throws NRE in headless, so the 5 damage + draw-1 never
        /// happen and the card vanishes. We reproduce the attack + draw. Bead
        /// sts2-rs-q6bix (PILE_COUNT: FLASH_OF_STEEL).
        /// </summary>
        public static bool FlashOfSteelOnPlayPrefix(
            object __instance,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext,
            MegaCrit.Sts2.Core.Entities.Cards.CardPlay cardPlay,
            ref Task __result)
        {
            __result = FlashOfSteelOnPlaySafe(__instance, choiceContext, cardPlay);
            return false;
        }

        private static async Task FlashOfSteelOnPlaySafe(
            object cardObj,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext,
            MegaCrit.Sts2.Core.Entities.Cards.CardPlay cardPlay)
        {
            try
            {
                var card = (MegaCrit.Sts2.Core.Models.CardModel)cardObj;
                if (cardPlay.Target != null)
                {
                    await MegaCrit.Sts2.Core.Commands.DamageCmd
                        .Attack(card.DynamicVars.Damage.BaseValue)
                        .FromCard(card)
                        .Targeting(cardPlay.Target)
                        .Execute(choiceContext);
                }
                await MegaCrit.Sts2.Core.Commands.CardPileCmd
                    .Draw(choiceContext, card.DynamicVars.Cards.BaseValue, card.Owner);
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] FlashOfSteel OnPlay safe: {ex.Message}");
            }
        }

        /// <summary>
        /// Harmony prefix: headless-safe Slice.OnPlay. The original (Slice.cs)
        /// adds NThinSliceVfx.Create(cardPlay.Target) to the combat-room VFX
        /// container BEFORE the DamageCmd — the same NThinSliceVfx pattern as
        /// FlashOfSteel — so the target/combat-room node deref throws NRE in
        /// headless, the 6 damage never lands, and the card vanishes into the
        /// Play pile (never flushed to discard). We reproduce only the attack.
        /// Bead sts2-rs-q6bix (PILE_COUNT: SLICE, and CALAMITY_CARD which plays
        /// a generated Slice).
        /// </summary>
        public static bool SliceOnPlayPrefix(
            object __instance,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext,
            MegaCrit.Sts2.Core.Entities.Cards.CardPlay cardPlay,
            ref Task __result)
        {
            __result = SliceOnPlaySafe(__instance, choiceContext, cardPlay);
            return false;
        }

        private static async Task SliceOnPlaySafe(
            object cardObj,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext,
            MegaCrit.Sts2.Core.Entities.Cards.CardPlay cardPlay)
        {
            try
            {
                var card = (MegaCrit.Sts2.Core.Models.CardModel)cardObj;
                if (cardPlay.Target != null)
                {
                    await MegaCrit.Sts2.Core.Commands.DamageCmd
                        .Attack(card.DynamicVars.Damage.BaseValue)
                        .FromCard(card)
                        .Targeting(cardPlay.Target)
                        .Execute(choiceContext);
                }
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] Slice OnPlay safe: {ex.Message}");
            }
        }

        /// <summary>
        /// Harmony prefix: headless-safe RollingBoulderPower.AfterPlayerTurnStart.
        /// The real hook only takes the DoDamage-direct branch under
        /// TestMode.IsOn; otherwise it constructs NRollingBoulderVfx and calls
        /// GodotObject.Connect(StringName, Callable, uint), which the headless
        /// GodotStubs lack → MissingMethodException (swallowed), so no boulder
        /// damage lands and the turn wedges. We reproduce the TestMode branch:
        /// DoDamage(HittableEnemies) then SetAmount(Amount + DamageVar(5)).
        /// Bead sts2-rs-ioty0 (q6bix PILE_COUNT: ROLLING_BOULDER).
        /// </summary>
        public static bool RollingBoulderAfterPlayerTurnStartPrefix(
            object __instance,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext,
            MegaCrit.Sts2.Core.Entities.Players.Player player,
            ref Task __result)
        {
            __result = RollingBoulderAfterPlayerTurnStartSafe(__instance, choiceContext, player);
            return false;
        }

        private static async Task RollingBoulderAfterPlayerTurnStartSafe(
            object powerObj,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext,
            MegaCrit.Sts2.Core.Entities.Players.Player player)
        {
            try
            {
                var power = (MegaCrit.Sts2.Core.Models.PowerModel)powerObj;
                if (player != power.Owner.Player)
                {
                    return;
                }
                // DoDamage(HittableEnemies): Amount Unpowered, dealt by Owner.
                await MegaCrit.Sts2.Core.Commands.CreatureCmd.Damage(
                    choiceContext,
                    power.CombatState.HittableEnemies,
                    power.Amount,
                    MegaCrit.Sts2.Core.ValueProps.ValueProp.Unpowered,
                    power.Owner);
                // Ramp: SetAmount(Amount + DynamicVars.Damage.IntValue).
                power.SetAmount(power.Amount + power.DynamicVars.Damage.IntValue);
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] RollingBoulder AfterPlayerTurnStart safe: {ex.Message}");
            }
        }

        /// <summary>
        /// Harmony prefix: deterministic, non-interactive
        /// EntropyPower.AfterPlayerTurnStart. The real hook opens an
        /// interactive CardSelectCmd.FromHand(prefs Amount) to choose which
        /// cards to transform; the headless harness can't resolve a
        /// mid-turn-start select, so it wedges EndTurn. We instead transform
        /// the FIRST Amount cards in hand pile order via
        /// CardCmd.TransformToRandom(card, CombatCardSelection) — matching the
        /// Rust impl so card identities + RNG counter stay in lockstep.
        /// Bead sts2-rs-isooo (q6bix PILE_COUNT: ENTROPY).
        /// </summary>
        public static bool EntropyAfterPlayerTurnStartPrefix(
            object __instance,
            MegaCrit.Sts2.Core.GameActions.Multiplayer.PlayerChoiceContext choiceContext,
            MegaCrit.Sts2.Core.Entities.Players.Player player,
            ref Task __result)
        {
            // sts2-agent fork: in agent mode the game's own hook runs, and its
            // CardSelectCmd.FromHand (TransformSelectionPrompt, exactly Amount cards)
            // reaches the client as an in-combat card_select.
            if (RunSimulator.AgentModeActive) return true;
            __result = EntropyAfterPlayerTurnStartSafe(__instance, player);
            return false;
        }

        private static async Task EntropyAfterPlayerTurnStartSafe(
            object powerObj,
            MegaCrit.Sts2.Core.Entities.Players.Player player)
        {
            try
            {
                var power = (MegaCrit.Sts2.Core.Models.PowerModel)powerObj;
                if (player != power.Owner.Player)
                {
                    return;
                }
                int amount = power.Amount;
                // Snapshot the first `amount` hand cards BEFORE transforming —
                // TransformToRandom mutates the hand pile mid-loop.
                var hand = MegaCrit.Sts2.Core.Entities.Cards.PileType.Hand.GetPile(player).Cards;
                var targets = new System.Collections.Generic.List<MegaCrit.Sts2.Core.Models.CardModel>();
                for (int i = 0; i < hand.Count && targets.Count < amount; i++)
                {
                    targets.Add(hand[i]);
                }
                foreach (var card in targets)
                {
                    await MegaCrit.Sts2.Core.Commands.CardCmd.TransformToRandom(
                        card, player.RunState.Rng.CombatCardSelection);
                }
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[WARN] Entropy AfterPlayerTurnStart safe: {ex.Message}");
            }
        }
    }

    private static void InitLocManager()
    {
        // Create a LocManager instance with stub tables via reflection.
        // LocManager.Initialize() fails because PlatformUtil isn't available,
        // and Harmony can't patch some LocString methods due to JIT issues.
        // Solution: create an uninitialized LocManager, set its _tables, and
        // use Harmony only for the simple LocTable.GetRawText fallback.
        try
        {
            // Create uninitialized LocManager and set Instance
            var instanceProp = typeof(LocManager).GetProperty("Instance",
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            var instance = System.Runtime.CompilerServices.RuntimeHelpers.GetUninitializedObject(typeof(LocManager));
            instanceProp!.SetValue(null, instance);

            // Load REAL localization data from localization_eng/ JSON files
            var tablesField = typeof(LocManager).GetField("_tables",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            var tables = new Dictionary<string, LocTable>();

            var locDir = Path.Combine(AppContext.BaseDirectory, "..", "..", "..", "..", "..", "localization_eng");
            if (Directory.Exists(locDir))
            {
                foreach (var file in Directory.GetFiles(locDir, "*.json"))
                {
                    try
                    {
                        var name = Path.GetFileNameWithoutExtension(file);
                        var data = System.Text.Json.JsonSerializer.Deserialize<Dictionary<string, string>>(
                            File.ReadAllText(file));
                        if (data != null)
                            tables[name] = new LocTable(name, data);
                    }
                    catch { }
                }
                Console.Error.WriteLine($"[INFO] Loaded {tables.Count} localization tables from {locDir}");
            }
            else
            {
                Console.Error.WriteLine($"[WARN] Localization dir not found: {locDir}");
                // Fallback: empty tables
                var tableNames = new[] {
                    "achievements","acts","afflictions","ancients","ascension",
                    "bestiary","card_keywords","card_library","card_reward_ui",
                    "card_selection","cards","characters","combat_messages",
                    "credits","enchantments","encounters","epochs","eras",
                    "events","ftues","game_over_screen","gameplay_ui",
                    "inspect_relic_screen","intents","main_menu_ui","map",
                    "merchant_room","modifiers","monsters","orbs","potion_lab",
                    "potions","powers","relic_collection","relics","rest_site_ui",
                    "run_history","settings_ui","static_hover_tips","stats_screen",
                    "timeline","vfx"
                };
                foreach (var name in tableNames)
                    tables[name] = new LocTable(name, new Dictionary<string, string>());
            }
            tablesField!.SetValue(instance, tables);

            // Set Language
            var langProp = typeof(LocManager).GetProperty("Language",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Public);
            try { langProp?.SetValue(instance, "eng"); } catch { }

            // Set CultureInfo
            var cultureProp = typeof(LocManager).GetProperty("CultureInfo",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Public);
            try { cultureProp?.SetValue(instance, System.Globalization.CultureInfo.InvariantCulture); } catch { }

            // Initialize _smartFormatter — the game uses `new SmartFormatter()`
            try
            {
                var sfField = typeof(LocManager).GetField("_smartFormatter",
                    System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.NonPublic);
                // Dump ALL fields (instance + static)
                foreach (var f in typeof(LocManager).GetFields(System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Public))
                    Console.Error.WriteLine($"[DEBUG] LocManager {(f.IsStatic?"static":"inst")} field: {f.Name} ({f.FieldType.Name})");
                Console.Error.WriteLine($"[DEBUG] sfField: {sfField?.Name ?? "null"} type: {sfField?.FieldType?.Name ?? "null"}");
                if (sfField != null)
                {
                    try
                    {
                        // List constructors to find the right one
                        var ctors = sfField.FieldType.GetConstructors(
                            System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.NonPublic);
                        Console.Error.WriteLine($"[DEBUG] SmartFormatter has {ctors.Length} constructors:");
                        foreach (var ctor in ctors)
                        {
                            var ps = ctor.GetParameters();
                            Console.Error.WriteLine($"  ({string.Join(", ", ps.Select(p => $"{p.ParameterType.Name} {p.Name}"))})");
                        }
                        // Try the one with fewest params
                        var bestCtor = ctors.OrderBy(c => c.GetParameters().Length).First();
                        var args2 = bestCtor.GetParameters().Select(p =>
                            p.HasDefaultValue ? p.DefaultValue :
                            p.ParameterType.IsValueType ? Activator.CreateInstance(p.ParameterType) : null
                        ).ToArray();
                        var sf = bestCtor.Invoke(args2);
                        // Register extensions using the game's own LoadLocFormatters logic
                        // Call it via reflection on LocManager instance
                        try
                        {
                            var loadMethod = typeof(LocManager).GetMethod("LoadLocFormatters",
                                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
                            if (loadMethod != null)
                            {
                                loadMethod.Invoke(instance, null);
                                Console.Error.WriteLine("[INFO] SmartFormatter initialized via LoadLocFormatters");
                            }
                            else
                            {
                                sfField.SetValue(null, sf);
                                Console.Error.WriteLine("[INFO] SmartFormatter set (no LoadLocFormatters found)");
                            }
                        }
                        catch (Exception lfEx)
                        {
                            sfField.SetValue(null, sf);
                            Console.Error.WriteLine($"[WARN] LoadLocFormatters failed: {lfEx.InnerException?.Message ?? lfEx.Message}");
                        }
                    }
                    catch (Exception sfEx)
                    {
                        Console.Error.WriteLine($"[WARN] SmartFormatter create failed: {sfEx.GetType().Name}: {sfEx.Message}");
                        if (sfEx.InnerException != null)
                            Console.Error.WriteLine($"  Inner: {sfEx.InnerException.GetType().Name}: {sfEx.InnerException.Message}");
                    }
                }
                else
                {
                    Console.Error.WriteLine("[WARN] _smartFormatter field not found in LocManager");
                }
            }
            catch (Exception ex) { Console.Error.WriteLine($"[WARN] _smartFormatter init: {ex.GetType().Name}: {ex.Message}\n{ex.InnerException?.Message}"); }

            // Initialize _engTables to point to _tables (avoid null ref in fallback)
            try
            {
                var engTablesField = typeof(LocManager).GetField("_engTables",
                    System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
                engTablesField?.SetValue(instance, tables);
            }
            catch { }

            Console.Error.WriteLine("[INFO] LocManager initialized with stub tables");

            // Use Harmony to patch methods that need fallback behavior
            var harmony = new Harmony("sts2headless.locpatch");

            // With real loc data loaded, we only need fallback patches for:
            // 1. LocTable.GetRawText — return key for missing entries instead of throwing
            // 2. LocManager.SmartFormat — _smartFormatter is null, return raw text instead
            // We do NOT patch GetFormattedText/GetRawText on LocString anymore
            // so the real localization pipeline works (needed for Neow event etc.)

            var getRawText = typeof(LocTable).GetMethod("GetRawText",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Public,
                null, new[] { typeof(string) }, null);
            var prefix = typeof(LocPatches).GetMethod(nameof(LocPatches.GetRawTextPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            if (getRawText != null && prefix != null)
            {
                harmony.Patch(getRawText, new HarmonyMethod(prefix));
                Console.Error.WriteLine("[INFO] Patched LocTable.GetRawText");
            }

            // Patch GetLocString to not throw
            var getLocString = typeof(LocTable).GetMethod("GetLocString");
            var glsPrefix = typeof(LocPatches).GetMethod(nameof(LocPatches.GetLocStringPrefix),
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            if (getLocString != null && glsPrefix != null)
            {
                try { harmony.Patch(getLocString, new HarmonyMethod(glsPrefix)); }
                catch (Exception ex4) { Console.Error.WriteLine($"[WARN] Failed to patch GetLocString: {ex4.Message}"); }
            }

            // Patch FromChooseABundleScreen to use our card selector
            try
            {
                var bundleMethod = typeof(MegaCrit.Sts2.Core.Commands.CardSelectCmd).GetMethod("FromChooseABundleScreen",
                    System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                var bundlePrefix = typeof(LocPatches).GetMethod(nameof(LocPatches.BundleScreenPrefix),
                    System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                if (bundleMethod != null && bundlePrefix != null)
                {
                    harmony.Patch(bundleMethod, new HarmonyMethod(bundlePrefix));
                    Console.Error.WriteLine("[INFO] Patched FromChooseABundleScreen");
                }
            }
            catch (Exception ex) { Console.Error.WriteLine($"[WARN] Bundle patch: {ex.Message}"); }

            // Patch Neutralize.OnPlay to avoid the NThinSliceVfx.Create NRE (same
            // Godot-node deref as Slice/FlashOfSteel); the replacement keeps the
            // real DamageCmd.Attack path so attack hooks (Vigor) still fire.
            try
            {
                var neutralizeType = typeof(MegaCrit.Sts2.Core.Models.Cards.Neutralize);
                var neutralizeOnPlay = neutralizeType.GetMethod("OnPlay",
                    System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
                if (neutralizeOnPlay != null)
                {
                    var neutPrefix = typeof(LocPatches).GetMethod(nameof(LocPatches.NeutralizePrefix),
                        System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
                    if (neutPrefix != null)
                    {
                        harmony.Patch(neutralizeOnPlay, new HarmonyMethod(neutPrefix));
                        Console.Error.WriteLine("[INFO] Patched Neutralize.OnPlay");
                    }
                }
            }
            catch (Exception ex) { Console.Error.WriteLine($"[WARN] Neutralize patch: {ex.Message}"); }

            // Patch HasEntry to always return true
            PatchMethod(harmony, typeof(LocTable), "HasEntry", nameof(LocPatches.HasEntryPrefix));

            // Patch IsLocalKey to always return true
            PatchMethod(harmony, typeof(LocTable), "IsLocalKey", nameof(LocPatches.HasEntryPrefix));

            // Patch LocString.Exists (static) to always return true
            var locStringExists = typeof(LocString).GetMethod("Exists",
                System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            if (locStringExists != null)
            {
                PatchMethod(harmony, locStringExists, nameof(LocPatches.HasEntryPrefix));
            }

            // Patch LocTable.GetLocStringsWithPrefix to return empty list
            PatchMethod(harmony, typeof(LocTable), "GetLocStringsWithPrefix", nameof(LocPatches.GetLocStringsWithPrefixPrefix));
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[WARN] InitLocManager failed: {ex.Message}");
        }
    }

    private static void PatchMethod(Harmony harmony, Type type, string methodName, string patchName)
    {
        try
        {
            var method = type.GetMethod(methodName, System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.Static);
            PatchMethod(harmony, method, patchName);
        }
        catch (Exception ex) { Console.Error.WriteLine($"[WARN] Failed to patch {type.Name}.{methodName}: {ex.Message}"); }
    }

    private static void PatchMethod(Harmony harmony, System.Reflection.MethodInfo? method, string patchName)
    {
        if (method == null) return;
        try
        {
            var prefix = typeof(LocPatches).GetMethod(patchName, System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.Public);
            if (prefix != null) harmony.Patch(method, new HarmonyMethod(prefix));
        }
        catch (Exception ex) { Console.Error.WriteLine($"[WARN] Failed to patch {method.Name}: {ex.Message}"); }
    }

    internal static class LocPatches
    {
        public static bool GetRawTextPrefix(LocTable __instance, string key, ref string __result)
        {
            // Return key as fallback "translation"
            __result = key;
            return false;
        }

        public static bool GetFormattedTextPrefix(LocString __instance, ref string __result)
        {
            __result = __instance?.LocEntryKey ?? "";
            return false;
        }

        public static bool GetRawTextInstancePrefix(LocString __instance, ref string __result)
        {
            __result = __instance?.LocEntryKey ?? "";
            return false;
        }


        /// <summary>Harmony prefix: replace Neutralize.OnPlay with a headless-safe
        /// attack (real DamageCmd.Attack builder, so Vigor latches+consumes) + weak,
        /// dropping only the NThinSliceVfx.Create Godot-node deref that NREs.</summary>
        public static bool NeutralizePrefix(CardModel __instance, ref Task __result,
            PlayerChoiceContext choiceContext, CardPlay cardPlay)
        {
            if (cardPlay.Target == null) { __result = Task.CompletedTask; return false; }
            __result = NeutralizeSafe(__instance, choiceContext, cardPlay);
            return false;
        }

        private static async Task NeutralizeSafe(CardModel card, PlayerChoiceContext ctx, CardPlay play)
        {
            try
            {
                // Use the real attack builder — the SAME path SliceOnPlaySafe /
                // FlashOfSteelOnPlaySafe keep — so Hook.BeforeAttack/AfterAttack
                // fire: VigorPower latches on the first powered-card attack and is
                // consumed HERE, matching Neutralize.cs:44-47 and the Rust engine.
                // The bare CreatureCmd.Damage used before skipped those hooks,
                // leaving a phantom un-consumed Vigor on the player (the parity
                // artifact "silent-neutralize"). The NRE the patch originally
                // worked around was the NThinSliceVfx.Create deref in OnPlay
                // (same as Slice/FlashOfSteel), NOT the DamageCmd itself, so the
                // builder path is headless-safe. WithAttackerAnim is omitted (no
                // animation in headless), exactly as Slice/FlashOfSteel do.
                await DamageCmd.Attack(card.DynamicVars.Damage.BaseValue)
                    .FromCard(card)
                    .Targeting(play.Target!)
                    .Execute(ctx);
                await PowerCmd.Apply<WeakPower>(ctx, play.Target!, card.DynamicVars["WeakPower"].BaseValue,
                    card.Owner.Creature, card);  // v0.107.1: leading PlayerChoiceContext
            }
            catch (Exception ex) { Console.Error.WriteLine($"[WARN] Neutralize safe: {ex.Message}"); }
        }

        public static bool HasEntryPrefix(ref bool __result)
        {
            __result = true;
            return false;
        }

        public static bool GetLocStringPrefix(LocTable __instance, string key, ref LocString __result)
        {
            var nameField = typeof(LocTable).GetField("_name",
                System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.NonPublic);
            var tableName = nameField?.GetValue(__instance) as string ?? "_unknown";
            __result = new LocString(tableName, key);
            return false;
        }

        /// <summary>
        /// Intercept bundle selection — store bundles and wait for player to pick a pack index.
        /// </summary>
        public static bool BundleScreenPrefix(
            MegaCrit.Sts2.Core.Entities.Players.Player player,
            IReadOnlyList<IReadOnlyList<CardModel>> bundles,
            ref Task<IEnumerable<CardModel>> __result)
        {
            if (bundles.Count == 0)
            {
                __result = Task.FromResult<IEnumerable<CardModel>>(Array.Empty<CardModel>());
                return false;
            }

            // Store pending bundles for the main loop to present
            var sim = _bundleSimRef;
            if (sim != null)
            {
                sim._pendingBundles = bundles;
                sim._pendingBundleTcs = new TaskCompletionSource<IEnumerable<CardModel>>();
                Console.Error.WriteLine($"[SIM] Bundle selection pending: {bundles.Count} packs");

                __result = sim._pendingBundleTcs.Task;
                return false;
            }

            __result = Task.FromResult<IEnumerable<CardModel>>(bundles[0]);
            return false;
        }

        // Static reference so Harmony patch can access the simulator instance
        internal static RunSimulator? _bundleSimRef;

        public static bool GetLocStringsWithPrefixPrefix(ref IReadOnlyList<LocString> __result)
        {
            __result = new List<LocString>();
            return false;
        }
    }

    private static void Log(string message)
    {
        Console.Error.WriteLine($"[SIM] {message}");
    }

    // Max stack frames printed per swallowed exception. Enough to name the
    // throwing game method and its caller chain out of the event/command layer,
    // without turning a repeating fault into megabytes of stderr.
    private const int ExceptionTraceFrames = 12;

    /// <summary>
    /// Log an exception the harness is deliberately swallowing.
    /// </summary>
    /// <remarks>
    /// Every in-run <c>catch</c> must route through here rather than logging
    /// <c>ex.Message</c> alone, for two reasons:
    /// <list type="number">
    /// <item>An NRE's message is "Object reference not set to an instance of an
    /// object." — it names neither the type nor the site. The sweep-side world-error
    /// detector (<c>sts2_client._WORLD_ERROR_RE</c>) keys on the exception TYPE name,
    /// so a message-only log is invisible to it and the throw is scored as an
    /// ordinary loss.</item>
    /// <item>Without frames there is no way to tell WHICH game method died, and
    /// these faults are exactly the ones that silently drop an event option's state
    /// effect (see <c>PatchEventVfxAudio</c>) — the effect vanishes, the run
    /// continues, and nothing downstream can attribute it.</item>
    /// </list>
    /// Frames are printed in the runtime's own <c>   at Foo.Bar()</c> shape, which
    /// deliberately does NOT match the detector's regex: one throw must score as one
    /// world error, not as a dozen.
    /// </remarks>
    private static void LogException(string context, Exception ex)
    {
        var inner = ex;
        while (inner.InnerException != null) inner = inner.InnerException;
        Log($"{context}: {inner.GetType().Name}: {inner.Message}");
        EngineErrors.Record("host", $"{context}: {inner.GetType().Name}: {inner.Message}\n{inner.StackTrace}");
        var trace = inner.StackTrace;
        if (string.IsNullOrEmpty(trace)) return;
        var frames = trace.Split('\n');
        for (var i = 0; i < frames.Length && i < ExceptionTraceFrames; i++)
        {
            var frame = frames[i].TrimEnd('\r');
            if (frame.Trim().Length > 0) Console.Error.WriteLine($"[SIM] {frame.TrimEnd()}");
        }
        if (frames.Length > ExceptionTraceFrames)
            Console.Error.WriteLine($"[SIM]    ... {frames.Length - ExceptionTraceFrames} more frame(s)");
    }

    /// <summary>
    /// The `act_selection` stream the real lobby rolls act variants from:
    /// `new Rng((uint)GetDeterministicHashCode(seed), "act_selection")`
    /// (StartRunLobby.cs:459). Deliberately separate from RunRngSet's streams,
    /// which is why an act mismatch cannot show up as an `up_front` skew.
    /// </summary>
    private static Rng ActSelectionRng(string seedStr) =>
        new Rng((uint)StringHelper.GetDeterministicHashCode(seedStr), "act_selection");

    /// <summary>
    /// Resolve the `act1` command argument to an ActModel. "roll" reproduces
    /// ActModel.GetRandomList against this run's UnlockState. Note the harness
    /// runs with TestMode.IsOn, so GetRandomList's first-discovery force branch
    /// (ActModel.cs:570) is dead here and the roll is purely seed-driven --
    /// exactly what makes it comparable to the Rust `roll_acts` port.
    /// </summary>
    private static ActModel? ResolveAct1(string act1, string seedStr, bool playedSave) =>
        act1.ToLowerInvariant() switch
        {
            "overgrowth" => ModelDb.Act<Overgrowth>(),
            "underdocks" => ModelDb.Act<Underdocks>(),
            "roll" => ActModel
                .GetRandomList(ActSelectionRng(seedStr), BuildUnlockState(playedSave), isMultiplayer: false)
                .First(),
            _ => null,
        };

    /// <summary>
    /// Seed-driven act roll, WITHOUT starting a run. Returns the three act id
    /// slugs in index order so `run_parity.py --act-parity` can compare them
    /// against the Rust `sts2_rs.roll_acts` port seed-for-seed. This is the
    /// gate that catches "our engine models the wrong Act 1", a class of bug
    /// no existing parity check could see.
    /// </summary>
    public Dictionary<string, object?> RollActs(string? seed, bool playedSave = false) =>
        _gameThread.Submit(() => RollActsImpl(seed, playedSave));

    private Dictionary<string, object?> RollActsImpl(string? seed, bool playedSave)
    {
        try
        {
            EnsureModelDbInitialized();
            var seedStr = SeedHelper.CanonicalizeSeed(seed ?? "");
            var unlockState = BuildUnlockState(playedSave);
            var acts = ActModel
                .GetRandomList(ActSelectionRng(seedStr), unlockState, isMultiplayer: false)
                .Select(a => a.Id.Entry)
                .ToList();
            return new Dictionary<string, object?>
            {
                ["type"] = "acts",
                ["seed"] = seedStr,
                ["acts"] = acts,
                // The pool the roll actually had to choose from -- if this is a
                // singleton the "parity" below is vacuous, so report it.
                ["underdocks_unlocked"] = unlockState.IsEpochRevealed<UnderdocksEpoch>(),
                ["test_mode"] = TestMode.IsOn,
            };
        }
        catch (Exception ex)
        {
            return ErrorWithTrace("RollActs failed", ex);
        }
    }

    private static Dictionary<string, object?> Error(string message) =>
        new() { ["type"] = "error", ["message"] = message };

    private static Dictionary<string, object?> ErrorWithTrace(string context, Exception ex)
    {
        var inner = ex;
        while (inner.InnerException != null) inner = inner.InnerException;
        return new Dictionary<string, object?>
        {
            ["type"] = "error",
            ["message"] = $"{context}: {inner.GetType().Name}: {inner.Message}",
            ["stack_trace"] = inner.StackTrace,
        };
    }

    public Dictionary<string, object?> GetFullMap() =>
        _gameThread.Submit(() => GetFullMapImpl());

    private Dictionary<string, object?> GetFullMapImpl()
    {
        if (_runState?.Map == null)
            return Error("No map available");

        var map = _runState.Map;
        var rows = new List<List<Dictionary<string, object?>>>();
        var currentCoord = _runState.CurrentMapCoord;
        var visited = _runState.VisitedMapCoords;

        for (int row = 0; row < map.GetRowCount(); row++)
        {
            var rowNodes = new List<Dictionary<string, object?>>();
            foreach (var point in map.GetPointsInRow(row))
            {
                if (point == null) continue;
                var children = point.Children?.Select(ch => new Dictionary<string, object?>
                {
                    ["col"] = (int)ch.coord.col,
                    ["row"] = (int)ch.coord.row,
                }).ToList();

                var isVisited = visited?.Any(v => v.col == point.coord.col && v.row == point.coord.row) ?? false;
                var isCurrent = currentCoord.HasValue &&
                    currentCoord.Value.col == point.coord.col && currentCoord.Value.row == point.coord.row;

                rowNodes.Add(new Dictionary<string, object?>
                {
                    ["col"] = (int)point.coord.col,
                    ["row"] = (int)point.coord.row,
                    ["type"] = point.PointType.ToString(),
                    ["children"] = children,
                    ["visited"] = isVisited,
                    ["current"] = isCurrent,
                });
            }
            if (rowNodes.Count > 0)
                rows.Add(rowNodes);
        }

        // Boss node
        var bossNode = new Dictionary<string, object?>
        {
            ["col"] = (int)map.BossMapPoint.coord.col,
            ["row"] = (int)map.BossMapPoint.coord.row,
            ["type"] = map.BossMapPoint.PointType.ToString(),
        };

        // Add boss name/id — use BossEncounter?.Id?.Entry
        try
        {
            var bossIdEntry = _runState.Act?.BossEncounter?.Id?.Entry;
            if (!string.IsNullOrEmpty(bossIdEntry))
            {
                var monsterKey = bossIdEntry.EndsWith("_BOSS") ? bossIdEntry[..^5] : bossIdEntry;
                if (monsterKey == "THE_KIN") monsterKey = "KIN_PRIEST";
                bossNode["id"] = bossIdEntry;
                bossNode["name"] = _loc.Monster(monsterKey);
            }
        }
        catch { }

        // A10 DoubleBoss: a second boss node sits directly above the act boss
        // and fights a DIFFERENT encounter (NBossMapPoint._Ready picks
        // Act.SecondBossEncounter for it). Surfaced as its own meta-node so
        // the map can name and illustrate it as itself instead of repeating
        // the act boss. Null below A10, where no such node exists.
        Dictionary<string, object?>? secondBossNode = null;
        try
        {
            var secondPoint = map.SecondBossMapPoint;
            if (secondPoint != null)
            {
                secondBossNode = new Dictionary<string, object?>
                {
                    ["col"] = (int)secondPoint.coord.col,
                    ["row"] = (int)secondPoint.coord.row,
                    ["type"] = secondPoint.PointType.ToString(),
                };
                var secondIdEntry = _runState.Act?.SecondBossEncounter?.Id?.Entry;
                if (!string.IsNullOrEmpty(secondIdEntry))
                {
                    var monsterKey = secondIdEntry.EndsWith("_BOSS") ? secondIdEntry[..^5] : secondIdEntry;
                    if (monsterKey == "THE_KIN") monsterKey = "KIN_PRIEST";
                    secondBossNode["id"] = secondIdEntry;
                    secondBossNode["name"] = _loc.Monster(monsterKey);
                }
            }
        }
        catch { }

        return new Dictionary<string, object?>
        {
            ["type"] = "map",
            ["context"] = RunContext(),
            ["rows"] = rows,
            ["boss"] = bossNode,
            ["second_boss"] = secondBossNode,
            ["current_coord"] = currentCoord.HasValue ? new Dictionary<string, object?>
            {
                ["col"] = (int)currentCoord.Value.col,
                ["row"] = (int)currentCoord.Value.row,
            } : null,
        };
    }

    public void CleanUp() => _gameThread.Submit(() => CleanUpImpl());

    private void CleanUpImpl()
    {
        try
        {
            if (RunManager.Instance.IsInProgress)
                RunManager.Instance.CleanUp(graceful: true);
            ReleaseStrandedEventOptionTask("run cleaned up");
            _runState = null;
        }
        catch (Exception ex)
        {
            LogException("CleanUp exception", ex);
        }
    }

    #endregion
}
