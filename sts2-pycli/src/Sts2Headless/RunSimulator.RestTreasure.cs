// sts2-agent fork: the rest site and the treasure room, in agent mode.
//
// Upstream forces the run to the map after one rest-site option (Smith and a cancelled
// Smith included, which breaks Miniature Tent), and plays a treasure room with no
// decision: gold, extras and relic 0 are taken, then the map. In agent mode the host
// returns the rooms' own decisions instead:
//
//   rest_site: choose_option{option_index} (the options the game lists; a disabled one
//              is refused, `option_disabled`), proceed (the room's Proceed button,
//              enabled once an option succeeded: `can_proceed`), use_potion,
//              discard_potion
//   treasure:  take_relic{relic_index}, proceed (with a relic still offered this is
//              the game's Skip button, which in singleplayer also leaves the room),
//              use_potion, discard_potion
//
// A rest-site option runs through the game's own path (RestSiteSynchronizer.
// ChooseLocalOption) on the thread pool, like an event option: Smith opens the
// ordinary card selection, and cancelling it (skip_select on the cancelable prompt)
// makes the option fail, so the room stays as it was, as on the game's screen. A
// successful option removes itself, or clears the rest (unless a relic such as
// Miniature Tent keeps them), and enables Proceed (NRestSiteRoom.ShowProceedButton).
//
// The treasure chest is opened on entering the room (the game's chest click, a step
// with no choice): DoNormalRewards (gold) then DoExtraRewardsIfNeeded, whose rewards go
// to the `rewards` decision, as the game shows them before the relic
// (NTreasureRoom.OpenChest). The relic is then offered; taking it goes through the
// game's vote (PickRelicLocally) and RelicCmd.Obtain, whose own offers go to the
// client too. The chest opening is reported with the decision that follows it, as
// `forced: [{kind: "open_chest", gold}]`. Rewards offered by a rest-site option (Dream Catcher, Tiny Mailbox) come
// to the `rewards` decision the same way. With agent mode off none of this runs.

using MegaCrit.Sts2.Core.Commands;
using MegaCrit.Sts2.Core.Entities.Players;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Runs;

namespace Sts2Headless;

public partial class RunSimulator
{
    /// <summary>A rest-site option still running (suspended at what it opened).</summary>
    private Task<bool>? _agentRestTask;

    /// <summary>An option succeeded in this rest site, so its Proceed button is enabled.</summary>
    private bool _restSiteChoiceMade;

    /// <summary>The chest of this treasure room has been opened.</summary>
    private bool _treasureOpened;

    /// <summary>The chest's relic has been taken.</summary>
    private bool _treasureRelicTaken;

    /// <summary>Treasure-room work still running: the chest's extra rewards, or obtaining the relic.</summary>
    private Task? _agentTreasureTask;

    /// <summary>
    /// Steps the host took on its own while a command ran (agent mode), returned with the
    /// command's decision as `forced`: {kind: "open_chest", gold}; after a boss
    /// {kind: "next_act", act} and {kind: "enter_ancient", node: {col, row}} (RunSimulator.cs).
    /// </summary>
    private readonly List<Dictionary<string, object?>> _agentForced = new();

    /// <summary>A new room is being entered.</summary>
    private void ResetAgentRestTreasure()
    {
        _agentRestTask = null;
        _restSiteChoiceMade = false;
        _treasureOpened = false;
        _treasureRelicTaken = false;
        _agentTreasureTask = null;
        _agentEventResumeTask = null;
        ResetAgentShop();
    }

    /// <summary>
    /// Drive a running shop purchase, event resume (RunSimulator.EventResume.cs), rest-site
    /// option or treasure task once what it opened has been answered: it completes or
    /// suspends at the next prompt. Called before reading the decision point.
    /// </summary>
    private void DriveAgentRoomTasks()
    {
        DriveAgentShopTask();
        DriveAgentEventResume();
        if (_agentRestTask != null && !AnySelectionPending())
        {
            WaitUntilOptionTaskSettles(_agentRestTask, "rest-site option (until next prompt or completion)");
            if (_agentRestTask.IsCompleted)
            {
                var task = _agentRestTask;
                _agentRestTask = null;
                if (task.IsFaulted) LogException("Rest site option", task.Exception!);
                else if (task.IsCompletedSuccessfully && task.Result) _restSiteChoiceMade = true;
                WaitForActionExecutor();
                _syncCtx.Pump();
            }
        }
        if (_agentTreasureTask != null && !AnySelectionPending())
        {
            WaitUntilOptionTaskSettles(_agentTreasureTask, "treasure rewards or relic (until next prompt or completion)");
            if (_agentTreasureTask.IsCompleted)
            {
                var task = _agentTreasureTask;
                _agentTreasureTask = null;
                if (task.IsFaulted) LogException("Treasure room", task.Exception!);
                WaitForActionExecutor();
                _syncCtx.Pump();
            }
        }
    }

