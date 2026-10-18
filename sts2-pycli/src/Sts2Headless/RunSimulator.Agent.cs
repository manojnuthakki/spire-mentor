// sts2-agent fork: agent-mode failure reporting and action validation.
//
// In agent mode (start_run{agent_mode:true}) every action is checked against the last
// decision point the host returned before anything executes, and every failure is
// structured:
//
//   {type: "error", kind: "rejected" | "error" | "timeout",
//    execution: "not_started" | "started" | "completed" | "unknown",
//    code, message, diag}
//
// A rejection is always `not_started`. After any other failure the client must re-read
// the decision (`get_decision`); until it does, actions are rejected with
// `state_unknown`. Engine errors that the game or the host would otherwise swallow
// (TaskHelper.LogTaskExceptions, ActionExecutor, the host's own catch blocks) are
// collected while a command runs and reported as `kind: error, code: engine_error`.
// With agent mode off none of this runs and responses are unchanged.

using System.Collections.Concurrent;
using HarmonyLib;
using MegaCrit.Sts2.Core.Combat;
using MegaCrit.Sts2.Core.Entities.Cards;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Entities.Potions;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Runs;

namespace Sts2Headless;

public partial class RunSimulator
{
    /// <summary>
    /// The last decision point returned to the client (agent mode). Actions are
    /// validated against it; null after a failure that may have changed state.
    /// </summary>
    private Dictionary<string, object?>? _lastDecision;

    /// <summary>
    /// Actions each decision kind accepts. A kind not listed (e.g. "unknown") is not
    /// checked. Potion use and discard are accepted wherever the host accepted them
    /// before; their own checks below decide whether the potion can be used there.
    /// </summary>
    private static readonly Dictionary<string, string[]> ActionsByDecision = new()
    {
        ["map_select"] = new[] { "select_map_node", "use_potion", "discard_potion" },
        ["combat_play"] = new[] { "play_card", "end_turn", "use_potion", "discard_potion" },
        ["card_select"] = new[] { "select_cards", "skip_select" },
        ["card_reward"] = new[] { "select_card_reward", "skip_card_reward", "use_potion", "discard_potion" },
        ["event_choice"] = new[] { "choose_option", "proceed", "leave_room", "use_potion", "discard_potion" },
        ["bundle_select"] = new[] { "select_bundle" },
        ["rest_site"] = new[] { "choose_option", "leave_room", "proceed", "use_potion", "discard_potion" },
        ["shop"] = new[] { "buy_card", "buy_relic", "buy_potion", "remove_card", "leave_room", "proceed", "use_potion", "discard_potion" },
        ["game_over"] = Array.Empty<string>(),
    };

    /// <summary>
    /// Run one client command. In agent mode: collect engine errors raised while it
    /// runs, turn every error response into a structured failure, and remember the
    /// decision point it returns.
    /// </summary>
    private Dictionary<string, object?> Tracked(string what, Func<Dictionary<string, object?>> work)
    {
        if (!_agentMode) return work();
        foreach (var stray in EngineErrors.Drain())
            Log($"[WARN] engine error between commands (not reported): {stray}");

        var result = work();
        var errors = EngineErrors.Drain().Distinct().ToList();
        if (IsType(result, "error") && !result.ContainsKey("kind"))
            result = Failure("error", "unknown", result.GetValueOrDefault("message") as string ?? "host error",
                null, "host_error", new() { ["stack_trace"] = result.GetValueOrDefault("stack_trace") });
        if (errors.Count > 0)
        {
            if (IsType(result, "error"))
            {
                if (result["diag"] is Dictionary<string, object?> diag) diag["engine_errors"] = errors;
            }
            else
            {
                result = Failure("error", "started",
                    $"the engine reported {errors.Count} error(s) during {what}; its effects may be partly applied",
                    null, "engine_error", new()
                    {
                        ["engine_errors"] = errors,
                        ["decision_after"] = result.GetValueOrDefault("decision"),
                    });
            }
        }

        if (IsType(result, "decision"))
            _lastDecision = result;
        else if (IsType(result, "error") && result.GetValueOrDefault("execution") as string != "not_started")
            _lastDecision = null;
        return result;
    }

    private static bool IsType(Dictionary<string, object?> d, string type) =>
        d.GetValueOrDefault("type") as string == type;

