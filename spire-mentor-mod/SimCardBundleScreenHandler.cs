// SimCardBundleScreenHandler.cs -- card bundle choice screen, decided by the agent.
// Originally ported from zhiyue/sts2-rl-agent (STS2BridgeMod); rewritten clean-room.
// Phase 1a: envelope + shared card serializer (see SimStateExtractor.cs).

using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Godot;
using MegaCrit.Sts2.Core.AutoSlay;
using MegaCrit.Sts2.Core.AutoSlay.Handlers;
using MegaCrit.Sts2.Core.AutoSlay.Helpers;
using MegaCrit.Sts2.Core.Context;
using MegaCrit.Sts2.Core.Models;
using MegaCrit.Sts2.Core.Nodes.Cards;
using MegaCrit.Sts2.Core.Nodes.CommonUi;
using MegaCrit.Sts2.Core.Nodes.Screens.CardSelection;
using MegaCrit.Sts2.Core.Random;
using MegaCrit.Sts2.Core.Runs;

namespace SpireMentor;

public class SimCardBundleScreenHandler : IScreenHandler, IHandler
{
    private const string ScreenName = nameof(NChooseABundleSelectionScreen);
    private const int PreviewSettleMs = 500;
    private static TimeSpan ConfirmCloseWait => SimConfig.Instance.Scaled(TimeSpan.FromSeconds(10));

    public Type ScreenType => typeof(NChooseABundleSelectionScreen);

    public TimeSpan Timeout => TimeSpan.FromHours(24);

    public async Task HandleAsync(Rng random, CancellationToken ct)
    {
        AutoSlayLog.EnterScreen(ScreenName);
        NChooseABundleSelectionScreen screen = AutoSlayer.GetCurrentScreen<NChooseABundleSelectionScreen>();

        List<NCardBundle> bundles = UiHelper.FindAll<NCardBundle>(screen);
        if (bundles.Count == 0)
        {
            AutoSlayLog.Warn("[SimCardBundle] No card bundles on screen");
            return;
        }

        NCardBundle chosen = await ChooseBundle(bundles, ct);
        Logger.Log($"[SimCardBundle] Picking bundle {bundles.IndexOf(chosen)}");
        await UiHelper.Click(chosen.Hitbox);
        // The screen previews the pick before it is confirmed.
        await Task.Delay(PreviewSettleMs, ct);

        NConfirmButton? confirm = UiHelper.FindFirst<NConfirmButton>(screen);
        if (confirm != null)
        {
            Logger.Log("[SimCardBundle] Confirming bundle");
            await UiHelper.Click(confirm);
            await WaitHelper.Until(() => !GodotObject.IsInstanceValid(screen) || !screen.IsVisibleInTree(),
                ct, ConfirmCloseWait, "Card bundle screen did not close after confirm");
        }

        AutoSlayLog.ExitScreen(ScreenName);
    }

    /// <summary>Asks the agent; the list travels under "bundles", not "options".</summary>
    private static async Task<NCardBundle> ChooseBundle(List<NCardBundle> bundles, CancellationToken ct)
    {
        List<Dictionary<string, object>> payload = bundles.Select((b, i) => BundleOption(b, i)).ToList();
        int index = await SimAgentChoice.AskAsync(
            (_, _) => new Dictionary<string, object>
            {
                [NonCombatBridgeProtocol.TypeField] = NonCombatBridgeProtocol.CardBundleState,
                ["bundles"] = payload,
            },
            bundles.Count, NonCombatBridgeProtocol.CardBundleState, "SimCardBundle", ct);
        return bundles[index];
    }

    private static Dictionary<string, object> BundleOption(NCardBundle bundle, int index) => new()
    {
        ["index"] = index,
        ["action"] = NonCombatBridgeProtocol.PickCardBundleAction,
        ["cards"] = bundle.Bundle.Select(card => SimStateExtractor.SerializeCard(card)).ToList(),
        ["enabled"] = true,
    };
}
