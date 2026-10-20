// sts2-agent fork: event pages in agent mode.
//
// Upstream leaves an event in three ways the client can't see or diagnose; in agent mode:
//
// - A finished event (it set an empty page: SetEventFinished) is left for the map, as the
//   game's Proceed button on the finished text would, and the response that follows says
//   so: `forced: [{kind: "event_finished", event_id}]` (Neow, the Ancients and the Crystal
//   Sphere included).
// - An option that raised no state change (no SetEventState, no SetEventFinished) and
//   entered no combat is no longer force-finished. The page is shown again with that
//   option disabled (`is_enabled: false, reason: "no_effect"`), as the game disables a
//   chosen option, and choose_option refuses it (`option_unavailable`). The mark lasts
//   until the event's state changes. An option that threw is reported as `engine_error`
//   as before, and the re-read shows the page the same way.
// - An unfinished event with no option is a failure (`event_without_options`), not a
//   silent move to the map. Nothing in v0.107.1 reaches it.
//
// Trial's DOUBLE_DOWN, whose only effect is the abandon-run popup, is listed disabled with
// reason `abandons_run` and refused: the interface offers no abandon.
//
// Each option carries the cards its hover tips show (HoverTipFactory.FromCard: Slippery
// Bridge's card to lose, Tinker Time's Mad Science previews) as `cards`, in the deck's card
// JSON. An option's own text variables (set on its title or description rather than on the
// event: The Future of Potions' potion, rarities and card type) are sent too: the title
// rendered with its own, the description's in `vars`. With agent mode off none of this runs.

using System.Text.RegularExpressions;
using MegaCrit.Sts2.Core.Events;
using MegaCrit.Sts2.Core.HoverTips;
using MegaCrit.Sts2.Core.Models;

namespace Sts2Headless;

public partial class RunSimulator
{
    /// <summary>The event whose options had no effect, the event's state count then, and those options.</summary>
    private EventModel? _agentNoEffectEvent;
    private int _agentNoEffectCount;
    private readonly HashSet<int> _agentNoEffectOptions = new();

    /// <summary>The option index of the last choose_option at an event (agent mode).</summary>
    private int _agentLastEventOption = -1;

    /// <summary>Mark an option that changed nothing on the event's current page.</summary>
    private void AgentMarkNoEffect(EventModel ev, int index)
    {
        var count = Volatile.Read(ref _eventAdvanceCount);
        if (!ReferenceEquals(ev, _agentNoEffectEvent) || count != _agentNoEffectCount)
        {
            _agentNoEffectOptions.Clear();
            _agentNoEffectEvent = ev;
            _agentNoEffectCount = count;
        }
        _agentNoEffectOptions.Add(index);
        Log($"Event {ev.GetType().Name}: option {index} changed nothing; shown again, disabled");
    }

    private bool AgentOptionHadNoEffect(EventModel ev, int index) =>
        ReferenceEquals(ev, _agentNoEffectEvent) && ReferenceEquals(ev, _advanceTrackedEvent)
        && Volatile.Read(ref _eventAdvanceCount) == _agentNoEffectCount && _agentNoEffectOptions.Contains(index);

    /// <summary>Trial's DOUBLE_DOWN: its body only opens the abandon-run popup (IL).</summary>
    private static bool AbandonsRun(EventModel ev, EventOption opt) =>
        ev.Id?.Entry == "TRIAL" && opt.TextKey?.EndsWith(".DOUBLE_DOWN") == true;