    /// <summary>A failure the client can act on (see the file header).</summary>
    private Dictionary<string, object?> Failure(string kind, string execution, string message, string? waitingFor,
        string? code = null, Dictionary<string, object?>? extra = null)
    {
        var pcs = _runState?.Players[0].PlayerCombatState;
        var diag = new Dictionary<string, object?>
        {
            ["waiting_for"] = waitingFor,
            ["combat_in_progress"] = CombatManager.Instance?.IsInProgress,
            ["play_phase"] = IsPlayPhase(),
            ["turn_phase"] = pcs?.Phase.ToString(),
            ["executor_running"] = RunManager.Instance?.ActionExecutor?.IsRunning,
            ["selection_pending"] = AnySelectionPending(),
            ["room"] = _runState?.CurrentRoom?.GetType().Name,
        };
        if (extra != null)
            foreach (var (k, v) in extra) diag[k] = v;
        return new Dictionary<string, object?>
        {
            ["type"] = "error",
            ["kind"] = kind,
            ["execution"] = execution,
            ["code"] = code,
            ["message"] = message,
            ["diag"] = diag,
        };
    }

    /// <summary>An action refused before anything executed.</summary>
    private Dictionary<string, object?> Reject(string code, string message) => new()
    {
        ["type"] = "error",
        ["kind"] = "rejected",
        ["execution"] = "not_started",
        ["code"] = code,
        ["message"] = message,
        ["diag"] = new Dictionary<string, object?> { ["decision"] = _lastDecision?.GetValueOrDefault("decision") },
    };

    /// <summary>
    /// Check an action against the last decision point and the live prompt. Returns a
    /// rejection, or null if the action may execute.
    /// </summary>
    private Dictionary<string, object?>? ValidateAgentAction(string action, Dictionary<string, object?>? args, Player player)
    {
        if (_lastDecision == null)
            return Reject("state_unknown", "the previous command failed after it started; re-read the decision with get_decision");
        var decision = _lastDecision.GetValueOrDefault("decision") as string ?? "";
        if (ActionsByDecision.TryGetValue(decision, out var allowed) && !allowed.Contains(action))
            return Reject("wrong_decision", $"{action} is not an action at {decision}");

        switch (action)
        {
            case "select_cards": return ValidateSelectCards(args);
            case "skip_select": return ValidateSkipSelect();
            case "select_card_reward": return ValidateSelectCardReward(args);
            case "skip_card_reward": return ValidateSkipCardReward();
            case "choose_option": return ValidateChooseOption(args);
            case "select_map_node": return ValidateMapNode(args);
            case "play_card": return ValidatePlayCard(args, player);
            case "use_potion": return ValidatePotion(args, player, use: true);
            case "discard_potion": return ValidatePotion(args, player, use: false);
            default: return null;
        }
    }

    private static bool TryGetInt(Dictionary<string, object?>? args, string key, out int value)
    {
        value = 0;
        if (args == null || !args.TryGetValue(key, out var raw) || raw == null) return false;
        if (raw is int i) { value = i; return true; }
        return int.TryParse(raw.ToString(), out value);
    }

    private Dictionary<string, object?>? ValidateSelectCards(Dictionary<string, object?>? args)
    {
        var options = _cardSelector.PendingOptions;
        if (!_cardSelector.HasPending || options == null)
            return Reject("no_pending_selection", "no card selection is pending");
        if (args == null || !args.TryGetValue("indices", out var raw) || raw == null)
            return Reject("missing_argument", "select_cards requires 'indices' (comma-separated card indices)");

        var text = raw.ToString()!.Trim();
        var indices = new List<int>();
        if (text.Length > 0)
        {
            foreach (var token in text.Split(','))
            {
                if (!int.TryParse(token.Trim(), out var index))
                    return Reject("malformed_indices", $"'{token.Trim()}' is not a card index");
                indices.Add(index);
            }
        }
        if (indices.Any(i => i < 0 || i >= options.Count))
            return Reject("index_out_of_range",
                $"index {indices.First(i => i < 0 || i >= options.Count)} is outside 0..{options.Count - 1}");
        if (indices.Distinct().Count() != indices.Count)
            return Reject("duplicate_index", "each card can be chosen once");
        int lo = _cardSelector.PendingMinSelect, hi = _cardSelector.PendingMaxSelect;
        if (indices.Count < lo || indices.Count > hi)
            return Reject("count_out_of_range", $"{indices.Count} card(s) chosen; this prompt takes {lo}..{hi}");
        return null;
    }

    private Dictionary<string, object?>? ValidateSkipSelect()
    {
        if (!_cardSelector.HasPending)
            return Reject("no_pending_selection", "no card selection is pending");
        if (_cardSelector.PendingMinSelect > 0)
            return Reject("skip_not_allowed",
                $"this prompt requires at least {_cardSelector.PendingMinSelect} card(s); choosing none is not allowed");
        return null;
    }

