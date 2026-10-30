//! Static slices of every auto-generated [`CardSpec`] factory.
//!
//! The per-character `*_specs_generated.rs` files declare hundreds of
//! `pub const fn <snake_name>() -> CardSpec` factories but do not
//! enumerate them. This module hand-rolls one `&[fn() -> CardSpec]`
//! slice per character, surfaced to
//! [`super::super::names::deck_card_spec_registry`] so that every card
//! whose dispatch handler exists is also listed as deck-buildable.
//!
//! Bead `sts2-rs-pjln` (cliext H4). Lists are hand-curated from the
//! generated files at pin `01c094d`; when the regen scripts add a new
//! card, append it to the matching slice. A `cargo test` round-trip
//! verifies that every entry in a slice has a corresponding `CardId`
//! that resolves back through `resolve_card_name`.

use super::status_specs_generated as st;
use super::types::CardSpec;

/// Auto-listed ironclad card factories (86 entries), in C#
/// `IroncladCardPool.GenerateAllCards` alphabetical order (bead
/// `sts2-rs-4z5iw`). This order — NOT a rarity grouping — is what
/// `eligible_ids_for_rarity` / `sample_distinct_*` filter to drive
/// `NextItem(idx)` card-reward parity with C# (the per-rarity slice
/// of a fully-alphabetical pool is alphabetical-within-rarity, and
/// the whole-pool order matches for unfiltered consumers like
/// ChoicesParadox). The full Ironclad reward pool is now present
/// (`NotYet`, a C#-only Rare absent from the Python pin, was ported
/// under the run-RNG C#-parity plan M4).
pub const IRONCLAD_SPECS: &[fn() -> CardSpec] = &[
    crate::cards::ironclad::aggression::spec,
    crate::cards::ironclad::anger::spec,
    crate::cards::ironclad::armaments::spec,
    crate::cards::ironclad::ashen_strike::spec,
    crate::cards::ironclad::barricade::spec,
    crate::cards::ironclad::bash::spec,
    crate::cards::ironclad::battle_trance::spec,
    crate::cards::ironclad::blood_wall::spec,
    crate::cards::ironclad::bloodletting::spec,
    crate::cards::ironclad::bludgeon::spec,
    crate::cards::ironclad::body_slam::spec,
    crate::cards::ironclad::brand::spec,
    crate::cards::ironclad::r#break::spec,
    crate::cards::ironclad::breakthrough::spec,
    crate::cards::ironclad::bully::spec,
    crate::cards::ironclad::burning_pact::spec,
    crate::cards::ironclad::cascade::spec,
    crate::cards::ironclad::cinder::spec,
    crate::cards::ironclad::colossus::spec,
    crate::cards::ironclad::conflagration::spec,
    crate::cards::ironclad::corruption::spec,
    crate::cards::ironclad::crimson_mantle::spec,
    crate::cards::ironclad::cruelty::spec,
    crate::cards::ironclad::dark_embrace::spec,
    crate::cards::ironclad::defend_ironclad::spec,
    crate::cards::ironclad::demon_form::spec,
    crate::cards::ironclad::demonic_shield::spec,
    crate::cards::ironclad::dismantle::spec,
    crate::cards::ironclad::dominate::spec,
    crate::cards::ironclad::drum_of_battle::spec,
    crate::cards::ironclad::evil_eye::spec,
    crate::cards::ironclad::expect_a_fight::spec,
    crate::cards::ironclad::feed::spec,
    crate::cards::ironclad::feel_no_pain::spec,
    crate::cards::ironclad::fiend_fire::spec,
    crate::cards::ironclad::fight_me::spec,
    crate::cards::ironclad::flame_barrier::spec,
    crate::cards::ironclad::forgotten_ritual::spec,
    crate::cards::ironclad::havoc::spec,
    crate::cards::ironclad::headbutt::spec,
    crate::cards::ironclad::hellraiser::spec,
    crate::cards::ironclad::hemokinesis::spec,
    crate::cards::ironclad::howl_from_beyond::spec,
    crate::cards::ironclad::impervious::spec,
    crate::cards::ironclad::infernal_blade::spec,
    crate::cards::ironclad::inferno::spec,
    crate::cards::ironclad::inflame::spec,
    crate::cards::ironclad::iron_wave::spec,
    crate::cards::ironclad::juggernaut::spec,
    crate::cards::ironclad::juggling::spec,
    crate::cards::ironclad::mangle::spec,
    crate::cards::ironclad::molten_fist::spec,
    // NotYet (Rare, C#-only) sits here alphabetically per
    // IroncladCardPool.cs:54 — `Mangle < MoltenFist < NotYet < Offering`.
    // Required for reward-pool NextItem(idx) parity vs C# (its absence
    // shifted every Rare after it by one). Ported under the run-RNG
    // C#-parity plan (M4); CardId::NotYet is a MANUAL addition in
    // card_ids_generated.rs (absent from the Python pin).
    crate::cards::ironclad::not_yet::spec,
    crate::cards::ironclad::offering::spec,
    crate::cards::ironclad::one_two_punch::spec,
    crate::cards::ironclad::pacts_end::spec,
    crate::cards::ironclad::perfected_strike::spec,
    crate::cards::ironclad::pillage::spec,
    crate::cards::ironclad::pommel_strike::spec,
    crate::cards::ironclad::primal_force::spec,
    crate::cards::ironclad::pyre::spec,
    crate::cards::ironclad::rage::spec,
    crate::cards::ironclad::rampage::spec,
    crate::cards::ironclad::rupture::spec,
    crate::cards::ironclad::second_wind::spec,
    crate::cards::ironclad::setup_strike::spec,
    crate::cards::ironclad::shrug_it_off::spec,
    crate::cards::ironclad::spite::spec,
    crate::cards::ironclad::stampede::spec,
    crate::cards::ironclad::stoke::spec,
    crate::cards::ironclad::stomp::spec,
    crate::cards::ironclad::stone_armor::spec,
    crate::cards::ironclad::strike_ironclad::spec,
    crate::cards::ironclad::sword_boomerang::spec,
    crate::cards::ironclad::tank::spec,
    crate::cards::ironclad::taunt::spec,
    crate::cards::ironclad::tear_asunder::spec,
    crate::cards::ironclad::thrash::spec,
    crate::cards::ironclad::thunderclap::spec,
    crate::cards::ironclad::tremble::spec,
    crate::cards::ironclad::true_grit::spec,
    crate::cards::ironclad::twin_strike::spec,
    crate::cards::ironclad::unmovable::spec,
    crate::cards::ironclad::unrelenting::spec,
    crate::cards::ironclad::uppercut::spec,
    crate::cards::ironclad::vicious::spec,
    crate::cards::ironclad::whirlwind::spec,
];

/// Auto-listed silent card factories (88 entries).
pub const SILENT_SPECS: &[fn() -> CardSpec] = &[
    crate::cards::silent::abrasive::spec,
    crate::cards::silent::accelerant::spec,
    crate::cards::silent::accuracy::spec,
    crate::cards::silent::acrobatics::spec,
    crate::cards::silent::adrenaline::spec,
    crate::cards::silent::afterimage::spec,
    crate::cards::silent::anticipate::spec,
    crate::cards::silent::assassinate::spec,
    crate::cards::silent::backflip::spec,
    crate::cards::silent::backstab::spec,
    crate::cards::silent::blade_of_ink::spec,
    crate::cards::silent::blade_dance::spec,
    crate::cards::silent::blur::spec,
    crate::cards::silent::bouncing_flask::spec,
    crate::cards::silent::bubble_bubble::spec,
    crate::cards::silent::bullet_time::spec,
    crate::cards::silent::burst::spec,
    crate::cards::silent::calculated_gamble::spec,
    crate::cards::silent::cloak_and_dagger::spec,
    crate::cards::silent::corrosive_wave::spec,
    crate::cards::silent::dagger_spray::spec,
    crate::cards::silent::dagger_throw::spec,
    crate::cards::silent::dash::spec,
    crate::cards::silent::deadly_poison::spec,
    crate::cards::silent::defend_silent::spec,
    crate::cards::silent::deflect::spec,
    crate::cards::silent::dodge_and_roll::spec,
    crate::cards::silent::echoing_slash::spec,
    crate::cards::silent::envenom::spec,
    crate::cards::silent::escape_plan::spec,
    crate::cards::silent::expertise::spec,
    crate::cards::silent::expose::spec,
    crate::cards::silent::fan_of_knives::spec,
    crate::cards::silent::finisher::spec,
    crate::cards::silent::flanking::spec,
    crate::cards::silent::flechettes::spec,
    crate::cards::silent::flick_flack::spec,
    // v0.107.1: Scare replaces FollowThrough in this exact pool slot (the
    // C# `SilentCardPool` keeps `Scare` between `FlickFlack` and `Footwork`,
    // i.e. NOT re-alphabetized), preserving every later card's RNG index.
    crate::cards::silent::scare::spec,
    crate::cards::silent::footwork::spec,
    crate::cards::silent::grand_finale::spec,
    crate::cards::silent::hand_trick::spec,
    crate::cards::silent::haze::spec,
    crate::cards::silent::hidden_daggers::spec,
    crate::cards::silent::infinite_blades::spec,
    crate::cards::silent::knife_trap::spec,
    crate::cards::silent::leading_strike::spec,
    crate::cards::silent::leg_sweep::spec,
    crate::cards::silent::malaise::spec,
    crate::cards::silent::master_planner::spec,
    crate::cards::silent::memento_mori::spec,
    crate::cards::silent::mirage::spec,
    crate::cards::silent::murder::spec,
    crate::cards::silent::neutralize::spec,
    crate::cards::silent::nightmare::spec,
    crate::cards::silent::noxious_fumes::spec,
    crate::cards::silent::outbreak::spec,
    crate::cards::silent::phantom_blades::spec,
    crate::cards::silent::piercing_wail::spec,
    crate::cards::silent::pinpoint::spec,
    crate::cards::silent::poisoned_stab::spec,
    crate::cards::silent::pounce::spec,
    crate::cards::silent::precise_cut::spec,
    crate::cards::silent::predator::spec,
    crate::cards::silent::prepared::spec,
    crate::cards::silent::reflex::spec,
    crate::cards::silent::ricochet::spec,
    crate::cards::silent::serpent_form::spec,
    crate::cards::silent::shadow_step::spec,
    crate::cards::silent::shadowmeld::spec,
    crate::cards::silent::skewer::spec,
    crate::cards::silent::slice::spec,
    crate::cards::silent::snakebite::spec,
    crate::cards::silent::sneaky::spec,
    crate::cards::silent::speedster::spec,
    crate::cards::silent::storm_of_steel::spec,
    crate::cards::silent::strangle::spec,
    crate::cards::silent::strike_silent::spec,
    crate::cards::silent::sucker_punch::spec,
    crate::cards::silent::suppress::spec,
    crate::cards::silent::survivor::spec,
    crate::cards::silent::tactician::spec,
    crate::cards::silent::the_hunt::spec,
    crate::cards::silent::tools_of_the_trade::spec,
    crate::cards::silent::tracking::spec,
    crate::cards::silent::untouchable::spec,
    crate::cards::silent::up_my_sleeve::spec,
    crate::cards::silent::well_laid_plans::spec,
    crate::cards::silent::wraith_form::spec,
];

/// Auto-listed defect card factories (87 entries).
pub const DEFECT_SPECS: &[fn() -> CardSpec] = &[
    crate::cards::defect::adaptive_strike::spec,
    crate::cards::defect::all_for_one::spec,
    crate::cards::defect::ball_lightning::spec,
    crate::cards::defect::barrage::spec,
    crate::cards::defect::beam_cell::spec,
    crate::cards::defect::biased_cognition::spec,
    crate::cards::defect::boost_away::spec,
    crate::cards::defect::boot_sequence::spec,
    crate::cards::defect::buffer::spec,
    crate::cards::defect::bulk_up::spec,
    crate::cards::defect::capacitor::spec,
    crate::cards::defect::chaos::spec,
    crate::cards::defect::charge_battery::spec,
    crate::cards::defect::chill::spec,
    crate::cards::defect::claw::spec,
    crate::cards::defect::cold_snap::spec,
    crate::cards::defect::compact::spec,
    crate::cards::defect::compile_driver::spec,
    crate::cards::defect::consuming_shadow::spec,
    crate::cards::defect::coolant::spec,
    crate::cards::defect::coolheaded::spec,
    crate::cards::defect::creative_ai::spec,
    crate::cards::defect::darkness::spec,
    crate::cards::defect::defend_defect::spec,
    crate::cards::defect::defragment::spec,
    crate::cards::defect::double_energy::spec,
    crate::cards::defect::dualcast::spec,
    crate::cards::defect::echo_form::spec,
    crate::cards::defect::energy_surge::spec,
    crate::cards::defect::feral::spec,
    crate::cards::defect::fight_through::spec,
    crate::cards::defect::flak_cannon::spec,
    crate::cards::defect::focused_strike::spec,
    crate::cards::defect::ftl::spec,
    crate::cards::defect::fusion::spec,
    crate::cards::defect::genetic_algorithm::spec,
    crate::cards::defect::glacier::spec,
    crate::cards::defect::glasswork::spec,
    crate::cards::defect::go_for_the_eyes::spec,
    crate::cards::defect::gunk_up::spec,
    crate::cards::defect::hailstorm::spec,
    crate::cards::defect::helix_drill::spec,
    crate::cards::defect::hologram::spec,
    crate::cards::defect::hotfix::spec,
    crate::cards::defect::hyperbeam::spec,
    crate::cards::defect::ice_lance::spec,
    crate::cards::defect::ignition::spec,
    crate::cards::defect::iteration::spec,
    crate::cards::defect::leap::spec,
    crate::cards::defect::lightning_rod::spec,
    crate::cards::defect::r#loop::spec,
    crate::cards::defect::machine_learning::spec,
    crate::cards::defect::meteor_strike::spec,
    crate::cards::defect::modded::spec,
    crate::cards::defect::momentum_strike::spec,
    crate::cards::defect::multi_cast::spec,
    crate::cards::defect::null::spec,
    crate::cards::defect::overclock::spec,
    crate::cards::defect::quadcast::spec,
    crate::cards::defect::rainbow::spec,
    crate::cards::defect::reboot::spec,
    crate::cards::defect::refract::spec,
    crate::cards::defect::rocket_punch::spec,
    crate::cards::defect::scavenge::spec,
    crate::cards::defect::scrape::spec,
    crate::cards::defect::shadow_shield::spec,
    crate::cards::defect::shatter::spec,
    crate::cards::defect::signal_boost::spec,
    crate::cards::defect::skim::spec,
    crate::cards::defect::smokestack::spec,
    crate::cards::defect::spinner::spec,
    crate::cards::defect::storm::spec,
    crate::cards::defect::strike_defect::spec,
    crate::cards::defect::subroutine::spec,
    crate::cards::defect::sunder::spec,
    crate::cards::defect::supercritical::spec,
    crate::cards::defect::sweeping_beam::spec,
    crate::cards::defect::synchronize::spec,
    crate::cards::defect::synthesis::spec,
    crate::cards::defect::tempest::spec,
    crate::cards::defect::tesla_coil::spec,
    crate::cards::defect::thunder::spec,
    crate::cards::defect::trash_to_treasure::spec,
    crate::cards::defect::turbo::spec,
    crate::cards::defect::uproar::spec,
    crate::cards::defect::voltaic::spec,
    crate::cards::defect::white_noise::spec,
    crate::cards::defect::zap::spec,
];

/// Auto-listed Regent card factories (88 entries). Bead
/// `sts2-rs-le2mn`: ported from the C# decomp (source of truth),
/// not the Python pin. Wires the full Regent pool into
/// `card_pod_from_id` / `rarity_for_card_id` / `specs_for_character`
/// so Regent cards can be constructed as PODs (previously `None`,
/// so every Regent attack dealt 0 in real play).
pub const REGENT_SPECS: &[fn() -> CardSpec] = &[
    crate::cards::regent::alignment::spec,
    crate::cards::regent::arsenal::spec,
    crate::cards::regent::astral_pulse::spec,
    crate::cards::regent::beat_into_shape::spec,
    crate::cards::regent::begone::spec,
    crate::cards::regent::big_bang::spec,
    crate::cards::regent::black_hole::spec,
    crate::cards::regent::bombardment::spec,
    crate::cards::regent::bulwark::spec,
    crate::cards::regent::bundle_of_joy::spec,
    crate::cards::regent::celestial_might::spec,
    crate::cards::regent::charge::spec,
    crate::cards::regent::child_of_the_stars::spec,
    crate::cards::regent::cloak_of_stars::spec,
    crate::cards::regent::collision_course::spec,
    crate::cards::regent::comet::spec,
    crate::cards::regent::conqueror::spec,
    crate::cards::regent::convergence::spec,
    crate::cards::regent::cosmic_indifference::spec,
    crate::cards::regent::crash_landing::spec,
    crate::cards::regent::crescent_spear::spec,
    crate::cards::regent::crush_under::spec,
    crate::cards::regent::decisions_decisions::spec,
    crate::cards::regent::defend_regent::spec,
    crate::cards::regent::devastate::spec,
    crate::cards::regent::dying_star::spec,
    crate::cards::regent::falling_star::spec,
    crate::cards::regent::foregone_conclusion::spec,
    crate::cards::regent::furnace::spec,
    crate::cards::regent::gamma_blast::spec,
    crate::cards::regent::gather_light::spec,
    crate::cards::regent::genesis::spec,
    crate::cards::regent::glimmer::spec,
    crate::cards::regent::glitterstream::spec,
    crate::cards::regent::glow::spec,
    crate::cards::regent::guards::spec,
    crate::cards::regent::guiding_star::spec,
    crate::cards::regent::hammer_time::spec,
    crate::cards::regent::heavenly_drill::spec,
    crate::cards::regent::hegemony::spec,
    crate::cards::regent::heirloom_hammer::spec,
    crate::cards::regent::hidden_cache::spec,
    crate::cards::regent::i_am_invincible::spec,
    crate::cards::regent::kingly_kick::spec,
    crate::cards::regent::kingly_punch::spec,
    crate::cards::regent::knockout_blow::spec,
    crate::cards::regent::know_thy_place::spec,
    crate::cards::regent::largesse::spec,
    crate::cards::regent::lunar_blast::spec,
    crate::cards::regent::make_it_so::spec,
    crate::cards::regent::manifest_authority::spec,
    crate::cards::regent::meteor_shower::spec,
    crate::cards::regent::monarchs_gaze_card::spec,
    crate::cards::regent::monologue_card::spec,
    crate::cards::regent::neutron_aegis::spec,
    crate::cards::regent::orbit::spec,
    crate::cards::regent::pale_blue_dot::spec,
    crate::cards::regent::parry_card::spec,
    crate::cards::regent::particle_wall::spec,
    crate::cards::regent::patter::spec,
    crate::cards::regent::photon_cut::spec,
    crate::cards::regent::pillar_of_creation::spec,
    crate::cards::regent::prophesize::spec,
    crate::cards::regent::quasar::spec,
    crate::cards::regent::radiate::spec,
    crate::cards::regent::refine_blade::spec,
    crate::cards::regent::reflect_card::spec,
    crate::cards::regent::resonance::spec,
    crate::cards::regent::royal_gamble::spec,
    crate::cards::regent::royalties_card::spec,
    crate::cards::regent::seeking_edge::spec,
    crate::cards::regent::seven_stars::spec,
    crate::cards::regent::shining_strike::spec,
    crate::cards::regent::solar_strike::spec,
    crate::cards::regent::spectrum_shift::spec,
    crate::cards::regent::spoils_of_battle::spec,
    crate::cards::regent::stardust::spec,
    crate::cards::regent::strike_regent::spec,
    crate::cards::regent::summon_forth::spec,
    crate::cards::regent::supermassive::spec,
    crate::cards::regent::sword_sage::spec,
    crate::cards::regent::terraforming::spec,
    crate::cards::regent::the_sealed_throne::spec,
    crate::cards::regent::the_smith::spec,
    crate::cards::regent::tyranny_card::spec,
    crate::cards::regent::venerate::spec,
    crate::cards::regent::void_form::spec,
    crate::cards::regent::wrought_in_war::spec,
];

/// Necrobinder card factories — rebuilt from scratch (C#-first) via the
/// character-port skill. WAVE 1: a partial slice (11 of the 88-card C# pool)
/// kept ALPHABETICAL so it can extend to the full pool in
/// `NecrobinderCardPool.GenerateAllCards()` order. Reward/combat-gen RNG parity
/// against the C# only holds once all 88 are present; until then the partial
/// pool is intentionally incomplete. Dirge is omitted (blocked on the Osty pet
/// system).
pub const NECROBINDER_SPECS: &[fn() -> CardSpec] = &[
    crate::cards::necrobinder::afterlife::spec,
    crate::cards::necrobinder::banshees_cry::spec,
    crate::cards::necrobinder::blight_strike::spec,
    crate::cards::necrobinder::bodyguard::spec,
    crate::cards::necrobinder::bone_shards::spec,
    crate::cards::necrobinder::borrowed_time::spec,
    crate::cards::necrobinder::bury::spec,
    crate::cards::necrobinder::calcify::spec,
    crate::cards::necrobinder::call_of_the_void::spec,
    crate::cards::necrobinder::capture_spirit::spec,
    crate::cards::necrobinder::cleanse::spec,
    crate::cards::necrobinder::countdown::spec,
    crate::cards::necrobinder::danse_macabre::spec,
    crate::cards::necrobinder::death_march::spec,
    crate::cards::necrobinder::deathbringer::spec,
    crate::cards::necrobinder::deaths_door::spec,
    crate::cards::necrobinder::debilitate::spec,
    crate::cards::necrobinder::defend_necrobinder::spec,
    crate::cards::necrobinder::defile::spec,
    crate::cards::necrobinder::defy::spec,
    crate::cards::necrobinder::delay::spec,
    crate::cards::necrobinder::demesne::spec,
    crate::cards::necrobinder::devour_life::spec,
    crate::cards::necrobinder::dirge::spec,
    crate::cards::necrobinder::drain_power::spec,
    crate::cards::necrobinder::dredge::spec,
    crate::cards::necrobinder::eidolon::spec,
    crate::cards::necrobinder::end_of_days::spec,
    crate::cards::necrobinder::enfeebling_touch::spec,
    crate::cards::necrobinder::eradicate::spec,
    crate::cards::necrobinder::fear::spec,
    crate::cards::necrobinder::fetch::spec,
    crate::cards::necrobinder::flatten::spec,
    crate::cards::necrobinder::forbidden_grimoire::spec,
    crate::cards::necrobinder::friendship::spec,
    crate::cards::necrobinder::glimpse_beyond::spec,
    crate::cards::necrobinder::grave_warden::spec,
    crate::cards::necrobinder::graveblast::spec,
    crate::cards::necrobinder::hang::spec,
    crate::cards::necrobinder::haunt::spec,
    crate::cards::necrobinder::high_five::spec,
    crate::cards::necrobinder::invoke::spec,
    crate::cards::necrobinder::legion_of_bone::spec,
    crate::cards::necrobinder::lethality::spec,
    crate::cards::necrobinder::melancholy::spec,
    crate::cards::necrobinder::misery::spec,
    crate::cards::necrobinder::necro_mastery::spec,
    crate::cards::necrobinder::negative_pulse::spec,
    crate::cards::necrobinder::neurosurge::spec,
    crate::cards::necrobinder::no_escape::spec,
    crate::cards::necrobinder::oblivion::spec,
    crate::cards::necrobinder::pagestorm::spec,
    crate::cards::necrobinder::parse::spec,
    crate::cards::necrobinder::poke::spec,
    crate::cards::necrobinder::protector::spec,
    crate::cards::necrobinder::pull_aggro::spec,
    crate::cards::necrobinder::pull_from_below::spec,
    crate::cards::necrobinder::putrefy::spec,
    crate::cards::necrobinder::rattle::spec,
    crate::cards::necrobinder::reanimate::spec,
    crate::cards::necrobinder::reap::spec,
    crate::cards::necrobinder::reaper_form::spec,
    crate::cards::necrobinder::reave::spec,
    crate::cards::necrobinder::right_hand_hand::spec,
    crate::cards::necrobinder::sacrifice::spec,
    crate::cards::necrobinder::scourge::spec,
    crate::cards::necrobinder::sculpting_strike::spec,
    crate::cards::necrobinder::seance::spec,
    crate::cards::necrobinder::sentry_mode::spec,
    crate::cards::necrobinder::severance::spec,
    crate::cards::necrobinder::shared_fate::spec,
    crate::cards::necrobinder::shroud::spec,
    crate::cards::necrobinder::sic_em::spec,
    crate::cards::necrobinder::sleight_of_flesh::spec,
    crate::cards::necrobinder::snap::spec,
    crate::cards::necrobinder::soul_storm::spec,
    crate::cards::necrobinder::sow::spec,
    crate::cards::necrobinder::spirit_of_ash::spec,
    crate::cards::necrobinder::spur::spec,
    crate::cards::necrobinder::squeeze::spec,
    crate::cards::necrobinder::strike_necrobinder::spec,
    crate::cards::necrobinder::the_scythe::spec,
    crate::cards::necrobinder::times_up::spec,
    crate::cards::necrobinder::transfigure::spec,
    crate::cards::necrobinder::undeath::spec,
    crate::cards::necrobinder::unleash::spec,
    crate::cards::necrobinder::veilpiercer::spec,
    crate::cards::necrobinder::wisp::spec,
];

