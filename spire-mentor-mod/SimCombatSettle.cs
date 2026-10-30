// SimCombatSettle.cs -- whether a combat board is settled, for combat_action's `settled` (sts2-agent fork, D1b).
//
// A card play or a potion that opens a selection (Armaments, Skill / Attack / Colorless Potion, Ashwater)
// parks its game action on the player's choice: GatheringPlayerChoice while the agent picks, then
// ReadyToResumeExecuting once answered, Executing again, and Finished (or Canceled) at its end. A board
// serialized anywhere before that end may still change with nothing more asked of the agent. Settled
// means all three hold:
//   - the last play or potion the agent asked for has ended (SimActionWatch: Done, Canceled or Faulted;
//     never Pending, which covers waiting, executing, parked on a choice and resuming);
//   - no selection is open (SimCardSelector.SelectionOpen: an answer the agent still owes);
//   - the game's ActionExecutor has nothing ready to run (IsRunning false: its loop ends only when no
//     queue has a ready action, so follow-up actions the play enqueued, hooks included, are done too).
// Depends on GameAction only (the inputs are passed in), so tests/potion-lifecycle drives it against
// the game's own class.

namespace SpireMentor;

internal static class SimCombatSettle
{
    /// <summary>Whether the board can no longer change without the agent's next answer.</summary>
    public static bool Settled(SimActionWatch? last, bool selectionOpen, bool executorRunning) =>
        !selectionOpen && !executorRunning && (last == null || last.Result != SimActionResult.Pending);
}
