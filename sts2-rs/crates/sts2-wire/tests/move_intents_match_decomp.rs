//! Every move's declared intents are the telegraph the game shows: the same
//! intent kinds, in order and number, as the C# `MoveState` constructor
//! (`MonsterMoveStateMachine/MoveState.cs:48-53`, `params AbstractIntent[]`).
//! The combat wire emits the whole list, so a declaration that enumerates a
//! move's mechanical effects instead (Queen's YOU_ARE_MINE once listed three
//! Debuffs for one `DebuffIntent`) shows on the Sim as extra icons.
//!
//! Reads the decompiled monsters from `STS2_DECOMP_DIR` or the sibling
//! `sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Monsters`; skips
//! (with a note) when neither is present. Engine-only states with no C#
//! `MoveState` of their own (a synthetic stun, a wake-up) aren't reached; a game
//! move is found as a wire `move_state` is, through the alias table first.

use std::collections::BTreeMap;
use std::path::PathBuf;

use sts2_content::monster_dispatch::{build_ai, MonsterSpawnArgs, SpawnPosition};
use sts2_core::MonsterKind;
use sts2_wire::state_dict::wire_move_state_alias;

fn decomp_dir() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("STS2_DECOMP_DIR").map(PathBuf::from),
        Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../sts2-wiki/decompiled/v0.107.1/MegaCrit.Sts2.Core.Models.Monsters"),
        ),
    ];
    candidates.into_iter().flatten().find(|p| p.is_dir())
}

/// The wire kind of a C# intent class (`DebuffIntent(strong: true)` is
/// `DebuffStrong`; `Hidden` collapses to the engine's `Unknown`).
fn wire_kind(class: &str, args: &str) -> String {
    match class {
        "SingleAttackIntent" | "MultiAttackIntent" => "Attack",
        "DefendIntent" => "Defend",
        "BuffIntent" => "Buff",
        "DebuffIntent" if args.contains("strong: true") => "DebuffStrong",
        "DebuffIntent" => "Debuff",
        "CardDebuffIntent" => "CardDebuff",
        "StatusIntent" => "StatusCard",
        "SummonIntent" => "Summon",
        "HealIntent" => "Heal",
        "SleepIntent" => "Sleep",
        "StunIntent" => "Stun",
        "EscapeIntent" => "Escape",
        "DeathBlowIntent" => "DeathBlow",
        "UnknownIntent" | "HiddenIntent" => "Unknown",
        other => return format!("?{other}"),
    }
    .to_owned()
}

/// `MoveState` id → its intents' wire kinds, from one monster's C# source.
fn csharp_moves(src: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut rest = src;
    while let Some(i) = rest.find("new MoveState(\"") {
        rest = &rest[i + "new MoveState(\"".len()..];
        let Some(q) = rest.find('"') else { break };
        let id = rest[..q].to_owned();
        // The constructor's argument list, to its closing parenthesis.
        let mut depth = 1;
        let mut end = q;
        for (j, ch) in rest[q..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = q + j;
                        break;
                    }
                }
                _ => {}
            }
        }
        let args = &rest[q..end];
        let mut kinds = Vec::new();
        let mut a = args;
        while let Some(k) = a.find("new ") {
            a = &a[k + 4..];
            let name: String = a.chars().take_while(|c| c.is_alphanumeric()).collect();
            let inner_end = a.find(')').unwrap_or(a.len());
            if name.ends_with("Intent") {
                kinds.push(wire_kind(&name, &a[..inner_end]));
            }
        }
        out.insert(id, kinds);
        rest = &rest[end..];
    }
    out
}

#[test]
fn every_declared_move_shows_the_games_intents() {
    let Some(dir) = decomp_dir() else {
        eprintln!("skipping: no decompiled monsters (set STS2_DECOMP_DIR)");
        return;
    };
    let mut classes: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("decomp dir").flatten() {
        let path = entry.path();
        let (Some(stem), Some("cs")) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|e| e.to_str()),
        ) else {
            continue;
        };
        let src = std::fs::read_to_string(&path).expect("readable");
        let moves = csharp_moves(&src);
        classes.insert(stem.to_ascii_uppercase(), moves);
    }
    let mut compared = 0;
    let mut wrong = Vec::new();
    for &kind in MonsterKind::ALL_KNOWN {
        let class = kind.python_name().replace('_', "").to_ascii_uppercase();
        let Some(cs) = classes.get(&class) else {
            continue;
        };
        let build = || build_ai(kind, MonsterSpawnArgs::default(), SpawnPosition::default());
        if std::panic::catch_unwind(build).is_err() {
            continue;
        }
        for (id, want) in cs {
            // The game's id resolved as a wire `move_state` is (alias table,
            // then the relaxed matcher).
            let mut ai = build();
            let resolved = wire_move_state_alias(kind, id)
                .is_some_and(|alias| ai.force_current_state_by_name(alias))
                || ai.force_current_state_by_name_relaxed(id);
            let Some(m) = resolved.then(|| ai.current_move_opt()).flatten() else {
                continue;
            };
            let have: Vec<String> = m
                .intents
                .iter()
                .map(|i| {
                    sts2_wire::emit::combat::bare_intent_to_value(*i)["kind"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect();
            compared += 1;
            if &have != want {
                wrong.push(format!("{kind:?} {id}: engine {have:?}, game {want:?}"));
            }
        }
    }
    assert!(compared > 200, "only {compared} moves matched the decomp");
    assert!(
        wrong.is_empty(),
        "declared intents differ from the game:\n{}",
        wrong.join("\n")
    );
}