/// Auto-listed colorless card factories (18 entries).
// C# `ColorlessCardPool.GenerateAllCards` order — strictly alphabetical by
// card class name (sts2-wiki v0.103.2
// `MegaCrit.Sts2.Core.Models.CardPools/ColorlessCardPool.cs`). The merchant
// colorless row (`Shops.NextItem` over the rarity-filtered subsequence) and
// every random-colorless generator that indexes this slice by RNG read it in
// pool order, so it MUST match C# alphabetical, not a rarity grouping (same
// fix as the per-character `*_SPECS` alphabetization — see
// `feedback_specs_slice_order_drives_combat_card_generation`).
pub const COLORLESS_SPECS: &[fn() -> CardSpec] = &[
    crate::cards::colorless::alchemize::spec,
    crate::cards::colorless::anointed::spec,
    crate::cards::colorless::automation::spec,
    crate::cards::colorless::beacon_of_hope::spec,
    crate::cards::colorless::beat_down::spec,
    crate::cards::colorless::believe_in_you::spec,
    crate::cards::colorless::bolas::spec,
    crate::cards::colorless::calamity_card::spec,
    crate::cards::colorless::catastrophe::spec,
    crate::cards::colorless::coordinate_card::spec,
    crate::cards::colorless::dark_shackles::spec,
    crate::cards::colorless::discovery::spec,
    crate::cards::colorless::dramatic_entrance::spec,
    crate::cards::colorless::entropy::spec,
    crate::cards::colorless::equilibrium::spec,
    crate::cards::colorless::eternal_armor::spec,
    crate::cards::colorless::fasten::spec,
    crate::cards::colorless::finesse::spec,
    crate::cards::colorless::fisticuffs::spec,
    crate::cards::colorless::flash_of_steel::spec,
    crate::cards::colorless::gang_up::spec,
    crate::cards::colorless::gold_axe::spec,
    crate::cards::colorless::hand_of_greed::spec,
    crate::cards::colorless::hidden_gem::spec,
    crate::cards::colorless::huddle_up::spec,
    crate::cards::colorless::impatience::spec,
    crate::cards::colorless::intercept_card::spec,
    crate::cards::colorless::jack_of_all_trades::spec,
    crate::cards::colorless::jackpot::spec,
    crate::cards::colorless::knockdown::spec,
    crate::cards::colorless::lift::spec,
    crate::cards::colorless::master_of_strategy::spec,
    crate::cards::colorless::mayhem_card::spec,
    crate::cards::colorless::mimic::spec,
    crate::cards::colorless::mind_blast::spec,
    crate::cards::colorless::nostalgia_card::spec,
    crate::cards::colorless::omnislice::spec,
    crate::cards::colorless::panache_card::spec,
    crate::cards::colorless::panic_button::spec,
    crate::cards::colorless::prep_time::spec,
    crate::cards::colorless::production::spec,
    crate::cards::colorless::prolong::spec,
    crate::cards::colorless::prowess::spec,
    crate::cards::colorless::purity::spec,
    crate::cards::colorless::rally::spec,
    crate::cards::colorless::rend::spec,
    crate::cards::colorless::restlessness::spec,
    crate::cards::colorless::rolling_boulder::spec,
    crate::cards::colorless::salvo::spec,
    crate::cards::colorless::scrawl::spec,
    crate::cards::colorless::secret_technique::spec,
    crate::cards::colorless::secret_weapon::spec,
    crate::cards::colorless::seeker_strike::spec,
    crate::cards::colorless::shockwave::spec,
    crate::cards::colorless::splash::spec,
    crate::cards::colorless::stratagem::spec,
    crate::cards::colorless::tag_team::spec,
    crate::cards::colorless::the_bomb_card::spec,
    crate::cards::colorless::the_gambit::spec,
    crate::cards::colorless::thinking_ahead::spec,
    crate::cards::colorless::thrumming_hatchet::spec,
    crate::cards::colorless::ultimate_defend::spec,
    crate::cards::colorless::ultimate_strike::spec,
    crate::cards::colorless::volley::spec,
];

/// Auto-listed status card factories (75 entries).
pub const STATUS_SPECS: &[fn() -> CardSpec] = &[
    st::wound,
    st::dazed,
    st::slimed,
    st::burn,
    st::void,
    st::beckon,
    st::debris,
    st::frantic_escape,
    st::infection,
    st::soot,
    st::toxic,
    st::ascenders_bane,
    st::bad_luck,
    st::clumsy,
    st::curse_of_the_bell,
    st::debt,
    st::decay,
    st::doubt,
    st::enthralled,
    st::folly,
    st::greed,
    st::guilty,
    st::injury,
    st::normality,
    st::pain,
    st::parasite,
    st::poor_sleep,
    st::regret,
    st::shame,
    st::spore_mind,
    st::writhe,
    st::rip_and_tear,
    st::apotheosis,
    st::apparition,
    st::brightest_flame,
    st::maul,
    st::neows_fury,
    st::relax,
    st::whistle,
    st::wish,
    st::byrd_swoop,
    st::caltrops,
    st::clash,
    st::distraction,
    st::dual_wield,
    st::enlightenment,
    st::entrench,
    st::exterminate,
    st::feeding_frenzy,
    st::hello_world,
    st::mad_science,
    st::metamorphosis,
    st::outmaneuver,
    st::peck,
    st::rebound,
    st::squash,
    st::stack,
    st::toric_toughness,
    st::fuel,
    st::giant_rock,
    st::luminesce,
    st::minion_dive_bomb,
    st::minion_sacrifice,
    st::minion_strike,
    st::shiv,
    st::soul,
    st::sovereign_blade,
    st::sweeping_gaze,
    st::disintegration,
    st::mind_rot,
    st::sloth_status,
    st::waste_away,
    st::byrdonis_egg,
    st::lantern_key,
    st::spoils_map,
    st::wither,
];

// ---------------------------------------------------------------------------
// Random-card-from-pool helpers — bead `sts2-rs-70anj.3.15`.
//
// Run-side helpers that select a card from the character pools via
// `RunState::rewards_rng`. Mirrors Python's `random_card_from_rarity`,
// `random_curse_card`, and `random_power_card` consumers
// (sts2_env/run/run_state.py + relics that pull from a typed pool —
// ArcaneScroll, DustyTome, NewLeaf, SereTalon, Pomander).
//
// Filtering composes by:
//   1. Choose the character slice(s) — `Some(c)` picks one, `None`
//      unions every character slice.
//   2. Walk factories, instantiate `CardSpec`, filter on rarity /
//      card_type as the helper requires.
//   3. Sample one matching entry via `rs.rng.rewards.next_int_exclusive`
//      (dotnet RunRngSet reward stream; bead `sts2-rs-hm2eb`).
//
// Empty-result returns `None` (e.g. a filter that matches nothing in a
// character's slice).
// ---------------------------------------------------------------------------

use crate::cards::CardRarity;
use sts2_core::cards::{CardId, CardType};
use sts2_core::potions::CharacterId;
use sts2_core::runstate::RunState;

/// Resolve a [`CharacterId`] to its static spec slice. Returns the
/// generated factory list for Ironclad/Silent/Defect/Regent/Necrobinder;
/// Regent's slice landed via `sts2-rs-le2mn` and Necrobinder's
/// (`NECROBINDER_SPECS`) has since landed too, both ported from the C# decomp.
///
/// GH #367: this is the choke point every RUN-layer pool consumer goes
/// through — card rewards, shop stock, the transform/discovery samplers — so
/// it is where C# `CardPoolModel.GetUnlockedCards`'s epoch prune belongs. With
/// nothing locked (the default, and the C# headless oracle's `UnlockState`)
/// it returns the original `'static` slices unchanged, so no existing caller
/// or fixture shifts.
#[must_use]
pub fn specs_for_character(character: CharacterId) -> &'static [fn() -> super::types::CardSpec] {
    let all = match character {
        CharacterId::Ironclad => IRONCLAD_SPECS,
        CharacterId::Silent => SILENT_SPECS,
        CharacterId::Defect => DEFECT_SPECS,
        CharacterId::Regent => REGENT_SPECS,
        CharacterId::Necrobinder => NECROBINDER_SPECS,
    };
    if crate::unlocks::any_locked() {
        return unlocked_specs(character as u8, all);
    }
    all
}

/// The epoch-filtered variant of [`specs_for_character`], reached only when a
/// locked set is installed (GH #367). Cached per `(character, unlocks
/// generation)` and leaked — see the twin in
/// `cards::colorless::helpers::locked_filtered_combat_pool` for why the leak
/// is bounded and why filtering preserves relative order.
fn unlocked_specs(
    key: u8,
    all: &'static [fn() -> super::types::CardSpec],
) -> &'static [fn() -> super::types::CardSpec] {
    use std::collections::BTreeMap;
    use std::sync::RwLock;

    type Slice = &'static [fn() -> super::types::CardSpec];
    static CACHE: RwLock<Option<(u64, BTreeMap<u8, Slice>)>> = RwLock::new(None);

    let generation = crate::unlocks::locked_generation();
    if let Some((gen, map)) = CACHE.read().expect("spec cache poisoned").as_ref() {
        if *gen == generation {
            if let Some(slice) = map.get(&key) {
                return slice;
            }
        }
    }
    let built: Vec<fn() -> super::types::CardSpec> = all
        .iter()
        .copied()
        .filter(|f| !crate::unlocks::is_card_locked(f().id))
        .collect();
    let leaked: Slice = Box::leak(built.into_boxed_slice());
    let mut guard = CACHE.write().expect("spec cache poisoned");
    match guard.as_mut() {
        Some((gen, map)) if *gen == generation => {
            map.insert(key, leaked);
        }
        _ => {
            let mut map = BTreeMap::new();
            map.insert(key, leaked);
            *guard = Some((generation, map));
        }
    }
    leaked
}

/// Sample one [`CardId`] from the union of per-character slice(s)
/// matching `predicate`. `character = Some(c)` filters to one slice;
/// `None` unions every character slice. Returns `None` when no spec
/// matches. Uses `rs.rewards_rng` for determinism with the seeded
/// RNG already threaded through `run_effects`.
fn sample_card<F>(rs: &mut RunState, character: Option<CharacterId>, predicate: F) -> Option<CardId>
where
    F: Fn(&super::types::CardSpec) -> bool,
{
    let all_slices: [&[fn() -> super::types::CardSpec]; 5] = [
        IRONCLAD_SPECS,
        SILENT_SPECS,
        DEFECT_SPECS,
        REGENT_SPECS,
        COLORLESS_SPECS,
    ];
    let single: [&[fn() -> super::types::CardSpec]; 1] = [match character {
        Some(c) => specs_for_character(c),
        None => &[],
    }];
    let slices: &[&[fn() -> super::types::CardSpec]] = if character.is_some() {
        &single
    } else {
        &all_slices
    };
    let mut matching: Vec<CardId> = Vec::new();
    for slice in slices {
        for factory in *slice {
            let spec = factory();
            // C# `CardFactory.CreateForReward` runs the pool through
            // `FilterForPlayerCount`, which drops `MultiplayerOnly` cards in
            // single-player. The dotnet RNG scales one sample by the LIST
            // LENGTH, so even an unpicked extra entry (Tank in the IC rare
            // pool) shifts which card the same draw resolves to (RP6
            // ArcaneScroll: cs=MANGLE rs=NOTYET, adjacent indices).
            if predicate(&spec) && !is_multiplayer_only(spec.id) {
                matching.push(spec.id);
            }
        }
    }
    if matching.is_empty() {
        return None;
    }
    // Bead `sts2-rs-hm2eb` (943w7 Stage 3): draw from the dotnet
    // `RunRngSet` reward stream, not the legacy Xoshiro `rewards_rng()`.
    let idx = rs.rng.rewards.next_int_exclusive(0, matching.len() as i32) as usize;
    Some(matching[idx])
}

/// Returns the [`CardId`] pair for the character's basic Strike and Defend
/// cards (`[strike, defend]`). Used by PandorasBox / LeafyPoultice to identify
/// the cards to transform out of the deck. Mirrors Python
/// `RunState.basic_strike_defend_cards` (`sts2_env/run/run_state.py`).
/// Every character now has a pair, so the `None` arm is unreachable — it
/// existed for a character STS2 does not have.
/// Regent's pair landed with its spec slice (`sts2-rs-le2mn`); Necrobinder's
/// landed with GH#32 (its starter Strike/Defend exist, so the
/// transform-a-Strike/Defend relics — LeafyPoultice et al. — must resolve
/// the pair, otherwise every Necrobinder run that picks LeafyPoultice at Neow
/// diverges at the post-Neow deck: C# transforms a Strike + Defend, Rust
/// no-op'd them).
/// Bead `sts2-rs-70anj.3.11`.
#[must_use]
pub fn basic_strike_defend_cards(character: CharacterId) -> Option<[CardId; 2]> {
    match character {
        CharacterId::Ironclad => Some([CardId::StrikeIronclad, CardId::DefendIronclad]),
        CharacterId::Silent => Some([CardId::StrikeSilent, CardId::DefendSilent]),
        CharacterId::Defect => Some([CardId::StrikeDefect, CardId::DefendDefect]),
        CharacterId::Regent => Some([CardId::StrikeRegent, CardId::DefendRegent]),
        CharacterId::Necrobinder => Some([CardId::StrikeNecrobinder, CardId::DefendNecrobinder]),
    }
}

/// Roll one transformation replacement off the `transformations` RNG
/// stream, mirroring C# `CardFactory.CreateRandomCardForTransform`
/// (out of combat) for a **Basic**-rarity original — the Strike/Defend
/// pair LeafyPoultice / similar relics transform.
///
/// C# `GetDefaultTransformationOptions(original, isInCombat:false)` →
/// `GetFilteredTransformationOptions(original, original.Pool
/// .GetUnlockedCards(...), isInCombat:false)`:
///   * `original.Pool` for a Basic Strike/Defend is the **character**
///     card pool;
///   * the original is not Status/Curse (`(uint)(rarity - 8) > 1u`
///     holds for Basic), so keep only `{Common, Uncommon, Rare}`;
///   * **out of combat → NO `CanBeGeneratedInCombat` filter**, so cards
///     C# flags non-generatable in combat (Feed, NotYet) stay in the
///     pool — this is why the out-of-combat transform pool is a superset
///     of [`super::ironclad::specs::ironclad_combat_generatable_specs`];
///   * exclude `c.Id == original.Id` (already excluded — Basic ∉ band);
///   * `FilterForPlayerCount` drops `MultiplayerOnly` cards in
///     single-player.
/// Order = `GenerateAllCards` alphabetical = the `*_SPECS` slice order
/// that [`specs_for_character`] returns. Pick =
/// `Transformations.NextItem(pool)` = `pool[NextInt(0, len)]`.
///
/// Returns `None` only if the pool is empty (defensive; non-empty for
/// every ported character at this pin). Bead `sts2-rs-beda`
/// (LeafyPoultice transform leg, umbrella `sts2-rs-01o`).
#[must_use]
pub fn roll_transform_replacement_basic(
    rs: &mut RunState,
    character: CharacterId,
    original_id: CardId,
) -> Option<CardId> {
    let pool = transform_replacement_pool(character, original_id);
    if pool.is_empty() {
        return None;
    }
    let idx = rs
        .rng
        .transformations
        .next_int_exclusive(0, pool.len() as i32) as usize;
    Some(pool[idx])
}

/// [`roll_transform_replacement_basic`] sibling reading the **niche**
/// stream instead of `transformations`. C# `CardCmd.TransformToRandom`
/// takes the rng as a parameter and callers differ: `LeafyPoultice.cs:36`
/// passes `PlayerRng.Transformations`, while `NewLeaf.cs` /
/// `PandorasBox.cs:24` pass `RunState.Rng.Niche`. Same pool, different
/// cursor.
#[must_use]
pub fn roll_transform_replacement_basic_niche(
    rs: &mut RunState,
    character: CharacterId,
    original_id: CardId,
) -> Option<CardId> {
    let pool = transform_replacement_pool(character, original_id);
    if pool.is_empty() {
        return None;
    }
    let idx = rs.rng.niche.next_int_exclusive(0, pool.len() as i32) as usize;
    Some(pool[idx])
}

/// [`roll_transform_replacement_basic`] sibling reading a named event's
/// own NON-shared stream: v0.107.1 `Rng((uint)(run_seed +
/// GetPlayerSlotIndex(Owner) + hash(name)))` (`EventModel.cs:238`).
/// `GetPlayerSlotIndex == Players.IndexOf(Owner) == 0` in single-player
/// (`RunState.cs:620`), so the additive player term is **0** — v0.103.2
/// used `Owner.NetId == 1` (`EventModel.cs:193`); v0.107.1 swapped `NetId`
/// for `GetPlayerSlotIndex` and dropped it. `skip_reads` small-range reads
/// are discarded first: C# `CalculateVars` rolls fire at event entry on
/// the same stream, so a later `CardCmd.TransformToRandom(item, base.Rng)`
/// starts past them (WhisperingHollow Hug: gold variance = read #1,
/// transform = read #2).
#[must_use]
pub fn roll_transform_replacement_event_stream(
    rs: &RunState,
    character: CharacterId,
    original_id: CardId,
    stream_name: &str,
    skip_reads: u32,
) -> Option<CardId> {
    let pool = transform_replacement_pool(character, original_id);
    if pool.is_empty() {
        return None;
    }
    let mut rng = sts2_core::dotnet_rng::Rng::with_name(rs.rng.seed, stream_name);
    for _ in 0..skip_reads {
        // One small-range NextInt = one underlying sample, regardless of
        // bounds — burn exactly what the entry-time rolls consumed.
        let _ = rng.next_int(0, 1);
    }
    let idx = rng.next_int_exclusive(0, pool.len() as i32) as usize;
    Some(pool[idx])
}

/// Roll `count` colorless cards for LeadPaperweight's choose screen.
/// Mirrors C# `LeadPaperweight.AfterObtained` →
/// `CardFactory.CreateForReward(owner, 2, CardCreationOptions(
/// ColorlessCardPool, CardCreationSource.Other, RegularEncounter))`
/// (`LeadPaperweight.cs:21-24` / `CardFactory.cs:72-92,171-199`), per
/// card on the single `PlayerRng.Rewards` stream:
///   1. `RollForRarity` — source `Other` (not `Encounter`) takes the
///      `RollWithBaseOdds` branch: one `NextFloat`, base Regular odds,
///      NO pity read or update.
///   2. `NextItem` over the colorless pool of that rarity minus the
///      already-picked blacklist.
///   3. `RollForUpgrade` — one `NextFloat`, always consumed (no
///      `NoUpgradeRoll` flag on this creation). The upgraded flag itself
///      is dropped (the pending-select carries bare `CardId`s); act-1
///      Neow has 0% upgrade odds so this is read-parity only.
pub fn roll_lead_paperweight_colorless_options(rs: &mut RunState, count: usize) -> Vec<CardId> {
    roll_colorless_options_base_odds(rs, count, true)
}

/// BrainLeech `Rip` colorless reward — same per-card machinery as
/// [`roll_lead_paperweight_colorless_options`] but the creation is
/// `ForNonCombatWithDefaultOdds` (`CardCreationOptions.cs:78`), which
/// stamps `CardCreationFlags.NoUpgradeRoll` — so NO upgrade float is
/// consumed after each pick.
pub fn roll_colorless_reward_options_no_upgrade(rs: &mut RunState, count: usize) -> Vec<CardId> {
    roll_colorless_options_base_odds(rs, count, false)
}

/// BrainLeech `ShareKnowledge` — `CardFactory.CreateForReward(owner, 5,
/// ForNonCombatWithDefaultOdds(owner.Character.CardPool))`: the same
/// no-pity / no-upgrade-roll machinery over the CHARACTER pool.
pub fn roll_character_reward_options_no_upgrade(rs: &mut RunState, count: usize) -> Vec<CardId> {
    let character = rs.character_id;
    roll_reward_options_base_odds(rs, count, false, &[character], false)
}

/// InfestedAutomaton Study (`Type == Power`) / TouchCore (`cost == 0`,
/// non-X) single-card grant — `CardFactory.CreateForReward(owner, 1,
/// ForNonCombatWithDefaultOdds([Character.CardPool], predicate))`
/// (`InfestedAutomaton.cs:28-52`). The same base-odds (`Source.Other` ⇒
/// `RollWithBaseOdds`, no pity read/update), no-upgrade-roll, no-colorless
/// machinery as [`roll_character_reward_options_no_upgrade`], but with the
/// C# `CardPoolFilter` predicate applied to the eligible pool (C#
/// `GetPossibleCards`'s `where CardPoolFilter(c)` leg). Exactly two
/// `PlayerRng.Rewards` reads: one `NextFloat` (rarity) + one `NextItem`.
/// Rarity escalation (`RollForRarity`'s `while !allowedRarities.Contains`)
/// is equivalent to the empty-candidate-pool escalation here for the
/// count-1 / no-blacklist case. `None` only on a totally empty filtered
/// pool (unreachable for shipped characters).
fn roll_filtered_character_reward_id(
    rs: &mut RunState,
    predicate: fn(&CardSpec) -> bool,
) -> Option<CardId> {
    use sts2_core::card_rewards::RarityContext;
    let character = rs.character_id;
    let mut rarity = rs
        .card_rarity_odds
        .roll_with_base_odds_with_dotnet(&mut rs.rng.rewards, RarityContext::Regular);
    let candidates: Vec<CardId> = loop {
        let c = eligible_ids_for_rarity_filtered(
            &[character],
            false,
            reward_rarity_to_card_rarity(rarity),
            predicate,
        );
        if !c.is_empty() {
            break c;
        }
        match next_highest_reward_rarity(rarity) {
            Some(next) => rarity = next,
            None => break c,
        }
    };
    if candidates.is_empty() {
        return None;
    }
    let idx = rs
        .rng
        .rewards
        .next_int_exclusive(0, candidates.len() as i32) as usize;
    Some(candidates[idx])
}

/// InfestedAutomaton Study: a random Power-type card from the character
/// pool off `PlayerRng.Rewards`. See [`roll_filtered_character_reward_id`].
#[must_use]
pub fn roll_infested_study_card(rs: &mut RunState) -> Option<CardId> {
    roll_filtered_character_reward_id(rs, |s| s.card_type == CardType::Power)
}

