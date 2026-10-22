// sts2-agent fork: an event that resumes after its fight, in agent mode.
//
// An event option can start a fight inside the event (EventModel.EnterCombatWithoutExitingEvent).
// Most such fights end the room on the combat's own rewards screen (Dense Vegetation, Punch
// Off, The Lantern Key). One asks to resume the event afterwards (shouldResumeAfterCombat:
// Battleworn Dummy): leaving the won fight runs RunManager.ResumePreviousRoom ->
// EventRoom.Resume -> EventSynchronizer.ResumeEvents, which starts each event's Resume()
// fire-and-forget (TaskHelper.RunSafely). Battleworn Dummy's Resume finishes the event and
// then pays out by setting: a potion offered with RewardsCmd.OfferCustom (setting 1), two
// random upgrades (setting 2), a relic obtained (setting 3); nothing when the 3 turns ran out.
//
// Upstream's Resume ran outside any task that surfaces custom rewards, so setting 1's potion
// was taken straight into the belt (and lost with a full belt), and nothing waited for Resume
// before the host left the finished event. In agent mode ResumeEvents is replaced by the same
// loop, with each Resume started under SurfaceCustomRewards and kept: rewards it offers come
// to the `rewards` decision (`source: custom`), a prompt it opens (a relic's own choice) to
// the ordinary decisions, and the host drives it to completion as it drives a rest-site option
// (RunSimulator.RestTreasure.cs) before reading the next decision. A Resume that doesn't finish
// within the settle cap is reported as `timeout, started, event_resume_not_settled` (and again
// on each re-read until it does), not left for the map. With agent mode off none of this runs.

using HarmonyLib;
using MegaCrit.Sts2.Core.Helpers;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Multiplayer.Game;
using MegaCrit.Sts2.Core.Rooms;
using MegaCrit.Sts2.Core.Runs;

namespace Sts2Headless;

public partial class RunSimulator
{
    /// <summary>The events' Resume() after an event fight, until it has completed (agent mode).</summary>
    private static Task? _agentEventResumeTask;

    private static bool _eventResumePatchInstalled;

    private static readonly System.Reflection.FieldInfo? EventSynchronizerEvents =
        AccessTools.Field(typeof(EventSynchronizer), "_events");

    private static void InstallEventResumePatch()
    {
        if (_eventResumePatchInstalled) return;
        _eventResumePatchInstalled = true;
        var method = AccessTools.Method(typeof(EventSynchronizer), nameof(EventSynchronizer.ResumeEvents));
        if (method == null || EventSynchronizerEvents == null)
        {
            Log("[WARN] EventSynchronizer.ResumeEvents or _events not found; event resume not patched");
            return;
        }
        new Harmony("sts2headless.eventresume").Patch(method,
            new HarmonyMethod(typeof(RunSimulator), nameof(ResumeEventsPrefix)));
    }

    /// <summary>
    /// EventSynchronizer.ResumeEvents (v0.107.1: each event's Resume(exitedRoom) through
    /// TaskHelper.RunSafely), with rewards the Resume offers surfaced and its task kept.
    /// </summary>
    private static bool ResumeEventsPrefix(EventSynchronizer __instance, AbstractRoom exitedRoom)
    {
        if (!AgentModeActive) return true;
        var tasks = new List<Task>();
        foreach (var ev in (List<EventModel>)EventSynchronizerEvents!.GetValue(__instance)!)
        {
            // Resume runs here, synchronously up to its first await, as in the game; the flag is
            // captured by its continuations and restored for the caller.
            var outer = SurfaceCustomRewards.Value;
            SurfaceCustomRewards.Value = true;
            try
            {
                var task = ev.Resume(exitedRoom);
                TaskHelper.RunSafely(task);
                tasks.Add(task);
            }
            finally { SurfaceCustomRewards.Value = outer; }
        }
        _agentEventResumeTask = tasks.Count == 1 ? tasks[0] : Task.WhenAll(tasks);
        return false;
    }

    /// <summary>Set once a resumed event has not finished within the settle cap (it hung).</summary>
    private static bool _agentEventResumeHung;

    /// <summary>
    /// Drive a resumed event once what it opened has been answered: it completes or suspends
    /// at the next prompt. Called before reading the decision point. The first wait is bounded
    /// by the settle cap; after that (a Resume that hung) each re-read only pumps and looks, as
    /// a hung Crystal Sphere divination does, and <see cref="EventResumeNotSettled"/> reports it.
    /// </summary>
    private void DriveAgentEventResume()
    {
        var task = _agentEventResumeTask;
        if (task == null || AnySelectionPending()) return;
        if (_agentEventResumeHung) _syncCtx.Pump();
        else if (!WaitFor(() => task.IsCompleted || AnySelectionPending(), SettleCapMs, "event resume after its fight"))
        {
            _agentEventResumeHung = true;
            return;
        }
        if (!task.IsCompleted) return;  // suspended at what it opened
        _agentEventResumeTask = null;
        _agentEventResumeHung = false;
        // A fault was already reported once, by the game's own TaskHelper.RunSafely.
        WaitForActionExecutor();
        _syncCtx.Pump();
    }

    /// <summary>
    /// A resumed event that hasn't finished and isn't waiting on the client: no decision is
    /// presented (in particular the finished event isn't left for the map) until it completes.
    /// </summary>
    private Dictionary<string, object?>? EventResumeNotSettled()
    {
        if (_agentEventResumeTask is not { IsCompleted: false } || AnySelectionPending()) return null;
        var ev = RunManager.Instance.EventSynchronizer?.GetLocalEvent();
        return Failure("timeout", "started", "the event's resume after its fight did not finish",
            "the event's Resume to complete", "event_resume_not_settled",
            new() { ["event_id"] = ev?.Id?.Entry });
    }

    private static void ResetAgentEventResume()
    {
        _agentEventResumeTask = null;
        _agentEventResumeHung = false;
    }
}
