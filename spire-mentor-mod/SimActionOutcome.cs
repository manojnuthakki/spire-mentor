// SimActionOutcome.cs -- how a game action the bridge enqueued ended (sts2-agent fork, D1b).
//
// GameAction.State alone can't say: GameAction.Execute sets Finished in its `finally` whenever
// the execution task has completed, also when ExecuteAction called Cancel() (State went to
// Canceled, then to Finished) or threw (GameAction.Exception set; TaskHelper.RunSafely logs and
// rethrows). So the cancellation is caught as it happens (BeforeCancelled) and the fault read
// from the action. Depends on GameAction only, so tests/potion-lifecycle drives it against the
// game's own class.

using MegaCrit.Sts2.Core.Entities.Actions;
using MegaCrit.Sts2.Core.GameActions;

namespace SpireMentor;

internal enum SimActionResult
{
    /// <summary>Not settled yet (waiting, executing, or paused on a player choice).</summary>
    Pending,
    /// <summary>Ran to its end without a cancellation or a fault.</summary>
    Done,
    /// <summary>
    /// Cancelled. The potion actions cancel only before their effect starts (no potion in the
    /// slot, an invalid target: UsePotionAction / DiscardPotionGameAction.ExecuteAction), and
    /// the queue cancels only actions not yet run, so nothing changed.
    /// </summary>
    Canceled,
    /// <summary>Threw: what it changed before the fault is unknown.</summary>
    Faulted,
}

internal sealed class SimActionWatch
{
    private bool _cancelled;

    /// <summary>Watch <paramref name="action"/>; call before it is enqueued.</summary>
    public SimActionWatch(GameAction action)
    {
        Action = action;
        action.BeforeCancelled += _ => _cancelled = true;
    }

    public GameAction Action { get; }

    public SimActionResult Result
    {
        get
        {
            GameActionState state = Action.State;
            if (state != GameActionState.Finished && state != GameActionState.Canceled)
                return SimActionResult.Pending;
            if (Action.Exception != null)
                return SimActionResult.Faulted;
            return _cancelled || state == GameActionState.Canceled ? SimActionResult.Canceled : SimActionResult.Done;
        }
    }
}