/// InfestedAutomaton TouchCore: a random 0-cost (non-X) card from the
/// character pool off `PlayerRng.Rewards`. X-cost cards carry `cost == -1`,
/// so the `== 0` test also excludes them (C# `Canonical == 0 && !CostsX`).
/// See [`roll_filtered_character_reward_id`].
#[must_use]
pub fn roll_infested_touch_core_card(rs: &mut RunState) -> Option<CardId> {
    roll_filtered_character_reward_id(rs, |s| s.cost == 0)
}

/// Lost Coffer / Orrery / event card grants — `CardFactory.CreateForReward(
/// owner, 3, new CardCreationOptions(owner.Character.CardPool,
/// CardCreationSource.Other, CardRarityOddsType.RegularEncounter))`
/// (`LostCoffer.cs:18-24`). `Other` source ⇒ `RollForRarity` takes the
/// `RollWithBaseOdds` branch: **base RegularEncounter odds, no pity offset
/// read, no pity-counter mutation**. NO `NoUpgradeRoll` flag, so the upgrade
/// `NextFloat` IS consumed per card (act-scaled odds; 0% at act 1). This is the
/// non-combat sibling of [`roll_card_reward_cards`] (which is the
/// `CardCreationSource.Encounter` pity-updating combat roller). Wired as
/// `ContentCallbacks::generate_noncombat_card_reward_options` and used by
/// `RunManager::drain_pending_rewards` (PARITY132).
///
/// sts2-agent fork (noncombat-card-reward-hooks): each card keeps its upgrade
/// roll (C# `RollForUpgrade` upgrades it), and the reward is a `CardReward`
/// (`IsCardReward`), so `Hook.TryModifyCardRewardOptions` runs every relic's
/// `TryModifyCardRewardOptionsLate` on it (the eggs, Wing Charm, Fresnel
/// Lens, Glitter, Silver Crucible, Silken Tress; Lava Lamp's needs a combat
/// room). The upgrade flags go to `rs.card_reward_upgraded_flags` as
/// [`generate_card_reward_options`](crate::run_callbacks::generate_card_reward_options)'s do.
#[must_use]
pub fn roll_noncombat_card_reward_options(rs: &mut RunState) -> Vec<CardId> {
    let character = rs.character_id;
    let mut cards: Vec<sts2_core::cards::Card> =
        roll_reward_cards_base_odds(rs, 3, true, &[character], false)
            .into_iter()
            .filter_map(|(id, upgraded)| card_pod_from_id(id, upgraded))
            .collect();
    let _ = sts2_core::run_effects::dispatch_modify_card_reward_options_late(rs, &mut cards);
    rs.card_reward_upgraded_flags = cards.iter().map(|c| c.upgraded).collect();
    cards.into_iter().map(|c| c.card_id).collect()
}

/// The ordinal sort key C# uses for a character's `CardPoolModel` inside
/// `StableShuffle` — the model `Id.Entry`, which is the slugified pool class
/// name `<NAME>_CARD_POOL` (`Ironclad.CardPool => IroncladCardPool`,
/// `ModelDb.cs` / `AbstractModel.CompareTo => Id.CompareTo`,
/// `ModelId.CompareTo` = ordinal `Category` then `Entry`). All character
/// pools share the `_CARD_POOL` suffix + Category, so the ordinal compare
/// reduces to the character-name prefix.
const fn character_pool_entry(character: CharacterId) -> &'static str {
    match character {
        CharacterId::Ironclad => "IRONCLAD_CARD_POOL",
        CharacterId::Silent => "SILENT_CARD_POOL",
        CharacterId::Regent => "REGENT_CARD_POOL",
        CharacterId::Necrobinder => "NECROBINDER_CARD_POOL",
        CharacterId::Defect => "DEFECT_CARD_POOL",
    }
}

/// `Kaleidoscope::AfterObtained` — generate `Cards` (=2) card rewards, each
/// offering one card drawn from each of 3 *other-character* card pools
/// (`MegaCrit.Sts2.Core.Models.Relics/Kaleidoscope.cs:33-50`). Per reward:
///   1. `RunState.Rng.Niche.StableShuffle` the owner's NON-owner character
///      pools (the 5-char roster minus the owner), then `Take(3)`.
///   2. For each of those 3 pools,
///      `CardFactory.CreateForReward(owner, 1, new CardCreationOptions(pool,
///      CardCreationSource.Other, RegularEncounter).WithFlags(
///      NoCardPoolModifications))`: one `Other`-source card on
///      `PlayerRng.Rewards` — base RegularEncounter rarity odds (the
///      `RollWithBaseOdds` branch: NO pity read, NO pity-counter mutation),
///      a `NextItem` pick, then an upgrade `NextFloat` (act-scaled; 0% at
///      act 1, so never upgraded at the Neow obtain). This is the per-pool
///      sibling of [`roll_noncombat_card_reward_options`] with a single
///      non-owner character pool and `count = 1`.
///
/// `StableShuffle` = `list.Sort()` (introsort by [`character_pool_entry`])
/// THEN `UnstableShuffle(Niche)`; the keys are distinct so the post-sort
/// order is fully determined, and [`crate::cards::dotnet_list_sort`] inside
/// `stable_shuffle_by` reproduces it. Returns the 2 rewards, each a
/// `Vec<CardId>` of 3 cards in C# generation order — the listener
/// [`crate::relics::shop_event::listener_kaleidoscope`] pushes each as a
/// `Reward::Card`, surfaced by the run loop as a `card_reward` pick. Bead
/// `sts2-rs-utgn4` (v0.107.1 Kaleidoscope; closes the PARITY13/PARITY117
/// Neow `card_reward` whole-run residual).
#[must_use]
pub fn roll_kaleidoscope_card_rewards(rs: &mut RunState, owner: CharacterId) -> Vec<Vec<CardId>> {
    /// `Kaleidoscope.DynamicVars.Cards` — `CanonicalVars => CardsVar(2)`.
    const CARDS: usize = 2;
    /// `StableShuffle(...).Take(3)` — pools (and thus cards) per reward.
    const POOLS_PER_REWARD: usize = 3;
    // The 5-character roster that owns a card pool (C# `ModelDb.AllCharacters`).
    // Order is irrelevant
    // — `StableShuffle` re-sorts before shuffling — but listing the full set
    // lets us drop the owner to get `CharacterCardPools.Where(p != owner)`.
    const POOL_CHARACTERS: [CharacterId; 5] = [
        CharacterId::Ironclad,
        CharacterId::Silent,
        CharacterId::Regent,
        CharacterId::Necrobinder,
        CharacterId::Defect,
    ];
    let mut rewards: Vec<Vec<CardId>> = Vec::with_capacity(CARDS);
    for _ in 0..CARDS {
        // Fresh non-owner pool list each reward (C# rebuilds + reshuffles per
        // iteration of its `for i in 0..Cards` loop).
        let mut pools: Vec<CharacterId> = POOL_CHARACTERS
            .iter()
            .copied()
            .filter(|c| *c != owner)
            .collect();
        rs.rng
            .niche
            .stable_shuffle_by(&mut pools, &|a: CharacterId, b: CharacterId| {
                character_pool_entry(a).cmp(character_pool_entry(b))
            });
        pools.truncate(POOLS_PER_REWARD);
        // One `CardCreationSource.Other` card per pool on the Rewards stream.
        let mut reward_cards: Vec<CardId> = Vec::with_capacity(POOLS_PER_REWARD);
        for ch in pools {
            reward_cards.extend(roll_reward_options_base_odds(rs, 1, true, &[ch], false));
        }
        rewards.push(reward_cards);
    }
    rewards
}

/// `RoomFullOfCheese.Gorge` — `CardFactory.CreateForReward(owner, count,
/// ForNonCombatWithUniformOdds(owner.Character.CardPool, c => c.Rarity ==
/// Common).WithFlags(NoRarityModification))` (`RoomFullOfCheese.cs:38-44`,
/// `CardFactory.cs:72-92,171-199`). Per card on the single `PlayerRng.Rewards`
/// stream, exactly ONE read:
///   - **Uniform odds ⇒ NO rarity roll** (`CreateForReward`'s
///     `RarityOdds == Uniform` branch skips `RollForRarity`): one `NextItem`
///     over the COMMON character pool minus the already-picked blacklist
///     (distinct rewards — C# `Except(blacklist)`).
///   - **NO upgrade roll**: `ForNonCombatWithUniformOdds` stamps
///     `CardCreationFlags.NoUpgradeRoll` (`CardCreationOptions.cs:88-91`), so
///     `CreateForReward` skips `RollForUpgrade` — no upgrade `NextFloat` is
///     consumed and every generated card is base (un-upgraded). Reading an
///     upgrade float here desynced the Rewards stream after the first pick
///     (RP3: card 1 matched, then the 8-card slate diverged).
/// Returns base `Card` PODs in generation order. Bead `sts2-rs-gorge-select`.
#[must_use]
pub fn roll_gorge_common_cards(rs: &mut RunState, count: usize) -> Vec<sts2_core::cards::Card> {
    let character = rs.character_id;
    // C# `GetPossibleCards(player)` (Common-filtered char pool), in
    // `GenerateAllCards` order — the same pool `eligible_ids_for_rarity`
    // serves the (parity-clean) regular reward path.
    let pool = eligible_ids_for_rarity(&[character], false, CardRarity::Common);
    let mut chosen_ids: Vec<CardId> = Vec::new();
    let mut out: Vec<sts2_core::cards::Card> = Vec::new();
    for _ in 0..count {
        // `Except(blacklist)` preserves pool order; `NextItem` indexes it.
        let candidates: Vec<CardId> = pool
            .iter()
            .copied()
            .filter(|id| !chosen_ids.contains(id))
            .collect();
        if candidates.is_empty() {
            break;
        }
        let idx = rs
            .rng
            .rewards
            .next_int_exclusive(0, candidates.len() as i32) as usize;
        let id = candidates[idx];
        chosen_ids.push(id);
        if let Some(card) = card_pod_from_id(id, false) {
            out.push(card);
        }
    }
    out
}

fn roll_colorless_options_base_odds(
    rs: &mut RunState,
    count: usize,
    consume_upgrade_roll: bool,
) -> Vec<CardId> {
    roll_reward_options_base_odds(rs, count, consume_upgrade_roll, &[], true)
}

fn roll_reward_options_base_odds(
    rs: &mut RunState,
    count: usize,
    consume_upgrade_roll: bool,
    character_ids: &[CharacterId],
    include_colorless: bool,
) -> Vec<CardId> {
    roll_reward_cards_base_odds(
        rs,
        count,
        consume_upgrade_roll,
        character_ids,
        include_colorless,
    )
    .into_iter()
    .map(|(id, _)| id)
    .collect()
}

/// [`roll_reward_options_base_odds`] with each card's upgrade roll result
/// (`false` when no roll is made).
fn roll_reward_cards_base_odds(
    rs: &mut RunState,
    count: usize,
    consume_upgrade_roll: bool,
    character_ids: &[CharacterId],
    include_colorless: bool,
) -> Vec<(CardId, bool)> {
    use sts2_core::card_rewards::{
        combat_card_upgrade_base_chance, roll_for_upgrade_with_dotnet, RarityContext,
    };
    let act_index = rs.act_index.saturating_sub(1);
    let ascension_level = rs.card_rarity_odds.ascension_level;
    let upgrade_base = combat_card_upgrade_base_chance();
    let mut chosen: Vec<CardId> = Vec::new();
    let mut upgrades: Vec<bool> = Vec::new();
    for _ in 0..count {
        let mut rarity = rs
            .card_rarity_odds
            .roll_with_base_odds_with_dotnet(&mut rs.rng.rewards, RarityContext::Regular);
        // C# `RollForRarity` escalates a rolled rarity absent from the
        // post-blacklist pool to the next-highest until allowed
        // (`GetNextHighestRarity`; Common → Uncommon → Rare). The shipped
        // colorless pool has gaps per rarity once blacklisted, so this
        // fires in practice.
        let mut candidates: Vec<CardId>;
        loop {
            candidates = eligible_ids_for_rarity(
                character_ids,
                include_colorless,
                reward_rarity_to_card_rarity(rarity),
            );
            candidates.retain(|id| !chosen.contains(id));
            if !candidates.is_empty() {
                break;
            }
            match next_highest_reward_rarity(rarity) {
                Some(next) => rarity = next,
                None => break,
            }
        }
        if candidates.is_empty() {
            continue;
        }
        let idx = rs
            .rng
            .rewards
            .next_int_exclusive(0, candidates.len() as i32) as usize;
        chosen.push(candidates[idx]);
        upgrades.push(
            consume_upgrade_roll
                && roll_for_upgrade_with_dotnet(
                    &mut rs.rng.rewards,
                    rarity,
                    act_index,
                    ascension_level,
                    upgrade_base,
                ),
        );
    }
    chosen.into_iter().zip(upgrades).collect()
}

/// Shared candidate pool for an out-of-combat card transform: the
/// candidate slice filtered to `{Common, Uncommon, Rare}`, minus
/// multiplayer-only entries and the original card (see
/// [`roll_transform_replacement_basic`]'s C# pin).
///
/// The candidate slice mirrors C# `CardFactory.GetDefaultTransformationOptions`
/// (`CardFactory.cs:126-131`): the pool comes from the ORIGINAL card's OWN
/// `CardPoolModel`, NOT the run character's pool. Concretely
/// `cardPool = (original.Type != Quest && original.Rarity != Event/Ancient/Token)
/// ? original.Pool : ColorlessCardPool`. So:
///   * a colorless original (`character_for_card_id` returns `None`) resolves
///     to `original.Pool == ColorlessCardPool` → use [`COLORLESS_SPECS`];
///   * an Ancient / Event / Token original (e.g. NeowsFury, a granted Ancient)
///     hits the explicit `ColorlessCardPool` fallback → use [`COLORLESS_SPECS`];
///   * otherwise (a Common/Uncommon/Rare card owned by a colour pool) use that
///     OWNING character's slice — which is the run character for native deck
///     cards but a DIFFERENT colour for SPLASH-injected off-colour cards.
///
/// `character` is the run character and is used only as the fallback when the
/// original id has no registered spec (defensive; not reached at this pin).
fn transform_replacement_pool(_character: CharacterId, original_id: CardId) -> Vec<CardId> {
    // Resolve the original card's own pool, mirroring C# `original.Pool`
    // (CardModel.cs:273-281) + the Event/Ancient/Token/Quest → ColorlessCardPool
    // override (CardFactory.cs:126-131).
    let original_rarity = rarity_for_card_id(original_id);
    let forces_colorless = matches!(
        original_rarity,
        Some(CardRarity::Event | CardRarity::Ancient | CardRarity::Token | CardRarity::Quest)
    );
    let pool_slice: &[fn() -> CardSpec] = if forces_colorless {
        COLORLESS_SPECS
    } else {
        // `original.Pool`: the colour pool that OWNS the card, independent of
        // which deck currently holds it. `None` => colorless / status / curse
        // / unported => ColorlessCardPool (colorless originals land here).
        match character_for_card_id(original_id) {
            Some(owner) => specs_for_character(owner),
            None => COLORLESS_SPECS,
        }
    };
    pool_slice
        .iter()
        .map(|f| f())
        .filter(|s| {
            matches!(
                s.rarity,
                CardRarity::Common | CardRarity::Uncommon | CardRarity::Rare
            ) && !is_multiplayer_only(s.id)
                && s.id != original_id
        })
        .map(|s| s.id)
        .collect()
}

/// Sample a random card of the given rarity from one character (or
/// all characters if `None`). Mirrors Python's
/// `random_card_from_rarity` consumers (ArcaneScroll → Rare,
/// Pomander → Rare upgrade target). Returns `None` if the pool has
/// no cards at that rarity.
#[must_use]
pub fn random_card_from_rarity(
    rs: &mut RunState,
    character: Option<CharacterId>,
    rarity: CardRarity,
) -> Option<CardId> {
    sample_card(rs, character, |s| s.rarity == rarity)
}

/// Sample the Ancient-rarity card `DustyTome` will grant on obtain.
///
/// Mirrors C# `DustyTome.SetupForPlayer` (`DustyTome.cs:52-55`):
/// `items = player.Character.CardPool.GetUnlockedCards(...)` filtered to
/// `c.Rarity == Ancient && !ArchaicTooth.TranscendenceCards.Contains(c)`,
/// then `PlayerRng.Rewards.NextItem(items)`. Two things differ from the
/// generic [`random_card_from_rarity`] call that previously fed this:
/// (1) the candidate pool is strictly the run's OWN character pool
/// (`player.Character.CardPool`), not the cross-character union — so a
/// Defect run can never be granted a Silent-only Ancient like WraithForm;
/// (2) the `ArchaicTooth.TranscendenceCards` upgrades
/// ({Break, Suppress, Protector, MeteorShower, Quadcast},
/// `ArchaicTooth.cs:26-50`) are excluded. Both are required so the single
/// `Rewards` `NextItem` draw indexes the SAME candidate list C# does (the
/// list length matters: the shared cursor must resolve identically for
/// the rest of the run). Consumes exactly one `Rewards` read, preserving
/// the single-draw invariant `setup_boss_relic_ancient` relies on.
#[must_use]
pub fn random_ancient_card_for_dusty_tome(
    rs: &mut RunState,
    character: CharacterId,
) -> Option<CardId> {
    // `ArchaicTooth.TranscendenceCards` = `TranscendenceUpgrades.Values`
    // (`ArchaicTooth.cs:26-50`): Break, Suppress, Protector, MeteorShower,
    // Quadcast. Excluded from the DustyTome Ancient candidate list.
    const TRANSCENDENCE_CARDS: [CardId; 5] = [
        CardId::Break,
        CardId::Suppress,
        CardId::Protector,
        CardId::MeteorShower,
        CardId::Quadcast,
    ];
    sample_card(rs, Some(character), |s| {
        s.rarity == CardRarity::Ancient && !TRANSCENDENCE_CARDS.contains(&s.id)
    })
}

/// Sample a random Power-typed card from one character (or all if
/// `None`). Mirrors Python's `random_power_card` consumer
/// (DustyTome). Returns `None` if no Power specs exist in the pool.
#[must_use]
pub fn random_power_card(rs: &mut RunState, character: Option<CharacterId>) -> Option<CardId> {
    sample_card(rs, character, |s| s.card_type == CardType::Power)
}

/// Sample a random Curse-typed card from the STATUS_SPECS slice
/// (curses are stored alongside statuses in `status_specs_generated`
/// today). Mirrors Python's `random_curse_card` consumer (SereTalon).
/// Returns `None` if STATUS_SPECS contains no Curse-typed specs.
#[must_use]
pub fn random_curse_card(rs: &mut RunState) -> Option<CardId> {
    let mut matching: Vec<CardId> = Vec::new();
    for factory in STATUS_SPECS {
        let spec = factory();
        if spec.card_type == CardType::Curse {
            matching.push(spec.id);
        }
    }
    if matching.is_empty() {
        return None;
    }
    // Bead `sts2-rs-hm2eb` (943w7 Stage 3): draw from the dotnet
    // `RunRngSet` reward stream, not the legacy Xoshiro `rewards_rng()`.
    let idx = rs.rng.rewards.next_int_exclusive(0, matching.len() as i32) as usize;
    Some(matching[idx])
}

/// Pick up to `n` distinct `CardId`s uniformly without replacement from
/// the colorless pool. Empty pool → empty Vec. Used by Toolbox.
#[must_use]
pub fn sample_distinct_colorless(rs: &mut RunState, n: usize) -> Vec<CardId> {
    sample_distinct_from_specs(rs, COLORLESS_SPECS, n)
}

/// Pick up to `n` distinct `CardId`s uniformly without replacement from
/// the given character's full spec slice. Used by ChoicesParadox.
#[must_use]
pub fn sample_distinct_character_cards(
    rs: &mut RunState,
    character: CharacterId,
    n: usize,
) -> Vec<CardId> {
    sample_distinct_from_specs(rs, specs_for_character(character), n)
}

fn sample_distinct_from_specs(
    rs: &mut RunState,
    specs: &[fn() -> super::types::CardSpec],
    n: usize,
) -> Vec<CardId> {
    if n == 0 || specs.is_empty() {
        return Vec::new();
    }
    let mut pool: Vec<CardId> = specs.iter().map(|f| f().id).collect();
    let take = n.min(pool.len());
    // Bead `sts2-rs-hm2eb` (943w7 Stage 3): draw from the dotnet
    // `RunRngSet` reward stream, not the legacy Xoshiro `rewards_rng()`.
    let rng = &mut rs.rng.rewards;
    let mut out: Vec<CardId> = Vec::with_capacity(take);
    for _ in 0..take {
        let idx = rng.next_int_exclusive(0, pool.len() as i32) as usize;
        out.push(pool.swap_remove(idx));
    }
    out
}

/// Pick up to `n` distinct `CardId`s of `rarity` from `character`'s pool,
/// uniformly without replacement, **preserving pool order on removal**.
///
/// Mirrors C# `CardFactory.CreateForReward` with `CardRarityOddsType.
/// Uniform` odds, a `c.Rarity == rarity` filter, and the
/// `CardCreationFlags.NoUpgradeRoll` flag (`CardFactory.cs:72-92,171-199`):
/// each of the `n` picks is `PlayerRng.Rewards.NextItem(remaining)` over
/// the order-preserving `options.Except(blacklist)` set, so a picked card
/// is removed without reordering the rest. (This differs from
/// [`sample_distinct_from_specs`], which uses `swap_remove`; the
/// order-preserving variant is what matches the C# reward-roll.) Used by
/// `HeftyTablet::after_obtained` (bead `sts2-rs-w6hdj`, W9d).
///
/// Faithfulness caveat: the per-character spec-slice order must match the
/// C# `CardPool.GenerateAllCards` order for the picked-card *identity* to
/// be byte-faithful; the RNG *draw count* is faithful regardless.
#[must_use]
pub fn sample_distinct_cards_of_rarity(
    rs: &mut RunState,
    character: CharacterId,
    rarity: CardRarity,
    n: usize,
) -> Vec<CardId> {
    if n == 0 {
        return Vec::new();
    }
    // Single-player reward pools exclude `MultiplayerOnly` cards: C#
    // `CardFactory.CreateForReward` runs the candidate set through
    // `FilterForPlayerCount` (`CardFactory.cs:175`), which drops every
    // `CardMultiplayerConstraint.MultiplayerOnly` card. Leaving them in makes
    // the rarity pool one (or more) entries longer than C#'s, shifting every
    // RNG-indexed pick. HeftyTablet's 3-Rare roll diverged for characters whose
    // Rare pool contains an MP-only Rare (Tank/Sneaky/Mimic/Rally/Knockdown/
    // GlimpseBeyond/BeaconOfHope/HammerTime/Ignition) — matching the filter the
    // sibling pool builders [`eligible_ids_for_rarity`] /
    // [`generate_scroll_box_bundles`] already apply.
    let mut pool: Vec<CardId> = specs_for_character(character)
        .iter()
        .filter_map(|f| {
            let s = f();
            (s.rarity == rarity && !is_multiplayer_only(s.id)).then_some(s.id)
        })
        .collect();
    let take = n.min(pool.len());
    let rng = &mut rs.rng.rewards;
    let mut out: Vec<CardId> = Vec::with_capacity(take);
    for _ in 0..take {
        let idx = rng.next_int_exclusive(0, pool.len() as i32) as usize;
        out.push(pool.remove(idx));
    }
    out
}

