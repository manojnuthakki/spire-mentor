// SimActionOutcome.cs -- how a game action the bridge enqueued ended (sts2-agent fork, D1b).
//
// GameAction.State alone can't say: GameAction.Execute sets Finished in its `finally` whenever
// the execution task has completed, also when ExecuteAction called Cancel() (State went to
// Canceled, then to Finished) or threw (GameAction.Exception set; TaskHelper.RunSafely logs and
// rethrows) or its task was cancelled (an OperationCanceledException: Exception stays null). So
// the game's own cancellation is caught as it happens (BeforeCancelled), and success is read
// positively from the execution task (GameAction._executionTask, private: by reflection),
// never from a missing exception. Depends on GameAction only, so tests/potion-lifecycle drives
// it against the game's own class.

using System.Reflection;
using System.Threading.Tasks;
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
    /// <summary>
    /// Threw, or its execution task was cancelled without the game's Cancel (an interrupted
    /// effect), or its task can't be read: what it changed is unknown.
    /// </summary>
    Faulted,
}

internal sealed class SimActionWatch
{
    private static readonly FieldInfo? ExecutionTaskField =
        typeof(GameAction).GetField("_executionTask", BindingFlags.Instance | BindingFlags.NonPublic);

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
            Task? execution = ExecutionTaskField?.GetValue(Action) as Task;
            if (execution == null)
            {
                // Never ran: cancelled before execution (the queue, or Cancel()); anything else
                // settled without a task can't be shown to have succeeded.
                return state == GameActionState.Canceled && _cancelled && ExecutionTaskField != null
                    ? SimActionResult.Canceled
                    : SimActionResult.Faulted;
            }
            if (!execution.IsCompleted)
                return SimActionResult.Pending;  // Canceled set mid-execution: wait for the task
            if (!execution.IsCompletedSuccessfully)
                return SimActionResult.Faulted;  // threw, or the task was cancelled
            return _cancelled ? SimActionResult.Canceled : SimActionResult.Done;
        }
    }
}