    // ── Rest site ──

    private Dictionary<string, object?> AgentRestSiteState(RestSiteRoom restRoom, Player player)
    {
        var options = restRoom.Options.Select((opt, i) => new Dictionary<string, object?>
        {
            ["index"] = i,
            ["option_id"] = opt.OptionId,
            ["name"] = opt.GetType().Name,
            // What the button shows: its title, and its description as a template with the
            // values the game filled in (Heal's amount includes Regal Pillow and the like).
            ["title"] = LocText(opt.Title),
            ["description"] = LocText(opt.Description),
            ["vars"] = LocVars(opt.Description, player),
            ["preview"] = RestPreview(opt, player),
            ["is_enabled"] = opt.IsEnabled,
            ["reason"] = opt.IsEnabled ? null : opt.OptionId switch
            {
                "SMITH" => "no_upgradable_cards",
                "COOK" => "not_enough_removable_cards",
                _ => "disabled",
            },
        }).ToList();
        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "rest_site",
            ["context"] = RunContext(),
            ["options"] = options,
            ["can_proceed"] = _restSiteChoiceMade,
            ["player"] = PlayerSummary(player),
        };
    }

    private string? LocText(MegaCrit.Sts2.Core.Localization.LocString? loc)
    {
        if (loc == null) return null;
        try { return _loc.Bilingual(loc.LocTable, loc.LocEntryKey); }
        catch { return loc.LocEntryKey; }
    }

    /// <summary>A LocString's variables as plain values (numbers, text, flags).</summary>
    private Dictionary<string, object?>? LocVars(MegaCrit.Sts2.Core.Localization.LocString? loc, Player player)
    {
        if (loc == null) return null;
        var vars = new Dictionary<string, object?>();
        try
        {
            foreach (var (name, value) in loc.Variables)
            {
                vars[name] = value switch
                {
                    MegaCrit.Sts2.Core.Localization.DynamicVars.DynamicVar dv => (object)(int)dv.BaseValue,
                    decimal d when d == Math.Floor(d) => (int)d,
                    decimal d => (double)d,
                    MegaCrit.Sts2.Core.Localization.LocString nested => LocText(nested),
                    // Text the game already formatted: under the host's loc stub it is
                    // loc keys (Regal Pillow's "+{Heal} HP" line); look them up.
                    string text => string.Join("\n", text.Split('\n').Select(line => FormattedLine(line, player))),
                    bool or int => value,
                    _ => value?.ToString(),
                };
            }
        }
        catch (Exception ex) { Log($"Rest option vars unreadable: {ex.Message}"); }
        return vars.Count > 0 ? vars : null;
    }

    /// <summary>
    /// A line the game formatted from a relic's text (`RELIC.additionalRestSiteHealText`,
    /// with the relic's own values): under the host's loc stub it arrives as the bare key,
    /// so look the text up and fill its {Name} placeholders from the owning relic's
    /// DynamicVars. A line that isn't such a key is returned as it came.
    /// </summary>
    private string FormattedLine(string line, Player player)
    {
        var text = _loc.BilingualFromKey(line);
        if (text == line) return line;
        var entry = line.Split('.')[0];
        var relic = player.Relics.FirstOrDefault(r => r.Id.Entry == entry);
        if (relic != null)
        {
            try
            {
                foreach (var dv in relic.DynamicVars.Values)
                    text = text.Replace("{" + dv.Name + "}", ((int)dv.BaseValue).ToString());
            }
            catch (Exception ex) { Log($"Relic text vars unreadable ({entry}): {ex.Message}"); }
        }
        return text;
    }

    /// <summary>
    /// What choosing an option would do, as the game computes it: HEAL's full heal
    /// (HealRestSiteOption.GetHealAmount, with Regal Pillow and other modifiers), which
    /// the description shows split into the base amount and extra lines.
    /// </summary>
    private Dictionary<string, object?>? RestPreview(MegaCrit.Sts2.Core.Entities.RestSite.RestSiteOption opt, Player player)
    {
        if (opt is not MegaCrit.Sts2.Core.Entities.RestSite.HealRestSiteOption) return null;
        try
        {
            var method = HarmonyLib.AccessTools.Method(typeof(MegaCrit.Sts2.Core.Entities.RestSite.HealRestSiteOption), "GetHealAmount");
            var p = method.GetParameters();
            object?[] args = p.Select(pi => pi.ParameterType.IsInstanceOfType(player) ? player
                : pi.ParameterType.IsInstanceOfType(player.Creature) ? (object?)player.Creature : null).ToArray();
            var amount = method.Invoke(method.IsStatic ? null : opt, args);
            // The heal applies the amount truncated to whole HP (82 max HP: 24.6 heals 24).
            return new() { ["heal"] = (int)Convert.ToDecimal(amount) };
        }
        catch (Exception ex)
        {
            Log($"Rest heal preview unavailable: {ex.GetType().Name}: {ex.Message}");
            return null;
        }
    }

    private Dictionary<string, object?> DoAgentRestOption(int optionIndex)
    {
        Log($"Rest site: choosing option {optionIndex}");
        _agentRestTask = Task.Run(() =>
        {
            SurfaceCustomRewards.Value = true;
            return RunManager.Instance.RestSiteSynchronizer.ChooseLocalOption(optionIndex);
        });
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> ProceedFromRestSite()
    {
        Log("Rest site: proceeding to the map");
        ForceToMap();
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?>? ValidateProceedFromRestSite()
    {
        if (_lastDecision!.GetValueOrDefault("can_proceed") is false)
            return Reject("proceed_not_allowed", "the rest site's Proceed button is enabled once an option has been chosen");
        return null;
    }

    // ── Treasure ──

    private Dictionary<string, object?> AgentTreasureState(TreasureRoom treasureRoom, Player player)
    {
        if (!_treasureOpened)
        {
            _treasureOpened = true;
            Log("Treasure room: opening the chest");
            WaitForActionExecutor();
            _syncCtx.Pump();
            var before = player.Gold;
            try
            {
                var rolled = treasureRoom.DoNormalRewards().GetAwaiter().GetResult();
                _syncCtx.Pump();
                Log($"Treasure room: {rolled} gold rolled");
            }
            catch (Exception ex) { LogException("Treasure gold", ex); }
            finally
            {
                // The chest was opened whether or not a gold hook threw; report what the
                // player actually gained (gold modifiers such as Bowler Hat apply).
                _agentForced.Add(new() { ["kind"] = "open_chest", ["gold"] = player.Gold - before });
            }
            _agentTreasureTask = Task.Run(() =>
            {
                SurfaceCustomRewards.Value = true;
                return treasureRoom.DoExtraRewardsIfNeeded();
            });
            return DetectDecisionPoint();
        }

        var relics = _treasureRelicTaken
            ? new List<Dictionary<string, object?>>()
            : (RunManager.Instance.TreasureRoomRelicSynchronizer.CurrentRelics ?? new List<MegaCrit.Sts2.Core.Models.RelicModel>())
                .Select((r, i) => RelicOfferDict(r, i)).ToList();
        return new Dictionary<string, object?>
        {
            ["type"] = "decision",
            ["decision"] = "treasure",
            ["context"] = RunContext(),
            ["relics"] = relics,
            ["can_skip"] = relics.Count > 0,
            ["player"] = PlayerSummary(player),
        };
    }

    private Dictionary<string, object?> DoTakeRelic(Player player, Dictionary<string, object?>? args)
    {
        TryGetInt(args, "relic_index", out var index);  // checked by ValidateTakeRelic
        var sync = RunManager.Instance.TreasureRoomRelicSynchronizer;
        var relic = sync.CurrentRelics![index].ToMutable();
        Log($"Treasure room: taking relic {index} ({relic.Id.Entry})");
        // The vote closes the picking session; RelicsAwarded's only subscriber is the
        // UI, which obtains the relic, so the obtain runs here (as upstream does).
        sync.PickRelicLocally(index);
        WaitForActionExecutor();
        _syncCtx.Pump();
        _treasureRelicTaken = true;
        _agentTreasureTask = Task.Run(() =>
        {
            SurfaceCustomRewards.Value = true;
            return (Task)RelicCmd.Obtain(relic, player);
        });
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?> ProceedFromTreasure()
    {
        var sync = RunManager.Instance.TreasureRoomRelicSynchronizer;
        if (!_treasureRelicTaken && sync.CurrentRelics is { Count: > 0 })
        {
            Log("Treasure room: skipping the relic");
            sync.SkipRelicLocally();
            WaitForActionExecutor();
            _syncCtx.Pump();
        }
        Log("Treasure room: proceeding to the map");
        ForceToMap();
        return DetectDecisionPoint();
    }

    private Dictionary<string, object?>? ValidateTakeRelic(Dictionary<string, object?>? args)
    {
        if (!TryGetInt(args, "relic_index", out var index))
            return Reject("missing_argument", "take_relic requires 'relic_index'");
        if (_lastDecision!.GetValueOrDefault("relics") is not List<Dictionary<string, object?>> relics || relics.Count == 0
            || RunManager.Instance.TreasureRoomRelicSynchronizer.CurrentRelics is not { Count: > 0 })
            return Reject("no_relic_offered", "no relic is offered here");
        if (index < 0 || index >= relics.Count)
            return Reject("invalid_relic_index", $"relic index {index} is outside 0..{relics.Count - 1}");
        return null;
    }
}