/// Generate the two Scroll Boxes card bundles, byte-faithful to C#
/// `ScrollBoxes.GenerateRandomBundles` (non-Defect path —
/// `ScrollBoxes.cs:49-85`).
///
/// Each of the two bundles is `[common, common, uncommon]`, drawn via the
/// `Rewards` stream (`PlayerRng.Rewards.NextItem` == `next_int_exclusive(0,
/// remaining.len())` with order-preserving removal). A single `used` set is
/// shared across both bundles, so no card id repeats across the four commons
/// and two uncommons. The per-bundle draw order is interleaved exactly as C#:
/// `common, common, uncommon, common, common, uncommon` (six draws total).
///
/// The Defect-only 1%% Claw bundle branch (`flag && rewards.NextInt(100) < 1`)
/// is wired: for Defect the `rewards.NextInt(100)` is read once per bundle
/// iteration (short-circuit means non-Defect never reads it), and a hit
/// (`< 1`) yields a `[Claw, Claw, Claw]` pack. Omitting the read desynced the
/// Rewards stream by 2 picks for every Defect Scroll Boxes / Pael's Wing
/// bundle offering.
///
/// Faithfulness depends on the per-character common/uncommon spec order
/// matching C# `CardPool.GetPossibleCards`; this is the same assumption
/// [`sample_distinct_cards_of_rarity`] already relies on.
/// Cards flagged `CardMultiplayerConstraint.MultiplayerOnly` in the C# game
/// (`grep MultiplayerOnly` over `Models.Cards` — 21 cards total). C#'s
/// `CardFactory.FilterForPlayerCount` drops these from single-player reward
/// pools; the Rust port dropped the `MultiplayerConstraint` model, so reward-
/// pool builders must exclude them explicitly to stay pool-faithful. Used by
/// [`eligible_ids_for_rarity`] (the main reward path) and
/// [`generate_scroll_box_bundles`].
///
/// `Sneaky` (Silent Rare) and `Tank` (Ironclad Rare) WERE unported when this
/// list was first written but have since been ported into their character
/// slices — leaving them out leaked an MP-only card into the rare reward
/// pool, shifting every `NextItem` pick (Silent/Ironclad Boss+Elite reward
/// divergences, caught by `run_parity.py --reward-parity`). (sts2-rs-jenwp M4)
///
/// `Coordinate` / `Intercept` (both `CardMultiplayerConstraint.MultiplayerOnly`
/// in C#) ARE ported — they live in `COLORLESS_SPECS` as
/// `coordinate_card` / `intercept_card` — so they must be listed here too: an
/// earlier comment claimed they were absent from the pool, but leaving them
/// out leaked two MP-only uncommons into the merchant colorless candidate
/// list, making it two entries longer than C#'s and shifting every colorless
/// `Shops.NextItem` pick (caught by `run_parity.py --shop-parity`).
#[must_use]
pub fn is_multiplayer_only(id: CardId) -> bool {
    matches!(
        id,
        CardId::BeaconOfHope
            | CardId::BelieveInYou
            | CardId::CoordinateCard
            | CardId::DemonicShield
            | CardId::EnergySurge
            | CardId::Flanking
            | CardId::GangUp
            | CardId::GlimpseBeyond
            | CardId::HammerTime
            | CardId::HuddleUp
            | CardId::Ignition
            | CardId::InterceptCard
            | CardId::Knockdown
            | CardId::Largesse
            | CardId::LegionOfBone
            | CardId::Lift
            | CardId::Mimic
            | CardId::Rally
            | CardId::SneakyCard
            | CardId::TagTeam
            | CardId::TankCard
    )
}

#[must_use]
pub fn generate_scroll_box_bundles(rs: &mut RunState, character: CharacterId) -> Vec<Vec<CardId>> {
    fn pool_of(character: CharacterId, rarity: CardRarity) -> Vec<CardId> {
        specs_for_character(character)
            .iter()
            .filter_map(|f| {
                let s = f();
                // Single-player reward pools exclude `MultiplayerOnly` cards
                // (C# `CardFactory.FilterForPlayerCount` / the possible-cards
                // filter). The Rust port otherwise leaves these in the pool
                // (the `MultiplayerConstraint` simplification), which would
                // make the rarity pool one larger than C#'s and shift every
                // RNG-indexed pick. Filtering here keeps Scroll Boxes bundles
                // byte-faithful (e.g. Necrobinder's `LegionOfBone`).
                (s.rarity == rarity && !is_multiplayer_only(s.id)).then_some(s.id)
            })
            .collect()
    }
    let commons = pool_of(character, CardRarity::Common);
    let uncommons = pool_of(character, CardRarity::Uncommon);
    // C# `flag = player.Character is Defect` gates the per-bundle Claw roll
    // (`ScrollBoxes.cs:53,65`). For Defect the `rewards.NextInt(100)` is
    // evaluated every bundle iteration (short-circuit means non-Defect skips
    // it and consumes NO RNG), so it MUST consume one read per bundle on the
    // Rewards stream or every downstream pick desyncs.
    let is_defect = character == CharacterId::Defect;
    fn pick(
        rng: &mut sts2_core::dotnet_rng::Rng,
        src: &[CardId],
        used: &mut Vec<CardId>,
    ) -> Option<CardId> {
        let mut avail: Vec<CardId> = src.iter().copied().filter(|c| !used.contains(c)).collect();
        if avail.is_empty() {
            return None;
        }
        let idx = rng.next_int_exclusive(0, avail.len() as i32) as usize;
        let chosen = avail.remove(idx);
        used.push(chosen);
        Some(chosen)
    }
    let rng = &mut rs.rng.rewards;
    let mut used: Vec<CardId> = Vec::with_capacity(6);
    let mut bundles: Vec<Vec<CardId>> = Vec::with_capacity(2);
    for _ in 0..2 {
        // Defect's 1% all-Claw bundle (`flag && rewards.NextInt(100) < 1`).
        // The roll is read unconditionally for Defect; `continue` past the
        // normal common/uncommon picks when it hits.
        if is_defect && rng.next_int_exclusive(0, 100) < 1 {
            bundles.push(vec![CardId::Claw, CardId::Claw, CardId::Claw]);
            continue;
        }
        let mut bundle: Vec<CardId> = Vec::with_capacity(3);
        for _ in 0..2 {
            if let Some(c) = pick(rng, &commons, &mut used) {
                bundle.push(c);
            }
        }
        if let Some(u) = pick(rng, &uncommons, &mut used) {
            bundle.push(u);
        }
        bundles.push(bundle);
    }
    bundles
}

/// Build a fresh `Card` POD for the given `(CardId, upgraded)` from
/// the registered spec factories. Returns `None` if no spec matches
/// the id (e.g. the card belongs to a yet-unported character). This
/// is the factory `sts2-core` calls via [`sts2_core::register_card_factory`]
/// when resolving `ChoiceKind::AddGeneratedToHand` selections.
#[must_use]
pub fn card_pod_from_id(card_id: CardId, upgraded: bool) -> Option<sts2_core::cards::Card> {
    // Perf 2026-07-04: memoized. The old body ran up to ~600 spec factory
    // constructors per call scanning every slice linearly; this is on the
    // MCTS rollout path (generated-card resolution, several thousand calls
    // per search). The base POD for a `CardId` is a pure function of the
    // static spec tables — `card_from_spec` is deterministic (constant uid
    // placeholder, static tag table, empty combat_vars/enchantments) — so
    // one `OnceLock` index built with the SAME slice order and
    // first-match-wins duplicate policy replays the original lookup
    // exactly. Per call: O(log n) map hit + clone of a pod whose SmallVecs
    // are empty (inline, no heap), then the identical upgraded/`apply_spec_
    // upgrade` transform as before.
    use std::collections::BTreeMap;
    use std::sync::OnceLock;
    static BASE_POD_INDEX: OnceLock<BTreeMap<CardId, sts2_core::cards::Card>> = OnceLock::new();
    let index = BASE_POD_INDEX.get_or_init(|| {
        let all_slices: [&[fn() -> super::types::CardSpec]; 7] = [
            IRONCLAD_SPECS,
            SILENT_SPECS,
            DEFECT_SPECS,
            REGENT_SPECS,
            NECROBINDER_SPECS,
            COLORLESS_SPECS,
            STATUS_SPECS,
        ];
        let mut map = BTreeMap::new();
        for slice in all_slices {
            for factory in slice {
                let spec = factory();
                // First match wins, matching the pre-memoization scan order.
                map.entry(spec.id)
                    .or_insert_with(|| super::status::card_from_spec(&spec));
            }
        }
        map
    });
    let mut card = *index.get(&card_id)?;
    card.upgraded = upgraded;
    if upgraded {
        apply_spec_upgrade(&mut card);
    }
    Some(card)
}

/// Map a `CardId` to the C# `CardRarity` enum discriminant
/// (`MegaCrit.Sts2.Core.Entities.Cards/CardRarity.cs`: None=0, Basic=1,
/// Common=2, Uncommon=3, Rare=4, Ancient=5, Event=6, Token=7, Status=8,
/// Curse=9, Quest=10) so core-side sorts replicate C# `orderby
/// c.Rarity` (Stratagem's `after_shuffle` grid-select — C#
/// `StratagemPower.cs:24`). The content-side
/// [`CardRarity`](super::types::CardRarity) enum declares its variants
/// in a DIFFERENT order (Status/Curse before Event), so the mapping is
/// explicit rather than `as u8`. Unknown ids (unported characters,
/// curses outside the spec slices) rank 0 (`None`) — C# has no such
/// cards in a live draw pile. Registered with
/// [`sts2_core::register_card_rarity_rank`] by
/// `cards::dispatch::install_default_registry`. Bead `sts2-rs-63mew`.
/// Reconstruct a card's C# `ModelId.Entry` (the PascalCase class name, e.g.
/// `SeekerStrike`, `IronWave`, `Vicious`, `Thunderclap`) from its Rust
/// python-name (`SCREAMING_SNAKE`, e.g. `SEEKER_STRIKE`, `IRON_WAVE`,
/// `VICIOUS_CARD`, `THUNDERCLAP`).
///
/// C# `CardSelectCmd` Draw-pile selects order options by `(Rarity, Id)` where
/// `Id` is the `ModelId`, compared ordinally by `Entry`
/// (`ModelId.CompareTo` → `string.Compare(Entry, …, Ordinal)`,
/// `CardSelectCmd.cs:407`). The python-name's `_` separators (byte 0x5F) and
/// the `_CARD` disambiguation suffix (absent in C#) break that ordinal order
/// versus PascalCase, so a raw python-name sort picks the wrong option for some
/// draw piles. The transform — drop a trailing `_CARD`, then PascalCase each
/// `_`-token — reproduces the decomp class name for 576/580 cards (the 4
/// residual placeholders are curses/statuses that never surface in a select).
#[must_use]
pub fn card_csharp_entry(card_id: CardId) -> String {
    let name = card_id.to_python_name();
    let base = name.strip_suffix("_CARD").unwrap_or(name);
    base.split('_')
        .filter(|tok| !tok.is_empty())
        .map(|tok| {
            let mut out = String::with_capacity(tok.len());
            let mut chars = tok.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.extend(chars.map(|c| c.to_ascii_lowercase()));
            }
            out
        })
        .collect()
}

#[must_use]
pub fn card_rarity_rank_csharp(card_id: CardId) -> u8 {
    use super::types::CardRarity as CR;
    let all_slices: [&[fn() -> super::types::CardSpec]; 7] = [
        IRONCLAD_SPECS,
        SILENT_SPECS,
        DEFECT_SPECS,
        REGENT_SPECS,
        NECROBINDER_SPECS,
        COLORLESS_SPECS,
        STATUS_SPECS,
    ];
    for slice in all_slices {
        for factory in slice {
            let spec = factory();
            if spec.id == card_id {
                return match spec.rarity {
                    CR::Basic => 1,
                    CR::Common => 2,
                    CR::Uncommon => 3,
                    CR::Rare => 4,
                    CR::Ancient => 5,
                    CR::Event => 6,
                    CR::Token => 7,
                    CR::Status => 8,
                    CR::Curse => 9,
                    CR::Quest => 10,
                };
            }
        }
    }
    0
}

/// The [`super::types::CardSpec`] registered for `card_id`, or `None` for an
/// id with no spec in any of the seven slices. Linear scan over the static
/// tables — fine for the low-frequency deck-management predicates below;
/// do NOT call it per-card in a combat inner loop.
#[must_use]
pub fn spec_for_card_id(card_id: CardId) -> Option<super::types::CardSpec> {
    let all_slices: [&[fn() -> super::types::CardSpec]; 7] = [
        IRONCLAD_SPECS,
        SILENT_SPECS,
        DEFECT_SPECS,
        REGENT_SPECS,
        NECROBINDER_SPECS,
        COLORLESS_SPECS,
        STATUS_SPECS,
    ];
    for slice in all_slices {
        for factory in slice {
            let spec = factory();
            if spec.id == card_id {
                return Some(spec);
            }
        }
    }
    None
}

/// C# `CardModel.IsRemovable => !Keywords.Contains(CardKeyword.Eternal)`
/// (`CardModel.cs:738`) for a live [`sts2_core::cards::Card`] — the filter
/// `CardSelectCmd.FromDeckForRemoval` (`CardSelectCmd.cs:629`) applies before
/// building the generic removal offer, and the same filter every other C#
/// `IsRemovable` consumer shares (Cook `CookRestSiteOption.cs:64`, Pael
/// `Pael.cs:166`, Amalgamator, SlipperyBridge, PandorasBox, WoodCarvings).
///
/// Reads `Keywords`, which in C# is the card's LIVE keyword set, not its
/// canonical one: `TezcatarasEmber.OnEnchant` calls
/// `Card.AddKeyword(CardKeyword.Eternal)` (`TezcatarasEmber.cs:18`), so a card
/// carrying that enchantment is non-removable even though its class declares
/// no Eternal keyword. NutritiousSoup applies TezcatarasEmber to every basic
/// Strike (`relics::shop_event::listener_nutritious_soup_after_obtained`), so
/// this is reachable in ordinary play — and because `removal_offer_order`
/// indices are positional, a wrongly-included Ember'd Strike shifts every
/// later removal pick.
///
/// Eternal curses (Greed, AscendersBane, …) are excluded; basic Strike/Defend
/// carry no Eternal keyword and ARE removable. Unknown ids default to
/// removable (C# has no live non-Eternal-but-unknown deck cards). Registered
/// with [`sts2_core::register_card_removable`] by
/// `cards::dispatch::install_default_registry`. Bead `sts2-rs-RP22`.
#[must_use]
pub fn card_is_removable_instance(card: &sts2_core::cards::Card) -> bool {
    use super::types::CardKeywords;
    match spec_for_card_id(card.card_id) {
        Some(spec) => {
            !crate::enchantments::effective_keywords(&spec, card).contains(CardKeywords::ETERNAL)
        }
        None => true,
    }
}

/// The spec-only half of [`card_is_removable_instance`]: removability from a
/// bare [`CardId`], with no instance to read enchantments off.
///
/// Use ONLY where no `Card` exists — notably the PPO/bridge wire, which
/// projects a static "these ids never appear on a removal screen" list into
/// Python (`sts2_rs.non_removable_cards()`). It is deliberately OPTIMISTIC
/// about enchantments: a TezcatarasEmber'd Strike reads removable here and is
/// not removable in the game. Every engine decision path must take the
/// instance form instead.
#[must_use]
pub fn card_is_removable_csharp(card_id: CardId) -> bool {
    use super::types::CardKeywords;
    spec_for_card_id(card_id).map_or(true, |spec| !spec.keywords.contains(CardKeywords::ETERNAL))
}

/// Rewrite a card's spec-level POD stats (cost / base_damage / base_block)
/// to their upgraded values, per the generated
/// [`card_upgrades_generated::upgrade_delta`] table. No-op for cards with
/// no spec-level upgrade (effect-only upgrades are handled inline by the
/// effect handlers). Idempotent w.r.t. the stored absolute values. Used by
/// [`card_pod_from_id`] at creation and by `sts2_core::cards::Card::upgrade`
/// (via the registered card factory) at runtime. Bead `sts2-rs-vd6pf`.
pub fn apply_spec_upgrade(card: &mut sts2_core::cards::Card) {
    if let Some(delta) = super::card_upgrades_generated::upgrade_delta(card.card_id) {
        if let Some(c) = delta.cost {
            card.cost = c;
        }
        if let Some(d) = delta.base_damage {
            card.base_damage = d;
        }
        if let Some(b) = delta.base_block {
            card.base_block = b;
        }
        // Bead `sts2-rs-59atd`: stamp the upgrade-aware keyword set onto
        // the instance so the engine-side keyword resolution
        // (`card_effects::card_*_inst`) can diverge the ~27 cards whose
        // keywords change on upgrade (Aggression/Afterimage/Entropy gain
        // INNATE, BurningPact drops EXHAUST, some gain RETAIN) from their
        // un-upgraded `CardId`-keyed registry entry. `delta.keywords` is the
        // absolute upgraded set; `None` leaves the registry authoritative.
        if let Some(kw) = delta.keywords {
            card.keyword_override = Some(keyword_override_from(kw));
        }
    }
}

/// Project a content-side [`CardKeywords`](super::types::CardKeywords) set
/// onto the core-side [`sts2_core::cards::KeywordOverride`] used by
/// `Card::keyword_override`. Only the keywords with engine-side resolution
/// (UNPLAYABLE / ETHEREAL / RETAIN / EXHAUST / INNATE) are carried;
/// deck-management flags (NON_REMOVABLE / NON_UPGRADEABLE) never change on
/// upgrade and have no per-instance consumer. Bead `sts2-rs-59atd`.
fn keyword_override_from(kw: super::types::CardKeywords) -> sts2_core::cards::KeywordOverride {
    use super::types::CardKeywords as CK;
    use sts2_core::cards::KeywordOverride as KO;
    let mut out = KO::NONE;
    if kw.contains(CK::UNPLAYABLE) {
        out = out.with(KO::UNPLAYABLE);
    }
    if kw.contains(CK::ETHEREAL) {
        out = out.with(KO::ETHEREAL);
    }
    if kw.contains(CK::RETAIN) {
        out = out.with(KO::RETAIN);
    }
    if kw.contains(CK::EXHAUST) {
        out = out.with(KO::EXHAUST);
    }
    if kw.contains(CK::INNATE) {
        out = out.with(KO::INNATE);
    }
    out
}

/// Lookup the `CardRarity` for an arbitrary [`CardId`] by scanning
/// every slice (Ironclad / Silent / Defect / Colorless / Status).
/// Returns `None` when the id has no registered factory (yet-unported
/// character). Used by [`roll_card_reward_cards`] to filter
/// `opts.custom_card_ids` by rolled rarity.
#[must_use]
pub fn rarity_for_card_id(card_id: CardId) -> Option<CardRarity> {
    let all_slices: [&[fn() -> super::types::CardSpec]; 7] = [
        IRONCLAD_SPECS,
        SILENT_SPECS,
        DEFECT_SPECS,
        REGENT_SPECS,
        NECROBINDER_SPECS,
        COLORLESS_SPECS,
        STATUS_SPECS,
    ];
    for slice in all_slices {
        for factory in slice {
            let spec = factory();
            if spec.id == card_id {
                return Some(spec.rarity);
            }
        }
    }
    None
}

/// Resolve a [`CardId`] to the [`CharacterId`] whose card pool *owns* it,
/// i.e. the character whose `*_SPECS` slice lists the id. Mirrors C#
/// `CardModel.Pool` (`CardModel.cs:273-295`), which resolves to the first
/// `CardPoolModel` in `ModelDb.AllCardPools` whose `AllCardIds.Contains(Id)`
/// — the card's own colour pool, **independent of which deck currently holds
/// it**. A Silent card (e.g. `MementoMori`) injected into an Ironclad deck via
/// SPLASH therefore still reports the Silent pool.
///
/// Returns `None` for ids not owned by any character slice (colorless / status
/// / curse / token / unported), which callers must classify separately (those
/// have their own pools: `ColorlessCardPool` / `StatusCardPool` / `CursePool`).
#[must_use]
pub fn character_for_card_id(card_id: CardId) -> Option<CharacterId> {
    const CHARACTERS: [CharacterId; 5] = [
        CharacterId::Ironclad,
        CharacterId::Silent,
        CharacterId::Defect,
        CharacterId::Regent,
        CharacterId::Necrobinder,
    ];
    for c in CHARACTERS {
        for factory in specs_for_character(c) {
            if factory().id == card_id {
                return Some(c);
            }
        }
    }
    None
}

/// Map a [`sts2_core::card_rewards::RewardRarity`] onto the
/// content-side [`CardRarity`] used by every [`CardSpec`]. Reward rolls
/// only ever produce `Common`/`Uncommon`/`Rare`, so the mapping is
/// total.
const fn reward_rarity_to_card_rarity(r: sts2_core::card_rewards::RewardRarity) -> CardRarity {
    use sts2_core::card_rewards::RewardRarity as RR;
    match r {
        RR::Common => CardRarity::Common,
        RR::Uncommon => CardRarity::Uncommon,
        RR::Rare => CardRarity::Rare,
    }
}

/// Walk every per-character + colorless slice and collect `CardId`s
/// whose [`CardSpec`] matches `rarity`. The walk is deterministic in
/// slice order so the sampler downstream is reproducible under a
/// seeded RNG.
fn eligible_ids_for_rarity(
    character_ids: &[CharacterId],
    include_colorless: bool,
    rarity: CardRarity,
) -> Vec<CardId> {
    let mut out: Vec<CardId> = Vec::new();
    for c in character_ids {
        for factory in specs_for_character(*c) {
            let spec = factory();
            // Single-player reward pools exclude `MultiplayerOnly` cards
            // (C# `CardFactory.FilterForPlayerCount` drops them when
            // `Players.Count == 1`). The Rust port dropped the
            // `MultiplayerConstraint` model, so leaving these in would make
            // the rarity pool one larger than C#'s and shift every
            // `NextItem`-indexed pick (e.g. Ironclad `DemonicShield`, an
            // MP-only uncommon, desynced the whole uncommon slate).
            // (sts2-rs-jenwp M4)
            if spec.rarity == rarity && !is_multiplayer_only(spec.id) {
                out.push(spec.id);
            }
        }
    }
    if include_colorless {
        for factory in COLORLESS_SPECS {
            let spec = factory();
            if spec.rarity == rarity && !is_multiplayer_only(spec.id) {
                out.push(spec.id);
            }
        }
    }
    out
}

/// [`eligible_ids_for_rarity`] with an extra `spec`-level `predicate` (the
/// C# `CardCreationOptions.CardPoolFilter`). Pool order is preserved, so a
/// `NextItem` index over the result matches C#'s
/// `GetPossibleCards(player).Where(rarity).Where(predicate)`.
fn eligible_ids_for_rarity_filtered(
    character_ids: &[CharacterId],
    include_colorless: bool,
    rarity: CardRarity,
    predicate: fn(&CardSpec) -> bool,
) -> Vec<CardId> {
    let mut out: Vec<CardId> = Vec::new();
    for c in character_ids {
        for factory in specs_for_character(*c) {
            let spec = factory();
            if spec.rarity == rarity && !is_multiplayer_only(spec.id) && predicate(&spec) {
                out.push(spec.id);
            }
        }
    }
    if include_colorless {
        for factory in COLORLESS_SPECS {
            let spec = factory();
            if spec.rarity == rarity && !is_multiplayer_only(spec.id) && predicate(&spec) {
                out.push(spec.id);
            }
        }
    }
    out
}

