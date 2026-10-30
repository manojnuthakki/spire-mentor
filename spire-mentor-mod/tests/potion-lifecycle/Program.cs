// Drives GameAction.Execute (the game's class) and checks what SimActionWatch reports.
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
Console.WriteLine(failures == 0 ? "all passed" : $"{failures} failed");
return failures == 0 ? 0 : 1;

sealed class Probe(Func<Probe, Task> body) : GameAction
{
    public override ulong OwnerId => 0;
    public override GameActionType ActionType => GameActionType.NonCombat;
    protected override Task ExecuteAction() => body(this);
    public override INetAction ToNetAction() => throw new NotSupportedException();
}
