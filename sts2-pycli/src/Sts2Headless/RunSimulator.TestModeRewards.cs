// sts2-agent fork: the rewards two relics offer on pickup, as the game rolls them.
//
// The host runs with TestMode.IsOn = true (EnsureModelDbInitialized; much of the game's
// headless-safety rests on it), and two relics pick their pickup rewards on that flag
// (v0.107.1):
//
//   CallingBell.GenerateRewards: under TestMode three fixed RelicRewards (Anchor, Gremlin
//     Horn, Mummified Hand); otherwise RelicReward(Common), (Uncommon), (Rare), each pulled
//     from the player's relic grab bag when the reward is populated.
//   Cauldron.GenerateRewards: under TestMode the fixed _testPotions (Flex, Weak, Vulnerable,
//     Strength, Dexterity); otherwise DynamicVars["Potions"] PotionRewards rolled from the
//     player's Rewards stream when populated.
//
// These are the only TestMode branches in the game that choose rewards, relics or potions
// (the others are UI, assets, saves, VFX and the rewards auto-taker the host already
// replaces). Each prefix returns the game's non-test list, so the real game's rewards are
// offered while TestMode stays on everywhere else. Not gated on agent mode: the non-test
// list is the game's in either mode.

using HarmonyLib;
using MegaCrit.Sts2.Core.Entities.Relics;
using MegaCrit.Sts2.Core.Models.Relics;
using MegaCrit.Sts2.Core.Rewards;

namespace Sts2Headless;

public partial class RunSimulator
{
    private static bool _testModeRewardPatchesInstalled;

    private static void InstallTestModeRewardPatches()
    {
        if (_testModeRewardPatchesInstalled) return;
        _testModeRewardPatchesInstalled = true;
        var harmony = new Harmony("sts2headless.testmoderewards");
        foreach (var (type, prefix) in new[]
                 {
                     (typeof(CallingBell), nameof(CallingBellGenerateRewardsPrefix)),
                     (typeof(Cauldron), nameof(CauldronGenerateRewardsPrefix)),
                 })
        {
            var method = AccessTools.Method(type, "GenerateRewards");
            if (method == null)
            {
                Log($"[WARN] {type.Name}.GenerateRewards not found; its TestMode rewards not patched");
                continue;
            }
            harmony.Patch(method, new HarmonyMethod(typeof(RunSimulator), prefix));
        }
    }

    /// <summary>CallingBell.GenerateRewards' non-test branch: a Common, an Uncommon and a Rare relic.</summary>
    private static bool CallingBellGenerateRewardsPrefix(CallingBell __instance, ref List<Reward> __result)
    {
        __result = new List<Reward>
        {
            new RelicReward(RelicRarity.Common, __instance.Owner),
            new RelicReward(RelicRarity.Uncommon, __instance.Owner),
            new RelicReward(RelicRarity.Rare, __instance.Owner),
        };
        return false;
    }

    /// <summary>Cauldron.GenerateRewards' non-test branch: DynamicVars["Potions"] rolled potion rewards.</summary>
    private static bool CauldronGenerateRewardsPrefix(Cauldron __instance, ref List<Reward> __result)
    {
        var count = __instance.DynamicVars["Potions"].IntValue;
        var list = new List<Reward>(count);
        for (var i = 0; i < count; i++)
            list.Add(new PotionReward(__instance.Owner));
        __result = list;
        return false;
    }
}