    /// <summary>Agent mode's extra fields on one event option (see the file header).</summary>
    private void AgentEventOptionFields(Dictionary<string, object?> dict, EventModel ev, EventOption opt, int index)
    {
        var reason = AbandonsRun(ev, opt) ? "abandons_run" : AgentOptionHadNoEffect(ev, index) ? "no_effect" : null;
        if (reason != null)
        {
            dict["is_enabled"] = false;
            dict["reason"] = reason;
        }
        var cards = (opt.HoverTips ?? Enumerable.Empty<IHoverTip>()).OfType<CardHoverTip>()
            .Select(t => t.Card).Where(c => c != null).Select(AgentCardDict).ToList();
        if (cards.Count > 0) dict["cards"] = cards;

        // The option's own text variables (LocString.Add on its title or description), the
        // ones its text names: the game also gives every option's description the
        // character's pronouns and IsMultiplayer (EventOption.AddLocVars), which only a
        // template that uses them needs (Morphic Grove's GROUP). The title is sent rendered,
        // as its button shows it; the description's variables join `vars`, over the event's
        // own of the same name. The two sets stay apart: in The Future of Potions `Rarity` is
        // the potion's rarity in the title ("Insert Event Potion") but the promised card's in
        // the description ({Potion}, {Rarity}, {Type}).
        var titleVars = AgentOwnVars(opt.Title, dict.GetValueOrDefault("title") as string);
        if (titleVars.Count > 0 && dict.GetValueOrDefault("title") is string template)
            dict["title"] = OwnVar.Replace(template, m =>
                titleVars.TryGetValue(m.Groups[1].Value, out var v) && m.Groups[2].Length == 0
                    ? v?.ToString() ?? "" : m.Value);
        var descriptionVars = AgentOwnVars(opt.Description, dict.GetValueOrDefault("description") as string);
        if (descriptionVars.Count > 0)
        {
            var vars = dict.GetValueOrDefault("vars") as Dictionary<string, object?> ?? new();
            foreach (var (name, value) in descriptionVars) vars[name] = value;
            dict["vars"] = vars;
        }
    }

    /// <summary>A placeholder in a loc template: its name, and a format if it has one ({Cards:plural:...}).</summary>
    private static readonly Regex OwnVar = new(@"\{(\w+)(:[^}]*)?\}");

    /// <summary>
    /// The variables a LocString carries that its template names, as the game shows them:
    /// text rendered, numbers as numbers, flags as flags.
    /// </summary>
    private Dictionary<string, object?> AgentOwnVars(MegaCrit.Sts2.Core.Localization.LocString? loc, string? template)
    {
        var vars = new Dictionary<string, object?>();
        if (loc == null || string.IsNullOrEmpty(template)) return vars;
        var named = OwnVar.Matches(template).Select(m => m.Groups[1].Value).ToHashSet();
        try
        {
            foreach (var (name, value) in loc.Variables)
            {
                if (!named.Contains(name)) continue;
                vars[name] = value switch
                {
                    MegaCrit.Sts2.Core.Localization.DynamicVars.StringVar sv => AgentLocText(sv.StringValue ?? ""),
                    MegaCrit.Sts2.Core.Localization.DynamicVars.DynamicVar dv => (int)dv.BaseValue,
                    MegaCrit.Sts2.Core.Localization.LocString nested => LocText(nested),
                    string text => AgentLocText(text),
                    bool or int => value,
                    _ => value?.ToString(),
                };
            }
        }
        catch (Exception ex) { Log($"Event option vars unreadable: {ex.Message}"); }
        return vars;
    }

    /// <summary>A card in the deck's JSON, with agent mode's extras.</summary>
    private Dictionary<string, object?> AgentCardDict(CardModel card)
    {
        var dict = DeckCardDict(card);
        AgentCardExtras(dict, card);
        return dict;
    }

    /// <summary>The finished event, reported on the response that follows.</summary>
    private void AgentEventFinished(EventModel? ev)
    {
        if (!_agentMode || ev == null) return;
        _agentForced.Add(new() { ["kind"] = "event_finished", ["event_id"] = ev.Id?.Entry ?? ev.GetType().Name.ToUpperInvariant() });
    }

    /// <summary>An unfinished event with no option to choose (agent mode): a failure to diagnose.</summary>
    private Dictionary<string, object?> AgentEventWithoutOptions(EventModel ev) =>
        Failure("error", "completed", $"event {ev.Id?.Entry} is unfinished and offers no option", "an event option",
            "event_without_options", new() { ["event_id"] = ev.Id?.Entry });

    private static readonly Regex UpgradedTitle = new(@"^(.+\.title)(\+\d*)$");

    /// <summary>
    /// A text variable as the game shows it. A card title is its loc key with the upgrade
    /// suffix appended (CardModel.Title: "BASH.title+" in Headless, where loc isn't loaded),
    /// so the key is rendered and the suffix kept: "Bash+".
    /// </summary>
    private string AgentLocText(string text)
    {
        var m = UpgradedTitle.Match(text);
        if (m.Success)
        {
            var title = _loc.BilingualFromKey(m.Groups[1].Value);
            if (title != m.Groups[1].Value) return title + m.Groups[2].Value;
        }
        return _loc.BilingualFromKey(text);
    }
}