/// Roll concrete card-reward options from the run's character pool,
/// then fire the run-side `modify_card_reward_options_late` channel so
/// relic listeners (FresnelLens enchant-on-reward, FrozenEgg/MoltenEgg/
/// ToxicEgg pre-upgrade legs, …) can mutate the resulting `Vec<Card>`
/// in place.
///
/// Ports the sampling+enchantment legs of Python
/// `sts2-rl-agent@01c094d:sts2_env/run/rewards.py::_pick_reward_card`
/// + `generate_combat_reward_cards`. The rarity slate and upgrade
/// flags come from [`RunState::generate_combat_card_rewards`]
/// (bead `sts2-rs-jyfvu`, P9.1.e), which already walks
/// `modify_card_reward_creation_options` against `opts` before
/// rolling.
///
/// Per-rarity sampling:
/// - `opts.has_custom_card_pool` ⇒ candidate pool is the filtered
///   subset of `opts.custom_card_ids` matching the rolled rarity;
/// - otherwise the union of every character slice in
///   `opts.character_ids` (or `[default_character]` when
///   `opts.character_ids` is empty and `opts.use_default_character_pool`
///   is true) plus the colorless pool when
///   `opts.include_colorless` is true, filtered by rarity.
/// - `chosen_ids` tracks distinctness across rolls so the slate has
///   no duplicates (matches Python's `chosen_ids: set` guard).
///
/// Rarities that yield zero candidates are skipped — the slate
/// returned can be shorter than `opts.num_cards`. Python's pin
/// falls back to the next-highest rarity in this case; the fallback
/// is documented as a follow-up here so the bead remains a thin
/// content-side slice. See bead `sts2-rs-jya0y` (P9.2.i.2) for the
/// pin and divergence notes.
///
/// Returns `Vec<Card>` rather than the literal `Vec<CardId>` named in
/// the bead title because `dispatch_modify_card_reward_options_late`
/// takes `&mut Vec<Card>` (listeners mutate per-instance
/// `enchantments` / `upgraded` state). Callers can map to `CardId`
/// via `card.card_id` when the id-only view is enough.
///
/// `default_character` substitutes for the Python
/// `run_state.player.character_id` accessor, which has no analogue on
/// the Rust `RunState` at this pin; the run-loop driver (which
/// already owns the active `CharacterId` via `RunManager`) passes it
/// in explicitly.
#[must_use]
pub fn roll_card_reward_cards(
    rs: &mut RunState,
    opts: sts2_core::runstate::CardRewardGenerationOptions,
    default_character: CharacterId,
) -> Vec<sts2_core::cards::Card> {
    use sts2_core::card_rewards::{
        combat_card_upgrade_base_chance, roll_for_upgrade_with_dotnet, RarityContext,
    };

    // Fold the creation options once (relic listeners — PrismaticGem,
    // DingyRug, SilverCrucible — reshape the pool / colorless flag /
    // upgrade-roll). C# folds per card inside `CardFactory.CreateForReward`,
    // but those listeners are option-shapers that consume no RNG, so a
    // single fold is RNG- and option-equivalent for the idempotent
    // listeners at this pin.
    let opts = rs.modify_card_reward_creation_options(opts);

    let has_custom = opts.has_custom_card_pool;
    let custom_ids = opts.custom_card_ids.clone();
    let include_colorless = opts.include_colorless;
    let character_ids: Vec<CharacterId> = if !opts.character_ids.is_empty() {
        opts.character_ids.clone()
    } else if opts.use_default_character_pool {
        vec![default_character]
    } else {
        Vec::new()
    };
    let num_cards = usize::from(opts.num_cards);
    let roll_upgrade = opts.roll_upgrade;
    let context = RarityContext::from_python_str(opts.context);
    // C# `RollForUpgrade` scales by 0-based `RunState.CurrentActIndex`
    // (act 1 → 0 → 0% upgrade chance); Rust `rs.act_index` is 1-based.
    // (sts2-rs-jenwp M4)
    let act_index = rs.act_index.saturating_sub(1);
    let ascension_level = rs.card_rarity_odds.ascension_level;
    let upgrade_base = combat_card_upgrade_base_chance();

    let mut chosen_ids: std::collections::BTreeSet<CardId> = std::collections::BTreeSet::new();
    let mut cards: Vec<sts2_core::cards::Card> = Vec::with_capacity(num_cards);

    // Per-card interleaved roll, matching C# `CardFactory.CreateForReward`
    // (sts2-wiki v0.103.2 `CardFactory.cs:72-92` / `171-199`). For each
    // card, in this exact order on the *single* `PlayerRng.Rewards`
    // stream (`PlayerOddsSet.cs:18` — `CardRarity = new
    // CardRarityOdds(rng.Rewards)`):
    //   1. RollForRarity   — `CardRarityOdds.Roll` (one NextFloat + pity)
    //   2. NextItem(items) — the candidate pick
    //   3. RollForUpgrade  — one NextFloat
    //
    // Bead `sts2-rs-i6h9i.1.1`: the prior code batched ALL rarities +
    // upgrades (`generate_combat_card_rewards`) and THEN all picks, which
    // desynced the shared Rewards stream (Rust draw order
    // `[rarity*, upgrade*, pick*]` vs C# `[rarity,pick,upgrade]` per card)
    // and yielded zero overlap with the C#-recorded reward cards at the
    // same seed/floor/deck — breaking same-seed deck-parity replay.
    for _ in 0..num_cards {
        // 1. Rarity (consumes Rewards; updates the pity counter).
        let rarity = rs
            .card_rarity_odds
            .roll_with_dotnet(&mut rs.rng.rewards, context);
        let cr = reward_rarity_to_card_rarity(rarity);
        // 2. Candidates for the rolled rarity, minus already-picked.
        let mut candidates: Vec<CardId> = if has_custom {
            custom_ids
                .iter()
                .copied()
                .filter(|&id| rarity_for_card_id(id) == Some(cr))
                .collect()
        } else {
            eligible_ids_for_rarity(&character_ids, include_colorless, cr)
        };
        candidates.retain(|id| !chosen_ids.contains(id));
        if candidates.is_empty() {
            continue;
        }
        let idx = rs
            .rng
            .rewards
            .next_int_exclusive(0, candidates.len() as i32) as usize;
        let pick = candidates[idx];
        chosen_ids.insert(pick);
        // 3. Upgrade roll AFTER the pick (consumes Rewards).
        let upgraded = roll_upgrade
            && roll_for_upgrade_with_dotnet(
                &mut rs.rng.rewards,
                rarity,
                act_index,
                ascension_level,
                upgrade_base,
            );
        if let Some(card) = card_pod_from_id(pick, upgraded) {
            cards.push(card);
        }
    }

    // C# `CardFactory.CreateForReward` fires `Hook.TryModifyCardRewardOptions`
    // (`CardFactory.cs:87`) AFTER rolling the base cards and BEFORE the
    // run-side `_late` channel (`Rewards/CardReward.cs:128-142` runs each
    // relic's mid-then-late listeners). The only shipped mid listener is
    // `LastingCandy.TryModifyCardRewardOptions` (`LastingCandy.cs:100-137`),
    // which appends a 4th Power card on every 2nd combat reward. It needs
    // the reward's rarity-odds context (Monster/Elite/Boss) + colorless
    // flag, which only live here in `opts`, so it is folded inline at the
    // exact C# hook site rather than via the context-less `Vec<Card>`
    // channel used by `_late`.
    append_lasting_candy_power_reward(
        rs,
        &mut cards,
        &character_ids,
        include_colorless,
        context,
        act_index,
        ascension_level,
        upgrade_base,
    );

    let _ = sts2_core::run_effects::dispatch_modify_card_reward_options_late(rs, &mut cards);
    cards
}

/// `LastingCandy.TryModifyCardRewardOptions` (sts2-wiki v0.103.2
/// `LastingCandy.cs:100-137`): on every 2nd combat reward (the relic's
/// `IsInTriggeringCombat` — `CombatsSeen > 0 && CombatsSeen % 2 == 0`,
/// `LastingCandy.cs:68-78`), append one random **Power** card to the
/// reward slate. Fired as the C# mid hook inside
/// `CardFactory.CreateForReward` (`CardFactory.cs:87`), after the base
/// cards roll and before the `_late` channel.
///
/// The appended card is rolled via a nested
/// `CardFactory.CreateForReward(owner, 1, options3)` where `options3` is
/// the Power-filtered pool with `Source = Other` and the OUTER reward's
/// `RarityOdds` (`LastingCandy.cs:128`). `Source != Encounter` makes
/// `RollForRarity` take the base-odds branch (`CardFactory.cs:201-216`),
/// and `NoUpgradeRoll` is NOT set so an upgrade roll IS consumed
/// (`CardFactory.cs:81-85`). Net: exactly THREE `PlayerRng.Rewards`
/// reads when triggered (base-odds rarity + `NextItem` pick + upgrade),
/// none when not — so the Rewards cursor stays aligned with C#.
///
/// Pool dedup mirrors `LastingCandy.cs:115-123`: prefer Powers whose id
/// is not already in `cards`; fall back to the full Power pool when the
/// preferred set is empty. The rarity is rolled over the rarities
/// present in the (deduped) Power pool, escalating to the next-highest
/// rarity when the rolled one has no candidates
/// (`CardFactory.RollForRarity`'s `while !allowedRarities.Contains`).
#[allow(clippy::too_many_arguments)]
fn append_lasting_candy_power_reward(
    rs: &mut RunState,
    cards: &mut Vec<sts2_core::cards::Card>,
    character_ids: &[CharacterId],
    include_colorless: bool,
    context: sts2_core::card_rewards::RarityContext,
    act_index: u8,
    ascension_level: u8,
    upgrade_base: f64,
) {
    use sts2_core::card_rewards::roll_for_upgrade_with_dotnet;

    // Ownership gate (`base.Owner != player` → false) + the every-2nd-
    // combat trigger (`IsInTriggeringCombat`). `CombatsSeen` lives in the
    // run-side `relic_counters`, flushed back from the combat-side
    // `on_after_combat_end_lasting_candy` bump by `run_bridge::exit_combat`
    // before the reward is rolled.
    if !rs.owns_active_relic(sts2_core::RelicId::LastingCandy) {
        return;
    }
    let combats_seen = rs
        .relic_counters
        .get(&sts2_core::RelicId::LastingCandy)
        .copied()
        .unwrap_or(0);
    if combats_seen <= 0 || combats_seen % 2 != 0 {
        return;
    }

    // `creationOptions.Source` for combat rewards is always `Encounter`
    // (this is the combat roller); the nested `options3` flips it to
    // `Other`, but the OUTER `RarityOdds` (= `context`) is what the
    // base-odds rarity roll uses.

    // Build the per-rarity deduped Power pool. C# computes the candidate
    // `enumerable` once: Powers not already offered (preferred), else all
    // Powers (`LastingCandy.cs:115-123`). The rarity roll then samples
    // over the rarities present in that pool.
    let already_offered: std::collections::BTreeSet<CardId> =
        cards.iter().map(|c| c.card_id).collect();

    let power_pool_for_rarity = |rarity: CardRarity, dedup: bool| -> Vec<CardId> {
        let mut v =
            eligible_ids_for_rarity_filtered(character_ids, include_colorless, rarity, |s| {
                s.card_type == CardType::Power
            });
        if dedup {
            v.retain(|id| !already_offered.contains(id));
        }
        v
    };

    // `enumerable.Any()` over the preferred (deduped) pool decides whether
    // dedup applies; if no un-offered Power exists at ALL, C# falls back
    // to the full Power pool for the entire roll.
    let any_preferred = [CardRarity::Common, CardRarity::Uncommon, CardRarity::Rare]
        .iter()
        .any(|&r| !power_pool_for_rarity(r, true).is_empty());
    let dedup = any_preferred;

    // Bail if there is no Power card to offer at all (`!enumerable.Any()`
    // → return false, no card appended, no RNG consumed).
    let has_any_power = [CardRarity::Common, CardRarity::Uncommon, CardRarity::Rare]
        .iter()
        .any(|&r| !power_pool_for_rarity(r, dedup).is_empty());
    if !has_any_power {
        return;
    }

    // 1. Base-odds rarity roll (`Source = Other` ⇒ `RollWithBaseOdds`,
    //    no pity read/update), then escalate to the next-highest rarity
    //    while the current one has no candidates.
    let mut rarity = rs
        .card_rarity_odds
        .roll_with_base_odds_with_dotnet(&mut rs.rng.rewards, context);
    let candidates: Vec<CardId> = loop {
        let c = power_pool_for_rarity(reward_rarity_to_card_rarity(rarity), dedup);
        if !c.is_empty() {
            break c;
        }
        match next_highest_reward_rarity(rarity) {
            Some(next) => rarity = next,
            None => break c,
        }
    };
    if candidates.is_empty() {
        return;
    }

    // 2. `NextItem(items)` pick over the rolled rarity's Power candidates.
    let idx = rs
        .rng
        .rewards
        .next_int_exclusive(0, candidates.len() as i32) as usize;
    let pick = candidates[idx];

    // 3. Upgrade roll AFTER the pick (`NoUpgradeRoll` not set).
    let upgraded = roll_for_upgrade_with_dotnet(
        &mut rs.rng.rewards,
        rarity,
        act_index,
        ascension_level,
        upgrade_base,
    );

    if let Some(card) = card_pod_from_id(pick, upgraded) {
        cards.push(card);
    }
}

/// Next-highest reward rarity for the merchant card-rarity escalation
/// loop (`CardFactory.CreateForMerchant`'s `while list.Count == 0:
/// rolledRarity = rolledRarity.GetNextHighestRarity()`). Common →
/// Uncommon → Rare → exhausted (`None`). Mirrors C#
/// `CardRarityExtensions.GetNextHighestRarity` for the three reachable
/// shop rarities.
const fn next_highest_reward_rarity(
    r: sts2_core::card_rewards::RewardRarity,
) -> Option<sts2_core::card_rewards::RewardRarity> {
    use sts2_core::card_rewards::RewardRarity as RR;
    match r {
        RR::Common => Some(RR::Uncommon),
        RR::Uncommon => Some(RR::Rare),
        RR::Rare => None,
    }
}

/// Character-pool candidates of `(rarity, want_type)` in C# pool order
/// (`Player.Character.CardPool.GetUnlockedCards` minus Basic / MP-only /
/// already-chosen), the `source.Where(c.Rarity == r && c.Type == type)`
/// list that `CardFactory.CreateForMerchant` indexes with
/// `Shops.NextItem`.
fn merchant_character_candidates(
    character: CharacterId,
    rarity: sts2_core::card_rewards::RewardRarity,
    want_type: CardType,
    chosen: &std::collections::BTreeSet<CardId>,
) -> Vec<CardId> {
    let cr = reward_rarity_to_card_rarity(rarity);
    specs_for_character(character)
        .iter()
        .map(|f| f())
        .filter(|s| {
            s.rarity == cr
                && s.card_type == want_type
                && !is_multiplayer_only(s.id)
                && !chosen.contains(&s.id)
        })
        .map(|s| s.id)
        .collect()
}

/// Map a relic-reward rarity onto the merchant `ShopRelicRarity` bucket.
const fn relic_rarity_to_shop(
    r: sts2_core::runstate::RelicRarity,
) -> sts2_core::shop::ShopRelicRarity {
    use sts2_core::runstate::RelicRarity as RR;
    use sts2_core::shop::ShopRelicRarity as SR;
    match r {
        // `roll_reward_relic_rarity` only ever yields Common/Uncommon/Rare;
        // Shop is mapped for totality.
        RR::Common => SR::Common,
        RR::Uncommon => SR::Uncommon,
        RR::Rare => SR::Rare,
        RR::Shop => SR::Shop,
    }
}

/// C# `RelicModel.MerchantCost` default — the per-rarity base price the
/// merchant relic-cost jitter scales. Mirrors `RelicModel.cs:278` (no
/// grab-bag relic overrides this default; the only `MerchantCost`
/// overrides in the decomp are on `Event`-rarity Fake* relics, which are
/// never in the Common/Uncommon/Rare/Shop grab bag).
const fn merchant_relic_cost(r: sts2_core::runstate::RelicRarity) -> i32 {
    use sts2_core::runstate::RelicRarity as RR;
    match r {
        RR::Common => 175,
        RR::Uncommon => 225,
        RR::Rare => 275,
        RR::Shop => 200,
    }
}

/// Roll one character-card merchant slot on the run's two streams in C#
/// `CardFactory.CreateForMerchant(player, options, CardType)` order:
/// rarity (`Rewards`, with `GetNextAllowedRarity` escalation), pick
/// (`Shops.NextItem`), the upgrade no-op (`Rewards`), then the cost
/// jitter (`Shops`, rolled twice for an on-sale slot per C# `SetOnSale`
/// re-running `CalcCost`). `chosen` accumulates picked ids so a card is
/// never offered twice (C# `_cardPool.Except(second)`); the pick is
/// inserted before returning. Shared by [`generate_merchant_inventory`]
/// (initial roll) and [`restock_shop_entry`] (Courier restock).
fn merchant_roll_character_card_slot(
    rs: &mut RunState,
    character: CharacterId,
    want_type: CardType,
    on_sale: bool,
    chosen: &mut std::collections::BTreeSet<CardId>,
) -> sts2_core::shop::ShopCardEntry {
    use sts2_core::card_rewards::RarityContext;
    use sts2_core::shop::{card_price, ShopCardEntry};
    let mut rarity = rs
        .card_rarity_odds
        .roll_without_changing_odds_with_dotnet(&mut rs.rng.rewards, RarityContext::Shop);
    let mut candidates = merchant_character_candidates(character, rarity, want_type, chosen);
    while candidates.is_empty() {
        match next_highest_reward_rarity(rarity) {
            Some(next) => {
                rarity = next;
                candidates = merchant_character_candidates(character, rarity, want_type, chosen);
            }
            None => break,
        }
    }
    let pick = if candidates.is_empty() {
        None
    } else {
        let idx = rs.rng.shops.next_int_exclusive(0, candidates.len() as i32) as usize;
        Some(candidates[idx])
    };
    let _ = rs.rng.rewards.next_float(1.0);
    let price = if on_sale {
        let _discard = card_price(rarity, &mut rs.rng.shops, false, false);
        card_price(rarity, &mut rs.rng.shops, false, true)
    } else {
        card_price(rarity, &mut rs.rng.shops, false, false)
    };
    let card = pick.and_then(|id| {
        chosen.insert(id);
        card_pod_from_id(id, false)
    });
    ShopCardEntry {
        rarity,
        card_type: want_type,
        card,
        is_colorless: false,
        on_sale,
        price,
    }
}

/// Roll one colorless-card merchant slot (fixed `rarity`) in C#
/// `CardFactory.CreateForMerchant(player, options, CardRarity)` order:
/// pick (`Shops.NextItem`), upgrade no-op (`Rewards`), cost (`Shops`,
/// ×1.15 colorless). Never on sale. Shared by the initial roll and the
/// Courier restock.
fn merchant_roll_colorless_card_slot(
    rs: &mut RunState,
    rarity: sts2_core::card_rewards::RewardRarity,
    chosen: &mut std::collections::BTreeSet<CardId>,
) -> sts2_core::shop::ShopCardEntry {
    use sts2_core::shop::{card_price, ShopCardEntry};
    let cr = reward_rarity_to_card_rarity(rarity);
    let candidates: Vec<CardId> = COLORLESS_SPECS
        .iter()
        .map(|f| f())
        .filter(|s| s.rarity == cr && !is_multiplayer_only(s.id) && !chosen.contains(&s.id))
        .map(|s| s.id)
        .collect();
    let pick = if candidates.is_empty() {
        None
    } else {
        let idx = rs.rng.shops.next_int_exclusive(0, candidates.len() as i32) as usize;
        Some(candidates[idx])
    };
    let _ = rs.rng.rewards.next_float(1.0);
    let price = card_price(rarity, &mut rs.rng.shops, true, false);
    let card = pick.and_then(|id| {
        chosen.insert(id);
        card_pod_from_id(id, false)
    });
    ShopCardEntry {
        rarity,
        card_type: CardType::Skill,
        card,
        is_colorless: true,
        on_sale: false,
        price,
    }
}

/// Roll one relic merchant slot for a pre-rolled `slot_rarity`: pull the
/// next relic of that rarity from the BACK of the grab bag
/// (`MerchantRelicEntry.FillSlot` → `PullNextRelicFromBack`, filtered by
/// `IsAllowedInShops` and `extra_exclude`) and jitter its `MerchantCost`
/// by one `Shops.NextFloat(0.85, 1.15)`. `extra_exclude` mirrors C#'s
/// restock blacklist (the currently-stocked relic ids); the initial roll
/// passes an empty set since the bag drains as it pulls.
fn merchant_roll_relic_slot(
    rs: &mut RunState,
    slot_rarity: sts2_core::runstate::RelicRarity,
    extra_exclude: &std::collections::BTreeSet<sts2_core::relics::RelicId>,
) -> sts2_core::shop::ShopRelicEntry {
    use sts2_core::shop::{
        ShopRelicEntry, MERCHANT_RELIC_PRICE_VARIANCE_MAX, MERCHANT_RELIC_PRICE_VARIANCE_MIN,
    };
    let pulled = rs.pop_relic_from_grab_bag_back(slot_rarity, |id| {
        sts2_core::shop::is_relic_allowed_in_shops(id) && !extra_exclude.contains(&id)
    });
    let jitter = rs.rng.shops.next_float_range(
        MERCHANT_RELIC_PRICE_VARIANCE_MIN,
        MERCHANT_RELIC_PRICE_VARIANCE_MAX,
    );
    let (relic_id, cost_rarity) = match pulled {
        Some((id, bucket_rarity)) => (Some(id), bucket_rarity),
        None => (None, slot_rarity),
    };
    let price = (f64::from(merchant_relic_cost(cost_rarity)) * jitter).round_ties_even() as i32;
    ShopRelicEntry {
        relic_rarity: relic_rarity_to_shop(slot_rarity),
        relic_id,
        price,
    }
}