    private Dictionary<string, object?>? ValidateSelectCardReward(Dictionary<string, object?>? args)
    {
        int count;
        if (_cardSelector.HasPendingReward) count = _cardSelector.PendingRewardCards!.Count;
        else if (_pendingCardReward != null) count = _pendingCardReward.Cards.Count();
        else return Reject("no_pending_card_reward", "no card reward is pending");
        if (!TryGetInt(args, "card_index", out var index))
            return Reject("missing_argument", "select_card_reward requires 'card_index'");
        if (index < 0 || index >= count)
            return Reject("invalid_card_index", $"card index {index} is outside 0..{count - 1}");
        return null;
    }

    private Dictionary<string, object?>? ValidateSkipCardReward()
    {
        if (_cardSelector.HasPendingReward) return null;  // event card rewards can always be skipped
        if (_pendingCardReward == null)
            return Reject("no_pending_card_reward", "no card reward is pending");
        if (!_pendingCardReward.CanSkip)
            return Reject("skip_not_allowed", "this card reward cannot be skipped");
        return null;
    }

    private Dictionary<string, object?>? ValidateChooseOption(Dictionary<string, object?>? args)
    {
        if (!TryGetInt(args, "option_index", out var index))
            return Reject("missing_argument", "choose_option requires 'option_index'");
        if (_lastDecision!.GetValueOrDefault("options") is not List<Dictionary<string, object?>> options)
            return null;
        if (index < 0 || index >= options.Count)
            return Reject("invalid_option", $"option index {index} is outside 0..{options.Count - 1}");
        var option = options[index];
        if (option.GetValueOrDefault("is_locked") is true)
            return Reject("option_locked", $"option {index} is locked");
        if (option.GetValueOrDefault("is_enabled") is false)
            return Reject("option_disabled", $"option {index} is not available");
        return null;
    }

    private Dictionary<string, object?>? ValidateMapNode(Dictionary<string, object?>? args)
    {
        if (!TryGetInt(args, "col", out var col) || !TryGetInt(args, "row", out var row))
            return Reject("missing_argument", "select_map_node requires 'col' and 'row'");
        if (_lastDecision!.GetValueOrDefault("choices") is not List<Dictionary<string, object?>> choices)
            return null;
        if (!choices.Any(c => Convert.ToInt32(c["col"]) == col && Convert.ToInt32(c["row"]) == row))
            return Reject("invalid_map_node", $"({col},{row}) is not a reachable map node");
        return null;
    }

    private Dictionary<string, object?>? ValidatePlayCard(Dictionary<string, object?>? args, Player player)
    {
        if (!TryGetInt(args, "card_index", out var index))
            return Reject("missing_argument", "play_card requires 'card_index'");
        var hand = player.PlayerCombatState?.Hand.Cards;
        if (hand == null)
            return Reject("wrong_decision", "not in combat");
        if (index < 0 || index >= hand.Count)
            return Reject("invalid_card_index", $"card index {index} is outside 0..{hand.Count - 1}");
        var card = hand[index];
        if (!card.CanPlay(out var reason, out _))
            return Reject(reason switch
            {
                UnplayableReason.EnergyCostTooHigh => "not_enough_energy",
                UnplayableReason.StarCostTooHigh => "not_enough_stars",
                _ => "unplayable",
            }, $"{card.Id.Entry} cannot be played: {reason}");
        if (card.TargetType == TargetType.AnyEnemy)
            return ValidateEnemyTarget(args);
        return null;
    }

    private Dictionary<string, object?>? ValidatePotion(Dictionary<string, object?>? args, Player player, bool use)
    {
        if (!TryGetInt(args, "potion_index", out var index))
            return Reject("missing_argument", $"{(use ? "use_potion" : "discard_potion")} requires 'potion_index'");
        var potions = player.Potions?.ToList() ?? new();
        if (index < 0 || index >= potions.Count)
            return Reject("invalid_potion_index", $"potion index {index} is outside 0..{potions.Count - 1}");
        var potion = potions[index];  // Player.Potions lists held potions only, so never null
        if (!player.CanRemovePotions)
            return Reject("potions_locked", "potions cannot be used or discarded right now");
        if (!use) return null;
        // The game's own Use button rules (NPotionPopup).
        bool usable = potion.Usage switch
        {
            PotionUsage.AnyTime => true,
            PotionUsage.CombatOnly => CombatManager.Instance.IsInProgress && IsPlayPhase(),
            _ => false,
        };
        if (!usable || !potion.PassesCustomUsabilityCheck)
            return Reject("potion_not_usable", $"{potion.Id.Entry} cannot be used here");
        if (potion.TargetType == TargetType.AnyEnemy && CombatManager.Instance.IsInProgress)
            return ValidateEnemyTarget(args);
        return null;
    }

