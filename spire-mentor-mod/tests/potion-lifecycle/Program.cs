// Drives GameAction.Execute (the game's class) and checks what SimActionWatch and SimCombatSettle report.
using MegaCrit.Sts2.Core.Entities.Actions;
using MegaCrit.Sts2.Core.Entities.Multiplayer;
using MegaCrit.Sts2.Core.GameActions;
using MegaCrit.Sts2.Core.GameActions.Multiplayer;
using SpireMentor;

int failures = 0;
async Task Check(string name, Func<Probe, Task> body, bool enqueue, bool cancelFirst, SimActionResult expected)
{
    var action = new Probe(body);
    var watch = new SimActionWatch(action);
    if (watch.Result != SimActionResult.Pending) { Console.WriteLine($"FAIL {name}: settled before running"); failures++; }
    if (enqueue) action.OnEnqueued(_ => { }, 1);
    if (cancelFirst) action.Cancel();
    else
    {
        // Execute rethrows a fault after its `finally` set Finished (TaskHelper.WhenAny): the
        // game's queue sees it, the watch must too.
        try { await action.Execute(); }
        catch (Exception ex) { Console.WriteLine($"     Execute threw {ex.GetType().Name}: {ex.Message}"); }
    }
    SimActionResult got = watch.Result;
    bool ok = got == expected;
    Console.WriteLine($"{(ok ? "ok  " : "FAIL")} {name}: {got} (state {action.State})");
    if (!ok) failures++;
}

await Check("finishes", _ => Task.CompletedTask, true, false, SimActionResult.Done);
await Check("finishes after awaiting", async _ => await Task.Delay(20), true, false, SimActionResult.Done);
await Check("cancels inside ExecuteAction", a => { a.Cancel(); return Task.CompletedTask; }, true, false, SimActionResult.Canceled);
await Check("cancelled before it runs", _ => Task.CompletedTask, true, true, SimActionResult.Canceled);
// ExecuteAction is async in every game action: a throw is held by its task.
await Check("throws", _ => Task.FromException(new InvalidOperationException("effect failed")), true, false, SimActionResult.Faulted);
await Check("throws after awaiting", async _ => { await Task.Delay(20); throw new InvalidOperationException("late"); }, true, false, SimActionResult.Faulted);
// A cancelled execution task without the game's Cancel: an interrupted effect, not a success.
await Check("its task is cancelled", _ => Task.FromCanceled(new CancellationToken(true)), true, false, SimActionResult.Faulted);
await Check("the effect throws OperationCanceledException", async _ => { await Task.Delay(20); throw new OperationCanceledException("interrupted"); }, true, false, SimActionResult.Faulted);

// SimCombatSettle (combat_action's `settled`): an action parked on a selection, answered, resuming, ended.
void Expect(string name, bool got, bool expected)
{
    bool ok = got == expected;
    Console.WriteLine($"{(ok ? "ok  " : "FAIL")} settled {name}: {got}");
    if (!ok) failures++;
}
Expect("with no action yet", SimCombatSettle.Settled(null, false, false), true);
Expect("with no action, a selection open", SimCombatSettle.Settled(null, true, false), false);
Expect("with no action, the executor running", SimCombatSettle.Settled(null, false, true), false);
var resume = new TaskCompletionSource();
var picked = new Probe(async a =>
{
    a.PauseForPlayerChoice();
    await a.WaitForActionToResumeExecutingAfterPlayerChoice();
    await resume.Task;  // the effect after the pick, still running
});
var pickWatch = new SimActionWatch(picked);
picked.OnEnqueued(_ => { }, 1);
Expect("enqueued, not run", SimCombatSettle.Settled(pickWatch, false, false), false);
await picked.Execute();  // returns as it parks on the choice
Expect($"parked on the selection ({picked.State})", SimCombatSettle.Settled(pickWatch, true, false), false);
Expect("parked, the selection's flag already down", SimCombatSettle.Settled(pickWatch, false, false), false);
picked.ResumeAfterGatheringPlayerChoice(2);
Expect($"answered, not resumed ({picked.State})", SimCombatSettle.Settled(pickWatch, false, false), false);
Task resumed = picked.Execute();
Expect($"resumed, its effect running ({picked.State})", SimCombatSettle.Settled(pickWatch, false, true), false);
Expect("resumed, the executor between actions", SimCombatSettle.Settled(pickWatch, false, false), false);
resume.SetResult();
await resumed;
Expect($"ended ({picked.State}, {pickWatch.Result})", SimCombatSettle.Settled(pickWatch, false, false), true);
Expect("ended, a follow-up action ready", SimCombatSettle.Settled(pickWatch, false, true), false);
Expect("ended, another selection open", SimCombatSettle.Settled(pickWatch, true, false), false);
var cancelled = new Probe(_ => Task.CompletedTask);
var cancelWatch = new SimActionWatch(cancelled);
cancelled.OnEnqueued(_ => { }, 3);
cancelled.Cancel();
Expect("cancelled before it ran", SimCombatSettle.Settled(cancelWatch, false, false), true);
var faulted = new Probe(_ => Task.FromException(new InvalidOperationException("effect failed")));
var faultWatch = new SimActionWatch(faulted);
faulted.OnEnqueued(_ => { }, 4);
try { await faulted.Execute(); } catch (InvalidOperationException) { }
Expect($"faulted ({faultWatch.Result})", SimCombatSettle.Settled(faultWatch, false, false), true);

