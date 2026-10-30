// SimCombatTurn.cs -- when the combat handler may ask the agent for a play, and one turn's ask loop (sts2-agent fork, D1b).
//
// A death mid-play (Breakthrough or Hemokinesis at 1 HP; met on Live at act 1's boss, 2 of 5 full runs) ends the fight
// with the player's turn phase still Play: CombatManager.LoseCombat marks the loss pending and ProcessPendingLoss sets
// IsInProgress false, but neither touches the phase. A loop that asked while the phase was Play sent the dead board
// again and waited for an answer the agent never owes, so Autopilot never reached the game over screen. Asking needs
// the play phase and a fight still going: in progress, not ending (a loss pending, or no living primary enemy and no
// hook holding the fight open: CombatManager.IsEnding) and the player alive. It is checked before each settle wait and
// again after it, since the fight can end while the last play settles.
// Depends on nothing of the game's (the probes are passed in), so tests/potion-lifecycle drives it.

using System;
using System.Threading;
using System.Threading.Tasks;

namespace SpireMentor;

internal static class SimCombatTurn
{
    /// <summary>Whether the fight is over for the handler: ended, ending, or the player dead.</summary>
    public static bool Over(bool inProgress, bool ending, bool playerDead) => !inProgress || ending || playerDead;

    /// <summary>Whether the handler may ask the agent for a play.</summary>
    public static bool MayAsk(bool playPhase, bool inProgress, bool ending, bool playerDead) =>
        playPhase && !Over(inProgress, ending, playerDead);

    /// <summary>
    /// One turn's decisions: settle, then ask and carry out the answer, until the agent ends the turn
    /// (true), <paramref name="cap"/> answers were taken, or <paramref name="mayAsk"/> no longer holds,
    /// before a settle wait or after it (false). <paramref name="beforeEach"/> gets the answers taken so far.
    /// </summary>
    public static async Task<bool> RunAsync(Func<bool> mayAsk, Func<Task> settle, Func<Task<bool>> askAndExecute,
        Action<int> beforeEach, int cap, CancellationToken ct)
    {
        for (int taken = 0; taken < cap && mayAsk(); taken++)
        {
            ct.ThrowIfCancellationRequested();
            beforeEach(taken);
            await settle();
            if (!mayAsk())
                return false; // the turn or the fight ended as the last play settled: nothing to ask
            if (await askAndExecute())
                return true;
        }
        return false;
    }
}