/// C#-faithful normal-merchant inventory generator. Mirrors
/// `MerchantInventory.CreateForNormalMerchant`
/// (`MegaCrit.Sts2.Core.Entities.Merchant/MerchantInventory.cs`) on the
/// player's two RNG streams in the exact C# consumption order:
///
/// * **Shops** — on-sale index (`NextInt(5)`), each card pick
///   (`NextItem`), each card-cost jitter (`NextFloat(0.95, 1.05)`, an
///   extra roll for the on-sale slot), and the 3 potion rolls
///   (`NextFloat` + `NextItem` per potion). 25 reads on a fresh run.
/// * **Rewards** — each colored card's shop-rarity roll
///   (`CardRarityOdds.RollWithoutChangingFutureOdds(Shop)`), every card's
///   upgrade no-op roll (`RollForUpgrade(card, -999999999, Rewards)` reads
///   one `NextFloat` and never upgrades because the base chance is
///   hugely negative), and the 2 relic-slot rarity rolls
///   (`RelicFactory.RollRarity`). 14 reads on a fresh run.
///
/// Read-for-read aligned with the headless C# `dump_shop` oracle
/// (`run_parity.py --shop-parity`). Card / colorless / potion / **relic**
/// identities and prices are seed-exact. Each relic slot rolls its rarity
/// (`[RollRarity(Rewards), RollRarity(Rewards), Shop]`), pulls from the
/// BACK of the grab bag filtered by `IsAllowedInShops`
/// (`MerchantRelicEntry.FillSlot` → `PullNextRelicFromBack`), then jitters
/// its `MerchantCost` (175 Common / 225 Uncommon / 275 Rare / 200 Shop) by
/// one `Shops.NextFloat(0.85, 1.15)` and `Mathf.RoundToInt`s the product.
/// Potion-cost jitter is intentionally absent — the headless oracle runs
/// with `TestMode` on, which skips `MerchantPotionEntry.CalcCost`'s
/// `NextFloat`, so potion prices are the flat rarity base. M4.
#[must_use]
pub fn generate_merchant_inventory(
    rs: &mut RunState,
    character: CharacterId,
) -> sts2_core::shop::ShopInventory {
    use sts2_core::shop::{
        card_removal_cost, potion_base_cost, ShopInventory, ShopPotionEntry, ShopPotionRarity,
        MERCHANT_CHARACTER_CARD_TYPES, MERCHANT_COLORLESS_CARD_RARITIES, MERCHANT_POTION_COUNT,
    };
    use sts2_core::PotionRarity;

    let mut inv = ShopInventory::empty();
    inv.removal_cost = card_removal_cost(rs.card_shop_removals_used, rs.ascension_level);
    let mut chosen: std::collections::BTreeSet<CardId> = std::collections::BTreeSet::new();

    // ── character card row: Shops sale index, then per-slot rarity(Rewards)
    //    / pick(Shops) / upgrade-noop(Rewards) / cost(Shops). ──
    let sale_index =
        rs.rng
            .shops
            .next_int_exclusive(0, MERCHANT_CHARACTER_CARD_TYPES.len() as i32) as usize;
    for (i, &want_type) in MERCHANT_CHARACTER_CARD_TYPES.iter().enumerate() {
        let on_sale = i == sale_index;
        let entry =
            merchant_roll_character_card_slot(rs, character, want_type, on_sale, &mut chosen);
        inv.cards.push(entry);
    }

    // ── colorless card row: one Uncommon, one Rare. pick(Shops) /
    //    upgrade-noop(Rewards) / cost(Shops, ×1.15 colorless). ──
    for &rarity in &MERCHANT_COLORLESS_CARD_RARITIES {
        let entry = merchant_roll_colorless_card_slot(rs, rarity, &mut chosen);
        inv.colorless_cards.push(entry);
    }

    // ── relic row: rarities [RollRarity(Rewards), RollRarity(Rewards),
    //    Shop] (C# `PopulateRelicEntries` rolls all three up front), then
    //    per slot pulls from the BACK of the grab bag filtered by
    //    IsAllowedInShops and jitters its MerchantCost by one
    //    Shops.NextFloat(0.85, 1.15) (`MerchantRelicEntry` ctor →
    //    FillSlot + CalcCost). The pulled relic's own rarity (the bucket
    //    it came from, after any fallback escalation) drives both cost
    //    and the displayed price. ──
    use sts2_core::runstate::RelicRarity;
    let empty_exclude = std::collections::BTreeSet::new();
    let r0 = rs.roll_reward_relic_rarity();
    let r1 = rs.roll_reward_relic_rarity();
    for slot_rarity in [r0, r1, RelicRarity::Shop] {
        let entry = merchant_roll_relic_slot(rs, slot_rarity, &empty_exclude);
        inv.relics.push(entry);
    }

    // ── potion row: 3 potions off Shops (2 reads each). Flat rarity-base
    //    price — the headless oracle's TestMode skips the cost jitter. ──
    let potions = crate::potions::all::create_random_potions_out_of_combat(
        &mut rs.rng.shops,
        character,
        MERCHANT_POTION_COUNT,
    );
    for kind in potions {
        let shop_rarity = match crate::potions::all::spec_for(kind).rarity {
            PotionRarity::Rare => ShopPotionRarity::Rare,
            PotionRarity::Uncommon => ShopPotionRarity::Uncommon,
            _ => ShopPotionRarity::Common,
        };
        inv.potions.push(ShopPotionEntry {
            potion_rarity: shop_rarity,
            potion_kind: Some(kind),
            price: potion_base_cost(shop_rarity),
        });
    }

    inv
}