    private Dictionary<string, object?>? ValidateEnemyTarget(Dictionary<string, object?>? args)
    {
        var alive = CombatManager.Instance.DebugOnlyGetState()?.Enemies.Count(e => e != null && e.IsAlive) ?? 0;
        if (!TryGetInt(args, "target_index", out var target))
            return Reject("invalid_target", "this needs an enemy target ('target_index')");
        if (target < 0 || target >= alive)
            return Reject("invalid_target", $"target index {target} is outside 0..{alive - 1} (living enemies)");
        return null;
    }
}

/// <summary>
/// sts2-agent fork: errors the engine logs (TaskHelper.LogTaskExceptions and
/// ActionExecutor both report a swallowed exception through Log.Error) and exceptions
/// the host catches and only logs. Drained per command by RunSimulator.Tracked.
/// </summary>
internal static class EngineErrors
{
    private const int MaxKept = 20;
    private const int MaxChars = 600;
    private static readonly ConcurrentQueue<string> Queue = new();
    private static int _hooked;

    public static void Hook()
    {
        if (Interlocked.Exchange(ref _hooked, 1) == 1) return;
        MegaCrit.Sts2.Core.Logging.Log.LogCallback += (level, text, _) =>
        {
            if (level == MegaCrit.Sts2.Core.Logging.LogLevel.Error) Record("engine", text);
        };
    }

    /// <summary>Keep the first line and the first game frame of a logged exception.</summary>
    public static void Record(string source, string text)
    {
        if (Queue.Count >= MaxKept) return;
        var lines = text.Split('\n').Select(l => l.Trim()).Where(l => l.Length > 0).ToList();
        var summary = lines.FirstOrDefault() ?? "";
        var frame = lines.Skip(1).FirstOrDefault(l => l.StartsWith("at MegaCrit."));
        if (frame != null) summary += " " + frame;
        if (summary.Length > MaxChars) summary = summary[..MaxChars];
        Queue.Enqueue($"{source}: {summary}");
    }

    public static List<string> Drain()
    {
        var drained = new List<string>();
        while (Queue.TryDequeue(out var item)) drained.Add(item);
        return drained;
    }
}

/// <summary>
/// sts2-agent fork: test-only fault injection, for scenarios that check how failures
/// are reported. <c>STS2_TEST_FAULT</c> is <c>hang:TYPE:METHOD</c> or
/// <c>throw:TYPE:METHOD</c>, naming a game method that returns a Task (for example
/// <c>hang:MegaCrit.Sts2.Core.Models.MonsterModel:PerformMove</c>). Every call then
/// returns a task that never completes, or one that faults, in place of the real body.
/// Unset (the normal case) installs nothing.
/// </summary>
internal static class TestFaults
{
    private static string _fault = "";

    public static void Install()
    {
        var fault = Environment.GetEnvironmentVariable("STS2_TEST_FAULT");
        if (string.IsNullOrEmpty(fault)) return;
        var parts = fault.Split(':', 2);
        var method = parts.Length == 2 ? AccessTools.Method(parts[1]) : null;
        if (parts[0] is not ("hang" or "throw") || method == null || method.ReturnType != typeof(Task))
        {
            // A misspelt test knob must not quietly run the scenario without its fault.
            Console.Error.WriteLine($"[FATAL] STS2_TEST_FAULT '{fault}': expected hang|throw:TYPE:METHOD returning Task");
            Environment.Exit(2);
        }
        _fault = fault;
        new Harmony("sts2headless.testfault").Patch(method,
            new HarmonyMethod(typeof(TestFaults), parts[0] == "hang" ? nameof(HangPrefix) : nameof(ThrowPrefix)));
        Console.Error.WriteLine($"[TEST] fault injected: {fault}");
    }

    public static bool HangPrefix(ref Task __result)
    {
        __result = new TaskCompletionSource().Task;
        return false;
    }

    public static bool ThrowPrefix(ref Task __result)
    {
        __result = Task.FromException(new InvalidOperationException($"STS2_TEST_FAULT {_fault}"));
        return false;
    }
}