// SimCombatTurn (the combat handler's ask loop): a fight that ends, or a player who dies, before or while the last play
// settles is never asked about again, the phase still Play (a death mid-play: LoseCombat leaves the phase alone).
void Turn(string name, bool got, bool expected)
{
    bool ok = got == expected;
    Console.WriteLine($"{(ok ? "ok  " : "FAIL")} turn {name}: {got}");
    if (!ok) failures++;
}
Turn("may ask in play, the fight going on", SimCombatTurn.MayAsk(true, true, false, false), true);
Turn("not asked out of the play phase", SimCombatTurn.MayAsk(false, true, false, false), false);
Turn("not asked, the player dead, the phase still Play", SimCombatTurn.MayAsk(true, true, false, true), false);
Turn("not asked, a loss pending (ending)", SimCombatTurn.MayAsk(true, true, true, false), false);
Turn("not asked, the fight over, the phase still Play", SimCombatTurn.MayAsk(true, false, false, false), false);
Turn("over when the player is dead", SimCombatTurn.Over(true, false, true), true);
Turn("not over while it goes on", SimCombatTurn.Over(true, false, false), false);

// The loop itself, its probes scripted: `dead` and `over` flip where the game would flip them.
async Task<(bool ended, int asked, int settled)> Loop(Func<int, bool> diesAtSettle, Func<int, bool> endsTurnAt, int cap = 50,
    bool deadBefore = false)
{
    bool dead = deadBefore;
    int asked = 0, settled = 0;
    bool ended = await SimCombatTurn.RunAsync(
        () => SimCombatTurn.MayAsk(true, true, false, dead),
        () => { if (diesAtSettle(settled++)) dead = true; return Task.CompletedTask; },
        () => Task.FromResult(endsTurnAt(asked++)),
        _ => { }, cap, CancellationToken.None);
    return (ended, asked, settled);
}
var r = await Loop(_ => false, n => n == 2);
Turn("the agent ends the turn on its third answer", r.ended && r.asked == 3, true);
r = await Loop(n => n == 1, _ => false);  // the second play settles into a death (Breakthrough at 1 HP)
Turn($"a death as a play settles: no ask after it (asked {r.asked}, settled {r.settled})", !r.ended && r.asked == 1 && r.settled == 2, true);
r = await Loop(_ => false, _ => false, deadBefore: true);
Turn($"dead before the turn's first ask: nothing asked or settled ({r.asked}, {r.settled})", r.asked == 0 && r.settled == 0, true);
r = await Loop(_ => false, _ => false, cap: 4);
Turn($"the cap ends the turn unended ({r.asked})", !r.ended && r.asked == 4, true);

Console.WriteLine(failures == 0 ? "all passed" : $"{failures} failed");
return failures == 0 ? 0 : 1;

sealed class Probe(Func<Probe, Task> body) : GameAction
{
    public override ulong OwnerId => 0;
    public override GameActionType ActionType => GameActionType.NonCombat;
    protected override Task ExecuteAction() => body(this);
    public override INetAction ToNetAction() => throw new NotSupportedException();
}