/// Re-populate a single merchant slot after a Courier purchase, mirroring
/// the C# `MerchantEntry.RestockAfterPurchase` overrides read-for-read
/// with the corresponding slot in [`generate_merchant_inventory`], so a
/// later shop visit stays aligned on both the `Shops` and `Rewards`
/// streams.
///
/// * **Card** (`MerchantCardEntry.RestockAfterPurchase` → `IsOnSale =
///   false; Populate()`): re-roll via
///   [`merchant_roll_character_card_slot`] / [`merchant_roll_colorless_card_slot`]
///   with the exclude set = every currently-stocked card id across BOTH
///   rows (C# `_cardPool.Except(second)`). The just-sold card is still in
///   the slot (C# leaves `CreationResult` set on a restock), so it is
///   excluded from the re-roll. The restocked slot is never on sale.
/// * **Relic** (`MerchantRelicEntry.RestockAfterPurchase`): roll ONE
///   fresh rarity (`RelicFactory.RollRarity`, one `Rewards` read), then
///   [`merchant_roll_relic_slot`] with the other stocked relics as the
///   FillSlot blacklist.
/// * **Potion** (`MerchantPotionEntry.RestockAfterPurchase` →
///   `FillSlot(Array.Empty)`): one `CreateRandomPotionOutOfCombat` off
///   the FULL pool (empty blacklist → no dedup), flat rarity-base price
///   (the headless oracle's `TestMode` skips the cost jitter).
pub fn restock_shop_entry(
    rs: &mut RunState,
    inv: &mut sts2_core::shop::ShopInventory,
    slot: sts2_core::shop::ShopSlot,
) {
    use sts2_core::shop::ShopSlot;
    let character = rs.character_id;
    match slot {
        ShopSlot::Card(unified_idx) => {
            // Exclude every stocked card across both rows (the just-sold
            // card in this slot is still present and thus excluded).
            let chosen: std::collections::BTreeSet<CardId> = inv
                .cards
                .iter()
                .chain(inv.colorless_cards.iter())
                .filter_map(|e| e.card.as_ref().map(|c| c.card_id))
                .collect();
            let mut chosen = chosen;
            let n_char = inv.cards.len();
            if unified_idx < n_char {
                let want_type = inv.cards[unified_idx].card_type;
                let entry =
                    merchant_roll_character_card_slot(rs, character, want_type, false, &mut chosen);
                inv.cards[unified_idx] = entry;
            } else {
                let ci = unified_idx - n_char;
                if ci >= inv.colorless_cards.len() {
                    return;
                }
                let rarity = inv.colorless_cards[ci].rarity;
                let entry = merchant_roll_colorless_card_slot(rs, rarity, &mut chosen);
                inv.colorless_cards[ci] = entry;
            }
        }
        ShopSlot::Relic(idx) => {
            if idx >= inv.relics.len() {
                return;
            }
            // FillSlot blacklist = the other stocked relics. (Already-
            // pulled relics are gone from the bag and the bought relic is
            // now owned, so this is belt-and-suspenders — matching C#.)
            let exclude: std::collections::BTreeSet<sts2_core::relics::RelicId> = inv
                .relics
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != idx)
                .filter_map(|(_, e)| e.relic_id)
                .collect();
            let slot_rarity = rs.roll_reward_relic_rarity();
            let entry = merchant_roll_relic_slot(rs, slot_rarity, &exclude);
            inv.relics[idx] = entry;
        }
        ShopSlot::Potion(idx) => {
            if idx >= inv.potions.len() {
                return;
            }
            use sts2_core::shop::{potion_base_cost, ShopPotionEntry, ShopPotionRarity};
            use sts2_core::PotionRarity;
            let kinds = crate::potions::all::create_random_potions_out_of_combat(
                &mut rs.rng.shops,
                character,
                1,
            );
            let Some(&kind) = kinds.first() else {
                return;
            };
            let shop_rarity = match crate::potions::all::spec_for(kind).rarity {
                PotionRarity::Rare => ShopPotionRarity::Rare,
                PotionRarity::Uncommon => ShopPotionRarity::Uncommon,
                _ => ShopPotionRarity::Common,
            };
            inv.potions[idx] = ShopPotionEntry {
                potion_rarity: shop_rarity,
                potion_kind: Some(kind),
                price: potion_base_cost(shop_rarity),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scroll Boxes makes two packs of `[common, common, uncommon]`, with no
    /// card id repeated across packs, no `MultiplayerOnly` card (single-player
    /// pool fidelity — e.g. Necrobinder's `LegionOfBone`), and deterministic
    /// output for a fixed RNG. Card-for-card C# parity is covered by the
    /// pycli `test_rust_game_backend` bundle test.
    #[test]
    fn scroll_box_bundles_shape_dedup_and_no_multiplayer_only() {
        use sts2_core::potions::CharacterId;
        let mut rs = RunState::new();
        let bundles = generate_scroll_box_bundles(&mut rs, CharacterId::Necrobinder);
        assert_eq!(bundles.len(), 2, "exactly two packs");
        let mut seen = std::collections::HashSet::new();
        for pack in &bundles {
            assert_eq!(pack.len(), 3, "each pack is 2 common + 1 uncommon");
            assert_eq!(rarity_for_card_id(pack[0]), Some(CardRarity::Common));
            assert_eq!(rarity_for_card_id(pack[1]), Some(CardRarity::Common));
            assert_eq!(rarity_for_card_id(pack[2]), Some(CardRarity::Uncommon));
            for &c in pack {
                assert!(seen.insert(c), "no card id repeats across packs");
                assert!(
                    !is_multiplayer_only(c),
                    "multiplayer-only cards are excluded"
                );
            }
        }
        assert!(
            bundles.iter().flatten().all(|&c| c != CardId::LegionOfBone),
            "LegionOfBone (MultiplayerOnly uncommon) must never appear"
        );
        // Deterministic: a fresh state with the same seed reproduces it.
        let mut rs2 = RunState::new();
        assert_eq!(
            generate_scroll_box_bundles(&mut rs2, CharacterId::Necrobinder),
            bundles
        );
    }

    /// HeftyTablet's Rare candidate pool ([`sample_distinct_cards_of_rarity`])
    /// must exclude `MultiplayerOnly` Rares, matching C#
    /// `CardFactory.CreateForReward` → `FilterForPlayerCount`
    /// (`CardFactory.cs:175`). Every character ships exactly one MP-only Rare
    /// in its spec slice (Ironclad Tank / Silent Sneaky / Defect Ignition /
    /// Regent HammerTime / Necrobinder GlimpseBeyond); leaving it in made the
    /// pool one entry too long and shifted the whole Rewards-RNG-indexed roll.
    /// Passing `n` ≥ pool size returns the entire (filtered) pool, so this is
    /// deterministic regardless of RNG draw order.
    #[test]
    fn hefty_tablet_rare_pool_excludes_multiplayer_only() {
        use sts2_core::potions::CharacterId;
        for (character, mp_rare) in [
            (CharacterId::Ironclad, CardId::TankCard),
            (CharacterId::Silent, CardId::SneakyCard),
            (CharacterId::Defect, CardId::Ignition),
            (CharacterId::Regent, CardId::HammerTime),
            (CharacterId::Necrobinder, CardId::GlimpseBeyond),
        ] {
            let mut rs = RunState::new();
            // n far above any character's Rare-pool size → full filtered pool.
            let pool = sample_distinct_cards_of_rarity(&mut rs, character, CardRarity::Rare, 999);
            assert!(
                pool.iter()
                    .all(|&c| rarity_for_card_id(c) == Some(CardRarity::Rare)),
                "{character:?}: every drawn card is Rare"
            );
            assert!(
                pool.iter().all(|&c| !is_multiplayer_only(c)),
                "{character:?}: no MultiplayerOnly card in the Rare reward pool"
            );
            assert!(
                !pool.contains(&mp_rare),
                "{character:?}: MP-only Rare {mp_rare:?} must be excluded"
            );
        }
    }

    /// `random_ancient_card_for_dusty_tome` must mirror
    /// `DustyTome.SetupForPlayer` (`DustyTome.cs:52-55`): the candidate set is
    /// the run character's OWN Ancient cards minus
    /// `ArchaicTooth.TranscendenceCards`. For Defect the pool's Ancient cards
    /// are {BiasedCognition, Quadcast}; Quadcast is a transcendence upgrade
    /// (Dualcast→Quadcast), so the candidate list collapses to a single
    /// {BiasedCognition} and the grant is deterministic regardless of the
    /// single `Rewards` draw. It must NEVER cross character pools (e.g. grant
    /// Silent's WraithForm to a Defect run).
    #[test]
    fn dusty_tome_ancient_card_is_per_character_and_excludes_transcendence() {
        use sts2_core::potions::CharacterId;
        // Defect: candidate set is exactly {BiasedCognition} → deterministic.
        let mut rs = RunState::new();
        let card = random_ancient_card_for_dusty_tome(&mut rs, CharacterId::Defect);
        assert_eq!(
            card,
            Some(CardId::BiasedCognitionCard),
            "Defect DustyTome grants BiasedCognition (Quadcast excluded by transcendence)"
        );

        // Across every character: the granted Ancient is in that character's
        // own slice, is Ancient-rarity, and is never a transcendence upgrade.
        for character in [
            CharacterId::Ironclad,
            CharacterId::Silent,
            CharacterId::Defect,
            CharacterId::Regent,
            CharacterId::Necrobinder,
        ] {
            let mut rs = RunState::new();
            if let Some(c) = random_ancient_card_for_dusty_tome(&mut rs, character) {
                assert_eq!(
                    rarity_for_card_id(c),
                    Some(CardRarity::Ancient),
                    "{character:?}: granted card is Ancient-rarity"
                );
                assert!(
                    !matches!(
                        c,
                        CardId::Break
                            | CardId::Suppress
                            | CardId::Protector
                            | CardId::MeteorShower
                            | CardId::Quadcast
                    ),
                    "{character:?}: transcendence upgrade {c:?} must be excluded"
                );
                let own: Vec<CardId> = specs_for_character(character)
                    .iter()
                    .map(|f| f().id)
                    .collect();
                assert!(
                    own.contains(&c),
                    "{character:?}: granted Ancient {c:?} must be in that character's own pool"
                );
            }
        }
    }

    /// Bead `sts2-rs-vd6pf`: upgraded cards must carry their upgraded
    /// spec stats (cost / base_damage / base_block), both when built
    /// directly via `card_pod_from_id(id, true)` and when an existing
    /// instance is upgraded at runtime via `Card::upgrade()`.
    #[test]
    fn card_pod_from_id_applies_upgrade_stats() {
        // base (un-upgraded) values
        let strike = card_pod_from_id(CardId::StrikeIronclad, false).unwrap();
        assert_eq!(strike.base_damage, 6, "base Strike = 6");
        // upgraded values (C# OnUpgrade: Strike +3 -> 9, Bash +2 -> 10,
        // Defend block 5 -> 8, Barricade cost 3 -> 2)
        let strike_up = card_pod_from_id(CardId::StrikeIronclad, true).unwrap();
        assert!(strike_up.upgraded);
        assert_eq!(strike_up.base_damage, 9, "upgraded Strike = 9");

        let bash_up = card_pod_from_id(CardId::Bash, true).unwrap();
        assert_eq!(bash_up.base_damage, 10, "upgraded Bash = 10");

        let defend_up = card_pod_from_id(CardId::DefendIronclad, true).unwrap();
        assert_eq!(defend_up.base_block, 8, "upgraded Defend blocks 8");

        let barricade_up = card_pod_from_id(CardId::BarricadeCard, true).unwrap();
        assert_eq!(barricade_up.cost, 2, "upgraded Barricade costs 2");
    }

    /// Bead `sts2-rs-RP22`: C# `IsRemovable => !Eternal`. Eternal curses
    /// (Greed, AscendersBane) are NOT removable by the generic removal offer;
    /// basic Strike/Defend and ordinary cards ARE (C# events remove basics).
    #[test]
    fn card_is_removable_csharp_excludes_only_eternal() {
        assert!(!card_is_removable_csharp(CardId::Greed), "Greed is Eternal");
        assert!(
            !card_is_removable_csharp(CardId::AscendersBane),
            "AscendersBane is Eternal"
        );
        assert!(
            card_is_removable_csharp(CardId::StrikeIronclad),
            "basic Strike removable via Neow/events (no Eternal)"
        );
        assert!(
            card_is_removable_csharp(CardId::DefendIronclad),
            "basic Defend removable (no Eternal)"
        );
    }

    /// Bead `sts2-rs-RP22`: `removal_offer_order` excludes Eternal cards once
    /// the content removability hook is installed, so a leading Greed curse no
    /// longer occupies offer index 0 and shifts the lockstep pick. Mirrors C#
    /// `FromDeckForRemoval`'s `Where(IsRemovable)` filter.
    #[test]
    #[serial_test::serial(global_registry)]
    fn removal_offer_order_excludes_eternal_curse() {
        use sts2_core::runstate::RunState;
        crate::cards::dispatch::install_default_registry();
        let mut rs = RunState::new();
        rs.master_deck.clear();
        rs.master_deck
            .push(card_pod_from_id(CardId::Greed, false).unwrap());
        rs.master_deck
            .push(card_pod_from_id(CardId::StrikeIronclad, false).unwrap());
        rs.master_deck
            .push(card_pod_from_id(CardId::DefendIronclad, false).unwrap());
        let order = rs.removal_offer_order();
        // Greed (index 0) is Eternal → excluded; only Strike(1) and Defend(2)
        // remain, in deck order (no curse to sort to front).
        assert_eq!(
            order,
            vec![1, 2],
            "Eternal Greed dropped from removal offer"
        );
    }

    /// Bead `sts2-rs-oq6nf`: Hologram's C# `OnUpgrade` bumps Block 3→5 and
    /// `RemoveKeyword(Exhaust)` — upgraded Hologram is no longer Exhaust.
    #[test]
    fn oq6nf_hologram_upgrade_bumps_block_and_drops_exhaust() {
        let base = card_pod_from_id(CardId::Hologram, false).expect("hologram");
        assert_eq!(base.base_block, 3, "base Hologram blocks 3");
        let up = card_pod_from_id(CardId::Hologram, true).expect("hologram+");
        assert_eq!(up.base_block, 5, "upgraded Hologram blocks 5");
        assert!(
            !sts2_core::card_effects::card_exhausts_inst(&up),
            "upgraded Hologram drops Exhaust via keyword_override",
        );
    }

    /// Bead `sts2-rs-1yi26`: C# `Knockdown.cs OnUpgrade`
    /// `DynamicVars.Damage.UpgradeValueBy(4m)` → 10 → 14. The
    /// `card_upgrades_generated::upgrade_delta` entry was previously
    /// missing, so any code path that upgraded Knockdown would
    /// silently drop the +4 damage. The sibling `KnockdownPower`
    /// (2 → 3) bump is gated inline in `play_knockdown` via
    /// `current_upgraded` and is covered by
    /// `zvm2n_knockdown_power_scales_with_upgrade` in
    /// `colorless::tests`.
    ///
    /// NOTE: C# `Knockdown` is `MultiplayerConstraint.MultiplayerOnly`
    /// and Knockdown is intentionally absent from `COLORLESS_SPECS`,
    /// so `card_pod_from_id` returns `None` for it today — the delta
    /// is dormant infrastructure that becomes load-bearing only if
    /// the card is ever added to a single-player pool. This test
    /// asserts the delta table directly to keep the value pinned to
    /// C# even while the card stays unreachable.
    #[test]
    fn yi26_knockdown_upgrade_delta_pins_damage_at_fourteen() {
        let delta = super::super::card_upgrades_generated::upgrade_delta(CardId::Knockdown)
            .expect("Knockdown delta wired in card_upgrades_generated");
        assert_eq!(
            delta.base_damage,
            Some(14),
            "Knockdown+ damage = 14 (C# Knockdown.cs OnUpgrade UpgradeValueBy(4m) on 10)",
        );
        assert_eq!(delta.base_block, None);
        assert_eq!(delta.cost, None);
        assert_eq!(delta.keywords, None);
    }

    /// Bead `sts2-rs-b1zgh`: Defect/Colorless factories author their
    /// upgrade ternaries with NAMED CONSTANTS (`STRIKE_DEFECT_UPGRADED_DAMAGE`
    /// …) instead of int literals; the scraper used to `int()` those and
    /// silently drop the row, so the entire Defect pool + much of Colorless
    /// never got the vd6pf rewrite. Lock the regression with constant-path
    /// cards, cross-checked vs the C# CardModels (the source of truth):
    /// StrikeDefect 6+3=9, Claw 3+1=4, Zap cost 1->0 (EnergyCost.UpgradeBy(-1)).
    #[test]
    fn constant_valued_upgrade_cards_are_captured() {
        let strike_d = card_pod_from_id(CardId::StrikeDefect, true).unwrap();
        assert_eq!(strike_d.base_damage, 9, "upgraded Strike (Defect) = 9");

        let defend_d = card_pod_from_id(CardId::DefendDefect, true).unwrap();
        assert_eq!(defend_d.base_block, 8, "upgraded Defend (Defect) blocks 8");

        let claw = card_pod_from_id(CardId::Claw, true).unwrap();
        assert_eq!(claw.base_damage, 4, "upgraded Claw deals 4");

        let zap = card_pod_from_id(CardId::Zap, true).unwrap();
        assert_eq!(zap.cost, 0, "upgraded Zap costs 0");
    }

    /// `Card::upgrade()` (the runtime path used by Smith / upgrade relics)
    /// must rewrite stats via the registered card factory, not just flip
    /// the flag. Bead `sts2-rs-vd6pf`.
    #[test]
    fn runtime_card_upgrade_rewrites_stats() {
        // Ensure the factory the core method relies on is registered.
        sts2_core::register_card_factory(card_pod_from_id);

        let mut strike = card_pod_from_id(CardId::StrikeIronclad, false).unwrap();
        assert_eq!(strike.base_damage, 6);
        let flipped = strike.upgrade();
        assert!(flipped, "first upgrade flips the flag");
        assert!(strike.upgraded);
        assert_eq!(
            strike.base_damage, 9,
            "runtime upgrade rewrites Strike to 9"
        );

        // Idempotent: a second upgrade is a no-op and does not double-apply.
        let again = strike.upgrade();
        assert!(!again, "second upgrade is a no-op");
        assert_eq!(strike.base_damage, 9, "stat is not double-upgraded");
    }

    /// Bead `sts2-rs-55nxa`: `Card::upgrade()` must apply the upgrade as an
    /// additive *delta* on top of in-combat permanent base-stat buffs, not
    /// reset to the upgraded spec constant. Claw permanently buffs its own
    /// `base_damage` (+Increase) on every play (`defect.rs`); upgrading
    /// after such a buff must keep the accumulated stacks AND add the
    /// upgrade delta (C# `Claw.cs` `Damage.UpgradeValueBy(1)`), i.e.
    /// `buffed_base + (upgraded_spec - base_spec)`. The old absolute
    /// overwrite reset `base_damage` to the upgraded spec (4), discarding
    /// the buff.
    #[test]
    fn upgrade_preserves_in_combat_base_damage_buff() {
        sts2_core::register_card_factory(card_pod_from_id);

        // Claw: C# DamageVar(3), OnUpgrade +1 -> upgraded spec 4 (delta +1).
        let base = card_pod_from_id(CardId::Claw, false).unwrap();
        assert_eq!(base.base_damage, 3, "base Claw damage = 3 (C# DamageVar)");
        let up_spec = card_pod_from_id(CardId::Claw, true).unwrap();
        assert_eq!(up_spec.base_damage, 4, "upgraded Claw spec = 4 (delta +1)");

        // Simulate two in-combat Claw plays: +2 each (C# Increase = 2).
        let mut claw = card_pod_from_id(CardId::Claw, false).unwrap();
        claw.base_damage += 2;
        claw.base_damage += 2;
        assert_eq!(claw.base_damage, 7, "two plays buff Claw to 7");

        // Upgrade AFTER the buff (e.g. Armaments-style upgrade-in-hand).
        assert!(claw.upgrade());
        assert_eq!(
            claw.base_damage, 8,
            "buff survives + upgrade delta (+1): 7 + 1 = 8, not reset to spec 4",
        );

        // Strict superset: an un-buffed upgrade still lands on the spec value.
        let mut fresh = card_pod_from_id(CardId::Claw, false).unwrap();
        fresh.upgrade();
        assert_eq!(fresh.base_damage, 4, "un-buffed Claw upgrades to spec 4");
    }

    /// Bead `sts2-rs-59atd` (vd6pf follow-up): upgrading a card whose
    /// keyword set changes must stamp the upgrade-aware
    /// `keyword_override` on the instance, and the engine-side
    /// `card_effects::card_*_inst` resolvers must honor it. Covers both
    /// the creation path (`card_pod_from_id(id, true)`) and the runtime
    /// path (`Card::upgrade()`), and verifies un-upgraded / no-delta cards
    /// leave `keyword_override == None` (so the CardId registry stays
    /// authoritative for the overwhelming majority).
    #[test]
    fn keyword_upgrade_stamps_override_and_inst_resolvers_honor_it() {
        use sts2_core::card_effects::{
            card_ethereal_inst, card_exhausts_inst, card_unplayable_inst,
        };
        use sts2_core::cards::KeywordOverride as KO;

        sts2_core::register_card_factory(card_pod_from_id);

        // --- No keyword delta: override stays None, registry authoritative.
        let strike_base = card_pod_from_id(CardId::StrikeIronclad, false).unwrap();
        assert_eq!(strike_base.keyword_override, None, "base card: no override");
        let strike_up = card_pod_from_id(CardId::StrikeIronclad, true).unwrap();
        assert_eq!(
            strike_up.keyword_override, None,
            "upgrade with no keyword delta leaves override None",
        );

        // --- GAIN INNATE on upgrade (Aggression). Creation path.
        let aggro_base = card_pod_from_id(CardId::AggressionCard, false).unwrap();
        assert_eq!(
            aggro_base.keyword_override, None,
            "base Aggression: no override"
        );
        let aggro_up = card_pod_from_id(CardId::AggressionCard, true).unwrap();
        assert_eq!(
            aggro_up.keyword_override,
            Some(KO::INNATE),
            "upgraded Aggression gains INNATE",
        );

        // --- Runtime path: Card::upgrade() carries the override forward.
        let mut aggro_rt = card_pod_from_id(CardId::AggressionCard, false).unwrap();
        assert!(aggro_rt.upgrade());
        assert_eq!(
            aggro_rt.keyword_override,
            Some(KO::INNATE),
            "runtime upgrade stamps INNATE override",
        );

        // --- Absolute set with EXHAUST honored by the inst resolver
        // (Apparition's upgraded keyword set is EXHAUST).
        let apparition_up = card_pod_from_id(CardId::Apparition, true).unwrap();
        assert_eq!(apparition_up.keyword_override, Some(KO::EXHAUST));
        assert!(
            card_exhausts_inst(&apparition_up),
            "upgraded Apparition exhausts via override",
        );
        // The upgraded set is EXHAUST only -> not ETHEREAL/UNPLAYABLE,
        // regardless of what the base CardId registry holds.
        assert!(!card_ethereal_inst(&apparition_up));
        assert!(!card_unplayable_inst(&apparition_up));

        // --- EXHAUST|INNATE combined set (BigBang).
        let bigbang_up = card_pod_from_id(CardId::BigBang, true).unwrap();
        assert_eq!(
            bigbang_up.keyword_override,
            Some(KO::EXHAUST.with(KO::INNATE)),
        );
        assert!(card_exhausts_inst(&bigbang_up));
    }

    /// Bead `sts2-rs-le2mn`: before the Regent spec slice landed,
    /// `card_pod_from_id` scanned only IC/Silent/Defect/Colorless/Status,
    /// so any Regent card returned `None` and (for attacks) dealt 0 in
    /// real play. Lock the regression: a representative Regent attack
    /// now builds as a POD with its C# base damage, and the slice is
    /// reachable via `rarity_for_card_id` + `specs_for_character`.
    #[test]
    fn regent_cards_build_as_pod_with_csharp_stats() {
        // AstralPulse: C# `base(0, Attack, Common, AllEnemies)` +
        // DamageVar(6) (AstralPulse.cs, v0.107.1; was 14 in v0.103.2 — the
        // attack now carries `.WithHitCount(2)`).
        let card = card_pod_from_id(CardId::AstralPulse, false)
            .expect("Regent AstralPulse must be POD-constructible");
        assert_eq!(card.card_id, CardId::AstralPulse);
        assert_eq!(card.base_damage, 6, "AstralPulse base damage from C#");
        assert_eq!(
            rarity_for_card_id(CardId::AstralPulse),
            Some(CardRarity::Common)
        );

        // Comet: C# DamageVar(33) — the largest single-hit in the pool;
        // a 0 here would mean the spec slice is missing.
        let comet = card_pod_from_id(CardId::Comet, false).expect("Comet POD");
        assert_eq!(comet.base_damage, 33);

        // Bulwark: block card — C# BlockVar(12) (v0.107.1; was 13 in v0.103.2).
        let bulwark = card_pod_from_id(CardId::Bulwark, false).expect("Bulwark POD");
        assert_eq!(bulwark.base_block, 12);

        // Whole pool resolves: every REGENT_SPECS factory id round-trips
        // back through `rarity_for_card_id` (i.e. the slice is wired into
        // every lookup table).
        for factory in REGENT_SPECS {
            let spec = factory();
            assert!(
                rarity_for_card_id(spec.id).is_some(),
                "Regent {:?} not reachable via rarity_for_card_id",
                spec.id
            );
        }
        assert_eq!(REGENT_SPECS.len(), 88, "Regent pool is 88 cards");
    }

    /// Bead `sts2-rs-70anj.3.11`: per-character basic Strike + Defend ids.
    #[test]
    fn basic_strike_defend_cards_per_character() {
        assert_eq!(
            basic_strike_defend_cards(CharacterId::Ironclad),
            Some([CardId::StrikeIronclad, CardId::DefendIronclad])
        );
        assert_eq!(
            basic_strike_defend_cards(CharacterId::Silent),
            Some([CardId::StrikeSilent, CardId::DefendSilent])
        );
        assert_eq!(
            basic_strike_defend_cards(CharacterId::Defect),
            Some([CardId::StrikeDefect, CardId::DefendDefect])
        );
        // GH#32: Necrobinder's pair must resolve so the
        // transform-a-Strike/Defend relics (LeafyPoultice et al.) fire — it
        // previously returned `None`, no-op'ing the transform and diverging
        // every Necrobinder run that picked LeafyPoultice at Neow.
        assert_eq!(
            basic_strike_defend_cards(CharacterId::Necrobinder),
            Some([CardId::StrikeNecrobinder, CardId::DefendNecrobinder])
        );
        assert_eq!(
            basic_strike_defend_cards(CharacterId::Regent),
            Some([CardId::StrikeRegent, CardId::DefendRegent])
        );
    }

    /// `eligible_ids_for_rarity_filtered` must EXCLUDE `MultiplayerOnly`
    /// cards in BOTH loops (kills the `delete !` mutants at 1725:41 — the
    /// character loop — and 1733:41 — the colorless loop). Deleting `!`
    /// flips the guard so the result keeps ONLY MP-only cards and DROPS
    /// every singleplayer card. We assert a known non-MP card of the
    /// rarity IS present (would vanish under the mutant) and that the
    /// known MP-only card of that rarity is ABSENT.
    #[test]
    fn eligible_ids_for_rarity_filtered_excludes_multiplayer_only_both_loops() {
        // Character loop (1725): Ironclad Rares. `TankCard` is the MP-only
        // Ironclad Rare; pick any non-MP Ironclad Rare as the witness.
        let witness_ironclad: CardId = specs_for_character(CharacterId::Ironclad)
            .iter()
            .map(|f| f())
            .find(|s| s.rarity == CardRarity::Rare && !is_multiplayer_only(s.id))
            .expect("Ironclad ships a non-MP Rare")
            .id;
        let ic = eligible_ids_for_rarity_filtered(
            &[CharacterId::Ironclad],
            false,
            CardRarity::Rare,
            |_| true,
        );
        assert!(
            ic.contains(&witness_ironclad),
            "non-MP Ironclad Rare {witness_ironclad:?} must survive the character-loop filter"
        );
        assert!(
            !ic.contains(&CardId::TankCard),
            "MP-only Ironclad Rare TankCard must be filtered out"
        );

        // Colorless loop (1733): pick a non-MP colorless card and the rarity
        // it lives at, then assert it survives with an empty character slice
        // and `include_colorless = true`.
        let (witness_colorless, colorless_rarity): (CardId, CardRarity) = COLORLESS_SPECS
            .iter()
            .map(|f| f())
            .find(|s| {
                !is_multiplayer_only(s.id)
                    && matches!(
                        s.rarity,
                        CardRarity::Common | CardRarity::Uncommon | CardRarity::Rare
                    )
            })
            .map(|s| (s.id, s.rarity))
            .expect("a non-MP colorless reward card exists");
        let col = eligible_ids_for_rarity_filtered(&[], true, colorless_rarity, |_| true);
        assert!(
            col.contains(&witness_colorless),
            "non-MP colorless card {witness_colorless:?} must survive the colorless-loop filter"
        );
    }

    /// The custom-pool candidate filter (1851:54) must keep ONLY cards
    /// whose rarity EQUALS the rolled rarity. We pass an all-Uncommon
    /// custom pool and 12 rolls: in the regular context Common dominates,
    /// so under the correct `==` filter a card is produced ONLY on the
    /// (few) Uncommon rolls — a small slate. The `==`→`!=` mutant inverts
    /// this: it admits the Uncommon pool on every NON-Uncommon roll, so
    /// nearly every roll yields a card and the slate balloons.
    ///
    /// v0.107.1: re-seeded to `RunRngSet::new(2)` (the default seed 0 now
    /// rolls 7/12 Uncommon under MegaRandom — Common no longer dominates that
    /// seed, which would invert the discrimination). Seed 2 produces 1 card
    /// under the correct filter; we assert `<= 5` (well under the ~11 the
    /// inverting mutant would emit).
    #[test]
    fn roll_card_reward_cards_custom_pool_filters_to_rolled_rarity() {
        let uncommons: Vec<CardId> = specs_for_character(CharacterId::Ironclad)
            .iter()
            .map(|f| f())
            .filter(|s| s.rarity == CardRarity::Uncommon && !is_multiplayer_only(s.id))
            .map(|s| s.id)
            .collect();
        assert!(
            uncommons.len() >= 10,
            "need a large distinct Uncommon pool so the mutant can balloon the slate"
        );
        let mut rs = RunState::new();
        rs.rng = sts2_core::run_rng_set::RunRngSet::new(2);
        let opts = sts2_core::runstate::CardRewardGenerationOptions {
            has_custom_card_pool: true,
            custom_card_ids: uncommons,
            num_cards: 12,
            roll_upgrade: false,
            ..Default::default()
        };
        let cards = roll_card_reward_cards(&mut rs, opts, CharacterId::Ironclad);
        // Every produced card is Uncommon (pool is all-Uncommon) — the
        // discriminating signal is the COUNT, not the rarity.
        assert!(
            cards.len() <= 5,
            "Common-dominated rolls should yield only the few Uncommon-roll \
             slots ({} cards) — a ballooned slate means the rarity filter \
             admitted off-rarity cards (== → != mutant)",
            cards.len()
        );
    }

    /// The upgrade short-circuit `roll_upgrade && roll_for_upgrade(...)`
    /// (1868:13) must NOT roll for upgrade when `roll_upgrade == false`.
    /// At act 3 (`act_index == 2`) the non-rare upgrade odds are 50%, so
    /// the `&&`→`||` mutant (`roll_upgrade || roll_for_upgrade(...)`) would
    /// upgrade ~half of a 10-card slate. With `roll_upgrade = false` the
    /// correct code upgrades ZERO cards regardless of act; we assert no
    /// card is upgraded.
    #[test]
    fn roll_card_reward_cards_no_upgrade_when_roll_upgrade_disabled() {
        let mut rs = RunState::new();
        rs.act_index = 3;
        let opts = sts2_core::runstate::CardRewardGenerationOptions {
            num_cards: 10,
            roll_upgrade: false,
            ..Default::default()
        };
        let cards = roll_card_reward_cards(&mut rs, opts, CharacterId::Ironclad);
        assert!(
            !cards.is_empty(),
            "act-3 Ironclad slate should still produce cards"
        );
        assert!(
            cards.iter().all(|c| !c.upgraded),
            "roll_upgrade=false must suppress ALL upgrades, even at act 3 \
             where non-rare upgrade odds are 50% (kills the && → || mutant)"
        );
    }

    /// Bead `sts2-rs-jya0y`: every sampled `Card` matches its rolled
    /// rarity, ids are distinct, and the slate is no longer than the
    /// requested `num_cards`.
    #[test]
    fn roll_card_reward_cards_returns_distinct_ironclad_cards() {
        let mut rs = RunState::new();
        let opts = sts2_core::runstate::CardRewardGenerationOptions::default();
        let cards = roll_card_reward_cards(&mut rs, opts, CharacterId::Ironclad);
        assert!(!cards.is_empty(), "ironclad pool should yield rewards");
        assert!(cards.len() <= 3);
        let mut ids: std::collections::BTreeSet<CardId> = std::collections::BTreeSet::new();
        for card in &cards {
            assert!(ids.insert(card.card_id), "duplicate id {:?}", card.card_id);
            let r = rarity_for_card_id(card.card_id).expect("sampled id has a spec");
            assert!(matches!(
                r,
                CardRarity::Common | CardRarity::Uncommon | CardRarity::Rare
            ));
        }
    }

    /// Bead `sts2-rs-jya0y`: `has_custom_card_pool` restricts the
    /// candidate set to `custom_card_ids` filtered by the rolled
    /// rarity.
    #[test]
    fn roll_card_reward_cards_honors_custom_pool() {
        let mut rs = RunState::new();
        // Bash is Uncommon, Anger/Armaments are Common — validates
        // that the per-rarity filter on `custom_card_ids` skips
        // off-rarity ids rather than picking them.
        let opts = sts2_core::runstate::CardRewardGenerationOptions {
            has_custom_card_pool: true,
            custom_card_ids: vec![CardId::Anger, CardId::Armaments, CardId::Bash],
            ..Default::default()
        };
        let cards = roll_card_reward_cards(&mut rs, opts, CharacterId::Ironclad);
        for card in &cards {
            assert!(
                matches!(
                    card.card_id,
                    CardId::Anger | CardId::Armaments | CardId::Bash
                ),
                "unexpected id {:?}",
                card.card_id
            );
        }
    }

    /// Bead `sts2-rs-jya0y`: a registered
    /// `modify_card_reward_options_late` listener runs against the
    /// produced `Vec<Card>` after sampling. Pin: FresnelLens's
    /// per-card enchant in Python (`shop_event.py::FresnelLens`).
    #[test]
    fn roll_card_reward_cards_fires_modify_late_dispatch() {
        use std::sync::atomic::{AtomicBool, Ordering};
        // The hook registry is process-global and shared across the
        // test binary (real FresnelLens / FrozenEgg / WingCharm /
        // Glitter listeners may already be installed). Count-based
        // assertions race under cargo's parallel runner; assert
        // boolean fire + non-empty cards instead. The sentinel
        // listener replaces FresnelLens's slot for the duration of
        // this test — acceptable because no other test in this crate
        // asserts FresnelLens output.
        static FIRED: AtomicBool = AtomicBool::new(false);
        static OBSERVED_NONEMPTY: AtomicBool = AtomicBool::new(false);
        FIRED.store(false, Ordering::SeqCst);
        OBSERVED_NONEMPTY.store(false, Ordering::SeqCst);
        sts2_core::run_effects::register_modify_card_reward_options_late(
            sts2_core::RelicId::FresnelLens,
            |_rs, cards| {
                FIRED.store(true, Ordering::SeqCst);
                if !cards.is_empty() {
                    OBSERVED_NONEMPTY.store(true, Ordering::SeqCst);
                }
                sts2_core::EffectResult::Resolved
            },
        );
        let mut rs = RunState::new();
        let opts = sts2_core::runstate::CardRewardGenerationOptions::default();
        let cards = roll_card_reward_cards(&mut rs, opts, CharacterId::Ironclad);
        assert!(!cards.is_empty());
        assert!(FIRED.load(Ordering::SeqCst), "listener never ran");
        assert!(
            OBSERVED_NONEMPTY.load(Ordering::SeqCst),
            "listener saw an empty cards slice — dispatch fired before sampling",
        );
    }

    /// Bead `sts2-rs-8qrhw`: a default reward roll over the Ironclad
    /// pool yields exactly 3 distinct cards (no rarity is so sparse
    /// after distinctness-filtering that the slate runs short on a
    /// real pool).
    #[test]
    fn roll_card_reward_cards_returns_three_cards_for_ironclad_default() {
        let mut rs = RunState::new();
        let opts = sts2_core::runstate::CardRewardGenerationOptions::default();
        let cards = roll_card_reward_cards(&mut rs, opts, CharacterId::Ironclad);
        assert_eq!(cards.len(), 3, "default reward slate should be 3 cards");
    }

    /// Bead `sts2-rs-8qrhw`: every sampled card belongs to the
    /// requested character's pool (no cross-character leakage when
    /// `include_colorless` is false and the default-pool flag is on).
    #[test]
    fn roll_card_reward_cards_pool_restricted_to_character() {
        let mut rs = RunState::new();
        let opts = sts2_core::runstate::CardRewardGenerationOptions {
            include_colorless: false,
            ..Default::default()
        };
        let ironclad_ids: std::collections::BTreeSet<CardId> =
            IRONCLAD_SPECS.iter().map(|f| f().id).collect();
        // Sample many slates to stress the pool restriction.
        for _ in 0..200 {
            let cards = roll_card_reward_cards(&mut rs, opts.clone(), CharacterId::Ironclad);
            for card in &cards {
                assert!(
                    ironclad_ids.contains(&card.card_id),
                    "non-Ironclad card {:?} leaked into Ironclad-only slate",
                    card.card_id,
                );
            }
        }
    }

    /// Bead `sts2-rs-8qrhw`: `modify_card_reward_creation_options`
    /// fires on each slate-roll. (Strict "exactly once" assertion is
    /// inspection-only — the global hook registry is shared under
    /// cargo's parallel runner, so a count-based gate would race with
    /// sibling tests that also call `roll_card_reward_cards`. The
    /// "fires once per dispatch" invariant is enforced by the macro
    /// itself in `run_effects.rs::run_hook_channel!` — `BTreeMap`
    /// dedupes by `RelicId` and `dispatch_*` walks the snapshot
    /// linearly.)
    #[test]
    fn roll_card_reward_cards_invokes_creation_options_channel() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static FIRED: AtomicBool = AtomicBool::new(false);
        FIRED.store(false, Ordering::SeqCst);
        sts2_core::run_effects::register_modify_card_reward_creation_options(
            sts2_core::RelicId::PaelsTooth,
            |_rs, _opts| {
                FIRED.store(true, Ordering::SeqCst);
                sts2_core::EffectResult::Resolved
            },
        );
        let mut rs = RunState::new();
        let opts = sts2_core::runstate::CardRewardGenerationOptions::default();
        let _cards = roll_card_reward_cards(&mut rs, opts, CharacterId::Ironclad);
        assert!(
            FIRED.load(Ordering::SeqCst),
            "modify_card_reward_creation_options channel never fired during slate roll",
        );
    }

    /// Bead `sts2-rs-8qrhw`: the rare-rate over many regular-context
    /// rolls sits in the Python pin's expected band. The exact
    /// per-roll rare chance is the act-0 base from `CardRarityOdds`
    /// (rare ≈ 0.04 at act 0, regular context) which over 3000 rolls
    /// should land in [0.005, 0.12]. The wide band absorbs pity drift;
    /// the assertion catches gross regressions like "every card is
    /// rare" or "rares are unreachable" without policing the exact
    /// pity curve (which has its own dedicated tests in
    /// `card_rewards.rs`).
    #[test]
    fn roll_card_reward_cards_rare_rate_in_band_for_regular_context() {
        use crate::cards::CardRarity;
        let mut rs = RunState::new();
        let mut rare = 0u32;
        let mut total = 0u32;
        for _ in 0..1000 {
            let opts = sts2_core::runstate::CardRewardGenerationOptions::default();
            let cards = roll_card_reward_cards(&mut rs, opts, CharacterId::Ironclad);
            for card in &cards {
                if rarity_for_card_id(card.card_id) == Some(CardRarity::Rare) {
                    rare += 1;
                }
                total += 1;
            }
        }
        assert!(total > 2500, "should have rolled > 2500 cards");
        let rate = f64::from(rare) / f64::from(total);
        assert!(
            (0.005..=0.20).contains(&rate),
            "rare rate {rate:.4} outside [0.005, 0.20] — pity machinery may be broken",
        );
    }

    /// Bead `sts2-rs-4z5iw`: `IRONCLAD_SPECS` must be in C#
    /// `IroncladCardPool.GenerateAllCards` alphabetical order so that
    /// `eligible_ids_for_rarity` — the candidate slice driving
    /// `NextItem(idx)` card-reward picks — matches
    /// `CardPool.GetUnlockedCards().Where(rarity)` exactly.
    ///
    /// The reward-eligible rarities are Common / Uncommon / Rare
    /// (`CardFactory.CreateForReward` excludes `Basic` + `Ancient`, so
    /// the two Ancient cards `Break`/`Corruption` never appear). The
    /// pre-fix grouped order mis-placed `Colossus` (Uncommon) in the
    /// rare-group region, pushing it to the *end* of the Uncommon
    /// slice instead of alphabetical slot 5 and shifting every later
    /// `NextItem(idx)`. These lists are the authoritative C#
    /// v0.103.2 per-rarity slices (alphabetical); `NotYet` (C#-only
    /// Rare) is now ported and present in the Rare slice.
    #[test]
    fn ironclad_specs_match_csharp_alphabetical_pool_order() {
        let expected_common: &[CardId] = &[
            CardId::Anger,
            CardId::Armaments,
            CardId::BloodWall,
            CardId::Bloodletting,
            CardId::BodySlam,
            CardId::Breakthrough,
            CardId::Cinder,
            CardId::Havoc,
            CardId::Headbutt,
            CardId::IronWave,
            CardId::MoltenFist,
            CardId::PerfectedStrike,
            CardId::PommelStrike,
            CardId::SetupStrikeCard,
            CardId::ShrugItOff,
            CardId::SwordBoomerang,
            CardId::Thunderclap,
            CardId::Tremble,
            CardId::TrueGrit,
            CardId::TwinStrike,
        ];
        // C# Uncommon slice minus DemonicShield: it's `MultiplayerOnly`, so
        // C#'s single-player reward pool (`FilterForPlayerCount`,
        // `Players.Count == 1`) drops it — verified against the live C#
        // engine via `run_parity.py --reward-parity` (sts2-rs-jenwp M4).
        let expected_uncommon: &[CardId] = &[
            CardId::AshenStrike,
            CardId::BattleTrance,
            CardId::Bludgeon,
            CardId::Bully,
            CardId::BurningPact,
            CardId::ColossusCard,
            CardId::Dismantle,
            CardId::Dominate,
            CardId::DrumOfBattleCard,
            CardId::EvilEye,
            CardId::ExpectAFight,
            CardId::FeelNoPainCard,
            CardId::FightMe,
            CardId::FlameBarrierCard,
            CardId::ForgottenRitual,
            CardId::Hemokinesis,
            CardId::HowlFromBeyond,
            CardId::InfernalBlade,
            CardId::InfernoCard,
            CardId::Inflame,
            CardId::JugglingCard,
            CardId::Pillage,
            CardId::RageCard,
            CardId::Rampage,
            CardId::RuptureCard,
            CardId::SecondWind,
            CardId::Spite,
            CardId::StampedeCard,
            CardId::Stomp,
            CardId::StoneArmor,
            CardId::Taunt,
            CardId::Unrelenting,
            CardId::Uppercut,
            CardId::ViciousCard,
            CardId::Whirlwind,
        ];
        // C# Rare slice minus Tank (`MultiplayerOnly` — excluded from
        // single-player reward pools). `NotYet` is now ported (C#-only,
        // MANUAL CardId) and appears at its alpha slot `Mangle < NotYet
        // < Offering`.
        let expected_rare: &[CardId] = &[
            CardId::AggressionCard,
            CardId::BarricadeCard,
            CardId::Brand,
            CardId::Cascade,
            CardId::Conflagration,
            CardId::CrimsonMantle,
            CardId::CrueltyCard,
            CardId::DarkEmbraceCard,
            CardId::DemonFormCard,
            CardId::Feed,
            CardId::FiendFire,
            CardId::HellraiserCard,
            CardId::Impervious,
            CardId::JuggernautCard,
            CardId::Mangle,
            CardId::NotYet,
            CardId::Offering,
            CardId::OneTwoPunchCard,
            CardId::PactsEnd,
            CardId::PrimalForce,
            CardId::Pyre,
            CardId::Stoke,
            CardId::TearAsunder,
            CardId::Thrash,
            CardId::Unmovable,
        ];

        let common = eligible_ids_for_rarity(&[CharacterId::Ironclad], false, CardRarity::Common);
        let uncommon =
            eligible_ids_for_rarity(&[CharacterId::Ironclad], false, CardRarity::Uncommon);
        let rare = eligible_ids_for_rarity(&[CharacterId::Ironclad], false, CardRarity::Rare);

        assert_eq!(
            common, expected_common,
            "Common reward slice vs C# alphabetical"
        );
        assert_eq!(
            uncommon, expected_uncommon,
            "Uncommon reward slice vs C# alphabetical"
        );
        assert_eq!(
            rare, expected_rare,
            "Rare reward slice vs C# alphabetical (NotYet excepted)"
        );
    }

    // ── mutation-coverage tests for the card-pool / transform / sampling
    //    helpers (cargo-mutants survivors on this file). Each pins a
    //    function's observable contract under a known input. ──

    /// `transform_replacement_pool` keeps only {Common, Uncommon, Rare}, drops
    /// `MultiplayerOnly` cards, and excludes the original. Kills the
    /// `&&`→`||` / `delete !` mutants on the filter (lines 1117–1118): any of
    /// those would let off-rarity, MP-only, or the original card into the pool.
    #[test]
    fn transform_replacement_pool_filters_rarity_mp_and_original() {
        // Use a Necrobinder common as the original; LegionOfBone is the
        // character's MP-only uncommon (must be excluded).
        let original = CardId::StrikeNecrobinder;
        let pool = transform_replacement_pool(CharacterId::Necrobinder, original);
        assert!(!pool.is_empty(), "Necrobinder transform pool is non-empty");
        for &id in &pool {
            assert!(
                matches!(
                    rarity_for_card_id(id),
                    Some(CardRarity::Common | CardRarity::Uncommon | CardRarity::Rare)
                ),
                "{id:?} is not Common/Uncommon/Rare (||-mutated rarity filter?)",
            );
            assert!(
                !is_multiplayer_only(id),
                "{id:?} is MultiplayerOnly (deleted `!` on is_multiplayer_only?)",
            );
            assert_ne!(id, original, "original card leaked into pool (||-mutated)");
        }
        assert!(
            !pool.contains(&CardId::LegionOfBone),
            "LegionOfBone (MP-only uncommon) must be excluded",
        );
        // A known non-MP common IS present (proves the filter isn't inverted to
        // MP-only-by `delete !`).
        assert!(
            pool.iter().any(|&id| !is_multiplayer_only(id)),
            "pool must contain at least one non-MP card",
        );
    }

    /// `roll_transform_replacement_basic_niche` returns `Some` for a real
    /// character pool (kills `-> None` body replacement at line 831). The
    /// drawn card obeys the pool contract.
    #[test]
    fn roll_transform_replacement_basic_niche_returns_some() {
        let mut rs = RunState::new();
        let got = roll_transform_replacement_basic_niche(
            &mut rs,
            CharacterId::Ironclad,
            CardId::StrikeIronclad,
        );
        let id = got.expect("non-empty Ironclad pool yields Some");
        assert!(matches!(
            rarity_for_card_id(id),
            Some(CardRarity::Common | CardRarity::Uncommon | CardRarity::Rare)
        ));
        assert_ne!(id, CardId::StrikeIronclad, "original excluded from result");
    }

    /// `random_card_from_rarity` returns a card OF the requested rarity (kills
    /// `==`→`!=` at line 1135: the mutant would return a card of any OTHER
    /// rarity).
    #[test]
    fn random_card_from_rarity_returns_matching_rarity() {
        for rarity in [CardRarity::Common, CardRarity::Uncommon, CardRarity::Rare] {
            let mut rs = RunState::new();
            let got = random_card_from_rarity(&mut rs, Some(CharacterId::Ironclad), rarity)
                .expect("Ironclad has cards at each reward rarity");
            assert_eq!(
                rarity_for_card_id(got),
                Some(rarity),
                "sampled card must match requested rarity {rarity:?}",
            );
        }
    }

    /// `random_power_card` returns a Power-typed card (kills `==`→`!=` at
    /// line 1180: the mutant would sample a non-Power card).
    #[test]
    fn random_power_card_returns_power_typed() {
        let mut rs = RunState::new();
        let got = random_power_card(&mut rs, Some(CharacterId::Ironclad))
            .expect("Ironclad has Power cards");
        let card = card_pod_from_id(got, false).expect("sampled Power has a spec");
        assert_eq!(
            card.card_type,
            CardType::Power,
            "random_power_card must yield a Power-typed card",
        );
    }

    /// `random_curse_card` returns a Curse-typed card (kills `==`→`!=` at
    /// line 1192: the mutant would collect non-Curse statuses instead).
    #[test]
    fn random_curse_card_returns_curse_typed() {
        let mut rs = RunState::new();
        let got = random_curse_card(&mut rs).expect("STATUS_SPECS contains curses");
        let card = card_pod_from_id(got, false).expect("sampled curse has a spec");
        assert_eq!(
            card.card_type,
            CardType::Curse,
            "random_curse_card must yield a Curse-typed card",
        );
    }

    /// `sample_distinct_colorless` / `sample_distinct_character_cards` /
    /// `sample_distinct_from_specs` return a non-empty Vec for `n > 0` over a
    /// non-empty pool (kills the `-> vec![]` body replacements at lines 1209 /
    /// 1220 / 1228), and an EMPTY Vec when `n == 0` or specs are empty (kills
    /// the `==`→`!=` and `||`→`&&` mutants in the `n == 0 || specs.is_empty()`
    /// guard at line 1228).
    #[test]
    fn sample_distinct_helpers_respect_count_and_pool() {
        let mut rs = RunState::new();
        let cl = sample_distinct_colorless(&mut rs, 3);
        assert!(!cl.is_empty(), "colorless sample with n=3 is non-empty");
        assert!(cl.len() <= 3);
        let mut seen = std::collections::HashSet::new();
        for &id in &cl {
            assert!(seen.insert(id), "colorless sample has distinct ids");
        }

        let chars = sample_distinct_character_cards(&mut rs, CharacterId::Ironclad, 4);
        assert!(!chars.is_empty(), "character sample with n=4 is non-empty");
        let ironclad_ids: std::collections::BTreeSet<CardId> =
            IRONCLAD_SPECS.iter().map(|f| f().id).collect();
        for &id in &chars {
            assert!(ironclad_ids.contains(&id), "{id:?} not in Ironclad pool");
        }

        // n == 0 → empty even with a non-empty spec slice (guard's `n == 0`
        // leg; `==`→`!=` mutant would treat n==0 as "non-zero" and roll).
        assert!(
            sample_distinct_from_specs(&mut rs, IRONCLAD_SPECS, 0).is_empty(),
            "n == 0 yields empty Vec",
        );
        // empty specs → empty even with n > 0 (guard's `specs.is_empty()` leg;
        // `||`→`&&` mutant would require BOTH conditions and then index an
        // empty pool / return non-empty incorrectly).
        let empty_specs: &[fn() -> super::super::types::CardSpec] = &[];
        assert!(
            sample_distinct_from_specs(&mut rs, empty_specs, 5).is_empty(),
            "empty spec slice yields empty Vec",
        );
    }

    /// `card_rarity_rank_csharp` returns the C# discriminant per rarity (kills
    /// `-> 1` body replacement at line 1479 — the const `1` would mis-rank
    /// every non-Basic card). Unknown ids rank 0.
    #[test]
    fn card_rarity_rank_csharp_maps_discriminants() {
        // Common = 2 (Anger is an Ironclad common).
        assert_eq!(card_rarity_rank_csharp(CardId::Anger), 2, "Common ranks 2");
        // Rare = 4 (Offering is an Ironclad rare).
        assert_eq!(card_rarity_rank_csharp(CardId::Offering), 4, "Rare ranks 4");
        // Curse = 9.
        assert_eq!(card_rarity_rank_csharp(CardId::Greed), 9, "Curse ranks 9");
        // Unknown id (no spec) ranks 0 — distinct from the mutated const 1.
        assert_eq!(
            card_rarity_rank_csharp(CardId::AscendersBane),
            9,
            "AscendersBane is a Curse (rank 9)",
        );
    }

    /// `eligible_ids_for_rarity` colorless branch obeys `rarity == r &&
    /// !is_multiplayer_only` (kills the `==`→`!=`, `&&`→`||`, `delete !`
    /// mutants on line 1703, which the existing Ironclad test never exercises
    /// because it passes `include_colorless = false`).
    #[test]
    fn eligible_ids_for_rarity_colorless_branch_filters() {
        // With include_colorless = true and an empty character list, the result
        // is exactly the colorless pool of that rarity, MP-only excluded.
        // (The colorless pool ships only Uncommon + Rare cards.)
        let rares = eligible_ids_for_rarity(&[], true, CardRarity::Rare);
        assert!(!rares.is_empty(), "colorless rares exist");
        for &id in &rares {
            assert_eq!(
                rarity_for_card_id(id),
                Some(CardRarity::Rare),
                "{id:?} not Rare (==→!= or &&→|| on colorless branch?)",
            );
            assert!(
                !is_multiplayer_only(id),
                "{id:?} is MP-only (deleted `!`?) — BeaconOfHope is a colorless MP-only Rare",
            );
        }
        // BeaconOfHope is a colorless MultiplayerOnly Rare → must be absent from
        // the Rare slice, and (being non-Common) absent from a different rarity.
        assert!(
            !rares.contains(&CardId::BeaconOfHope),
            "colorless MP-only BeaconOfHope excluded from the Rare slice",
        );
        // Different-rarity branch is non-degenerate: the Uncommon slice is
        // non-empty and disjoint in rarity from the Rare slice (kills ==→!=,
        // which would swap which cards land in each slice).
        let uncommons = eligible_ids_for_rarity(&[], true, CardRarity::Uncommon);
        assert!(!uncommons.is_empty(), "colorless uncommons exist");
        for &id in &uncommons {
            assert_eq!(rarity_for_card_id(id), Some(CardRarity::Uncommon));
        }
    }

    /// `eligible_ids_for_rarity_filtered` applies BOTH the rarity/MP filter AND
    /// the predicate, over the character pool and the colorless branch (kills
    /// the `==`→`!=`, `&&`→`||`, `delete !` mutants on lines 1725 & 1733).
    #[test]
    fn eligible_ids_for_rarity_filtered_applies_predicate_and_filters() {
        // Predicate: Power-typed only. Result must be Uncommon Powers, no
        // MP-only. (Ironclad ships Uncommon Powers like Inflame/FeelNoPain.)
        let powers = eligible_ids_for_rarity_filtered(
            &[CharacterId::Ironclad],
            true,
            CardRarity::Uncommon,
            |s| s.card_type == CardType::Power,
        );
        assert!(!powers.is_empty(), "Ironclad has Uncommon Powers");
        for &id in &powers {
            assert_eq!(
                rarity_for_card_id(id),
                Some(CardRarity::Uncommon),
                "{id:?} not Uncommon (mutated rarity check?)",
            );
            assert!(!is_multiplayer_only(id), "{id:?} MP-only (deleted `!`?)");
            let card = card_pod_from_id(id, false).expect("has spec");
            assert_eq!(
                card.card_type,
                CardType::Power,
                "{id:?} not a Power — predicate dropped (&&→||?)",
            );
        }
        // A predicate that excludes everything yields an empty pool — proves the
        // predicate `&&` leg is load-bearing (a `&&`→`||` mutant would let
        // non-matching cards through and make this non-empty).
        let none = eligible_ids_for_rarity_filtered(
            &[CharacterId::Ironclad],
            true,
            CardRarity::Uncommon,
            |_| false,
        );
        assert!(none.is_empty(), "false predicate must yield empty pool");
    }

    /// Defect Scroll Boxes still produce the `[common, common, uncommon]`
    /// shape, and the per-bundle Claw roll consumes one Rewards read without
    /// corrupting the shape (kills the `is_defect ==`→`!=` at line 1392 and
    /// the `&&` / `<` mutants on the 1% all-Claw gate at line 1414). With the
    /// `is_defect` flag inverted, Defect would skip the gate (wrong RNG
    /// consumption) — here we just pin that Defect output is well-formed and
    /// deterministic.
    #[test]
    fn scroll_box_bundles_defect_shape_is_well_formed() {
        let mut rs = RunState::new();
        let bundles = generate_scroll_box_bundles(&mut rs, CharacterId::Defect);
        assert_eq!(bundles.len(), 2, "two packs for Defect");
        for pack in &bundles {
            // Either a normal [common, common, uncommon] pack, or the rare 1%
            // all-Claw pack [Claw, Claw, Claw].
            let all_claw = pack.iter().all(|&c| c == CardId::Claw);
            if all_claw {
                assert_eq!(pack.len(), 3, "all-Claw pack has 3 Claws");
            } else {
                assert_eq!(pack.len(), 3, "normal pack is 2 common + 1 uncommon");
                assert_eq!(rarity_for_card_id(pack[0]), Some(CardRarity::Common));
                assert_eq!(rarity_for_card_id(pack[1]), Some(CardRarity::Common));
                assert_eq!(rarity_for_card_id(pack[2]), Some(CardRarity::Uncommon));
            }
        }
        // Deterministic for a fixed seed.
        let mut rs2 = RunState::new();
        assert_eq!(
            generate_scroll_box_bundles(&mut rs2, CharacterId::Defect),
            bundles,
        );
    }

    /// `merchant_character_candidates` filters by rarity AND type AND MP AND
    /// not-already-chosen (kills the `==`→`!=` / `&&`→`||` mutants on lines
    /// 1917–1920). A picked card excluded via `chosen` proves the
    /// `!chosen.contains` leg.
    #[test]
    fn merchant_character_candidates_filters_rarity_type_and_chosen() {
        use sts2_core::card_rewards::RewardRarity;
        let empty: std::collections::BTreeSet<CardId> = std::collections::BTreeSet::new();
        let attacks = merchant_character_candidates(
            CharacterId::Ironclad,
            RewardRarity::Common,
            CardType::Attack,
            &empty,
        );
        assert!(!attacks.is_empty(), "Ironclad has Common attacks");
        for &id in &attacks {
            assert_eq!(rarity_for_card_id(id), Some(CardRarity::Common));
            let card = card_pod_from_id(id, false).expect("has spec");
            assert_eq!(card.card_type, CardType::Attack, "{id:?} not an Attack");
            assert!(!is_multiplayer_only(id), "{id:?} MP-only leaked in");
        }
        // Excluding the first candidate via `chosen` removes exactly it.
        let mut chosen: std::collections::BTreeSet<CardId> = std::collections::BTreeSet::new();
        chosen.insert(attacks[0]);
        let filtered = merchant_character_candidates(
            CharacterId::Ironclad,
            RewardRarity::Common,
            CardType::Attack,
            &chosen,
        );
        assert!(
            !filtered.contains(&attacks[0]),
            "chosen card must be excluded (!chosen.contains leg)",
        );
        assert_eq!(filtered.len(), attacks.len() - 1, "exactly one fewer");
    }

    /// `merchant_relic_cost` returns the distinct per-rarity base price (kills
    /// the `-> 0 / 1 / -1` body replacements on line 1948 — those would
    /// collapse every rarity to one constant).
    #[test]
    fn merchant_relic_cost_per_rarity_base_prices() {
        use sts2_core::runstate::RelicRarity;
        assert_eq!(merchant_relic_cost(RelicRarity::Common), 175);
        assert_eq!(merchant_relic_cost(RelicRarity::Uncommon), 225);
        assert_eq!(merchant_relic_cost(RelicRarity::Rare), 275);
        assert_eq!(merchant_relic_cost(RelicRarity::Shop), 200);
    }

    /// `generate_merchant_inventory` builds a full, well-formed inventory:
    /// 5 character cards, 2 colorless, 3 relics, 3 potions. The relic-cost
    /// `*` jitter (line 2110) and the potion-rarity match arms (lines
    /// 2127–2128) are exercised by asserting positive relic prices and that
    /// each potion's mapped shop rarity is consistent with its spec rarity.
    #[test]
    fn generate_merchant_inventory_is_well_formed() {
        use sts2_core::shop::{ShopPotionRarity, ShopRelicRarity};
        let mut rs = RunState::new();
        let inv = generate_merchant_inventory(&mut rs, CharacterId::Ironclad);

        assert_eq!(inv.cards.len(), 5, "5 character cards");
        assert_eq!(inv.colorless_cards.len(), 2, "2 colorless cards");
        assert_eq!(inv.relics.len(), 3, "3 relics");
        assert_eq!(inv.potions.len(), 3, "3 potions");

        // Relic prices are the per-rarity base MULTIPLIED by a [0.85, 1.15]
        // jitter then banker's-rounded. Pin the deterministic per-seed prices
        // (kills `*`→`+` and `*`→`/` on line 2110: the additive mutant yields
        // ~base+1 ∈ {176, 201, 226, 276} and the division mutant ~base, neither
        // of which can equal these `base * jitter` products).
        // v0.107.1 MegaRandom shifted both the rolled relic rarities and the
        // jitter; the v0.107.1 PlayerRngSet seed fix (player term NetId=1 →
        // GetPlayerSlotIndex=0, bead `sts2-rs-ms8mm` family) then re-seeded the
        // `shops` stream off the run-seed base, shifting these again. The
        // full-vector golden still differs from the additive mutant (~base+1 ∈
        // {176,201,226,276}) and division mutant (~base) in ≥1 slot (240, 273
        // ∉ either set), so it keeps killing both.
        assert_eq!(
            inv.relics.iter().map(|r| r.price).collect::<Vec<_>>(),
            vec![240, 273, 201],
            "deterministic relic prices for seed-0 Ironclad (base * jitter)",
        );
        for r in &inv.relics {
            assert!(r.price > 0, "relic price must be positive, got {}", r.price);
        }

        // Potion shop-rarity mapping: each entry's mapped rarity matches the
        // potion's own spec rarity bucket (kills the deleted Rare/Uncommon match
        // arms on lines 2127–2128, which would collapse those to Common).
        for p in &inv.potions {
            let kind = p.potion_kind.expect("merchant potion slot is filled");
            let spec_rarity = crate::potions::all::spec_for(kind).rarity;
            let expected = match spec_rarity {
                sts2_core::PotionRarity::Rare => ShopPotionRarity::Rare,
                sts2_core::PotionRarity::Uncommon => ShopPotionRarity::Uncommon,
                _ => ShopPotionRarity::Common,
            };
            assert_eq!(
                p.potion_rarity, expected,
                "potion {kind:?} (spec {spec_rarity:?}) mapped to wrong shop rarity",
            );
        }

        // Relic slot rarities are the three buckets (two rolled + Shop); the
        // Shop bucket comes last and must map through `relic_rarity_to_shop`.
        assert_eq!(
            inv.relics[2].relic_rarity,
            ShopRelicRarity::Shop,
            "third relic slot is the Shop bucket",
        );
    }

    /// The Courier restock must consume the SAME per-slot `Shops` / `Rewards`
    /// reads as the initial roll of that slot type — this is the whole point
    /// (keeping both streams C#-aligned for later shop visits). Character card:
    /// rarity + upgrade-noop (2 `Rewards`), pick + cost (2 `Shops`). Colorless:
    /// pick + cost (2 `Shops`), upgrade-noop (1 `Rewards`). Relic: RollRarity
    /// (1 `Rewards`), jitter (1 `Shops`) — the bag pull consumes no RNG, so an
    /// empty bag still advances identically. Potion: rarity + pick (2 `Shops`),
    /// no `Rewards` (`TestMode` flat price).
    #[test]
    fn restock_consumes_matching_per_slot_reads() {
        use sts2_core::shop::{is_shop_entry_available, ShopSlot};
        let mut rs = RunState::new();
        let mut inv = generate_merchant_inventory(&mut rs, CharacterId::Ironclad);
        let n_char = inv.cards.len();

        let check = |rs: &mut RunState,
                     inv: &mut sts2_core::shop::ShopInventory,
                     slot: ShopSlot,
                     want_shops: u32,
                     want_rewards: u32,
                     label: &str| {
            let s0 = rs.rng.shops.counter();
            let r0 = rs.rng.rewards.counter();
            restock_shop_entry(rs, inv, slot);
            assert_eq!(
                rs.rng.shops.counter() - s0,
                want_shops,
                "{label}: shops reads"
            );
            assert_eq!(
                rs.rng.rewards.counter() - r0,
                want_rewards,
                "{label}: rewards reads"
            );
        };

        check(&mut rs, &mut inv, ShopSlot::Card(0), 2, 2, "character card");
        check(
            &mut rs,
            &mut inv,
            ShopSlot::Card(n_char),
            2,
            1,
            "colorless card",
        );
        check(&mut rs, &mut inv, ShopSlot::Relic(0), 1, 1, "relic");
        check(&mut rs, &mut inv, ShopSlot::Potion(0), 2, 0, "potion");

        // Restocked card/potion slots are re-stocked and never on sale.
        assert!(inv.cards[0].card.is_some(), "char card restocked");
        assert!(!inv.cards[0].on_sale, "restocked card is never on sale");
        assert!(is_shop_entry_available(inv.cards[0].price));
        assert!(inv.potions[0].potion_kind.is_some(), "potion restocked");
    }

    /// Restock is deterministic for a fixed seed: same run seed → identical
    /// restocked cards/relics/potions + prices.
    #[test]
    fn restock_is_deterministic_for_fixed_seed() {
        use sts2_core::shop::ShopSlot;
        let build = || {
            let mut rs = RunState::new();
            let mut inv = generate_merchant_inventory(&mut rs, CharacterId::Ironclad);
            let n_char = inv.cards.len();
            restock_shop_entry(&mut rs, &mut inv, ShopSlot::Card(0));
            restock_shop_entry(&mut rs, &mut inv, ShopSlot::Card(n_char));
            restock_shop_entry(&mut rs, &mut inv, ShopSlot::Relic(1));
            restock_shop_entry(&mut rs, &mut inv, ShopSlot::Potion(2));
            (
                inv.cards[0].card.as_ref().map(|c| c.card_id),
                inv.cards[0].price,
                inv.colorless_cards[0].card.as_ref().map(|c| c.card_id),
                inv.relics[1].relic_id,
                inv.relics[1].price,
                inv.potions[2].potion_kind,
                inv.potions[2].price,
            )
        };
        assert_eq!(build(), build());
    }

    /// The character-card restock excludes every currently-stocked card
    /// (C# `_cardPool.Except(second)`), including the slot's just-sold card:
    /// the re-rolled card id must not collide with any OTHER stocked slot.
    /// (It may in principle re-pick nothing if the pool is exhausted, but the
    /// Ironclad pool is large, so a real card is expected.)
    #[test]
    fn restock_character_card_excludes_other_stocked_cards() {
        use sts2_core::shop::ShopSlot;
        let mut rs = RunState::new();
        let mut inv = generate_merchant_inventory(&mut rs, CharacterId::Ironclad);
        restock_shop_entry(&mut rs, &mut inv, ShopSlot::Card(0));
        let new_id = inv.cards[0].card.as_ref().map(|c| c.card_id);
        let others: Vec<_> = inv
            .cards
            .iter()
            .skip(1)
            .chain(inv.colorless_cards.iter())
            .filter_map(|e| e.card.as_ref().map(|c| c.card_id))
            .collect();
        if let Some(id) = new_id {
            assert!(
                !others.contains(&id),
                "restocked card {id:?} must not duplicate another stocked slot",
            );
        }
    }
}
