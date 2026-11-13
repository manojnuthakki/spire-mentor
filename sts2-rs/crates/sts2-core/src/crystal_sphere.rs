//! The Crystal Sphere minigame (C# `CrystalSphereMinigame`, `CrystalSphereItem`
//! and its five item kinds), sts2-agent fork, lane F's F8 (`crystal-sphere`).
//!
//! The Ancient-less event opens an 11x11 grid of hidden cells: the game clears
//! the four corners' L-shapes (24 cells), then places 15 items by
//! `Rng.NextItem` over every top-left cell the item fits on (x-major scan, no
//! cleared or occupied cell), in a fixed order. The player spends divinations
//! (3 for Uncover Future, 6 for Payment Plan) with the Big tool (the clicked
//! cell and its eight neighbours) or the Small tool (that cell); an item is
//! revealed once all its cells are clear, in the order the cells were cleared.
//! The revealed items become the minigame's rewards when the last divination
//! is spent (`sts2-content`'s `crystal_sphere` module rolls them off the same
//! RNG, as `OneOffSynchronizer.OfferCrystalSphereRewards` does).
//!
//! This module is the grid and its rules only: no rewards and no run state.
//! A player sees the grid size, the cleared cells and the items with a cleared
//! cell (kind, footprint, revealed), never the items under the fog: callers
//! must not expose `items()` of an unrevealed item beyond that
//! ([`CrystalSphere::visible_items`]).

use crate::DotNetRng as Rng;

/// Grid width and height (C# `_defaultWidth` / `_defaultHeight`).
pub const GRID: usize = 11;

/// Which tool the next click uses (C# `CrystalSphereToolType`; the game's
/// `None` is never selectable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// One cell.
    Small,
    /// The cell and its eight neighbours.
    Big,
}

/// A card reward's rarity (C# `CardRarity` for the three the sphere places).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SphereCardRarity {
    Common,
    Uncommon,
    Rare,
}

/// The five item kinds (C# `CrystalSphereRelic`, `...Potion(rarity)`,
/// `...CardReward(rarity)`, `...Curse`, `...Gold(isBig)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Relic,
    /// Potion by rarity: `rare` false is Common (1x3), true is Rare (2x2).
    Potion {
        rare: bool,
    },
    CardReward(SphereCardRarity),
    Curse,
    /// `big` false is 10 gold (1x1), true is 30 (2x1).
    Gold {
        big: bool,
    },
}

impl ItemKind {
    /// `(width, height)` in cells (C# `Size`).
    #[must_use]
    pub fn size(self) -> (usize, usize) {
        match self {
            Self::Relic => (4, 4),
            Self::Potion { rare: false } => (1, 3),
            Self::Potion { rare: true } | Self::CardReward(_) | Self::Curse => (2, 2),
            Self::Gold { big: false } => (1, 1),
            Self::Gold { big: true } => (2, 1),
        }
    }

    /// The agent-facing kind name (`card_reward`, `curse`, `gold`, `potion`, `relic`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Relic => "relic",
            Self::Potion { .. } => "potion",
            Self::CardReward(_) => "card_reward",
            Self::Curse => "curse",
            Self::Gold { .. } => "gold",
        }
    }
}

/// One placed item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    /// Top-left cell.
    pub x: usize,
    pub y: usize,
    /// False when `place` found no room (C# `PlaceItem` returned false): the
    /// item is in the list but on no cell.
    pub placed: bool,
}

/// Why a divination was refused (the game's screen ignores a click on a
/// cleared cell, and the grid has no other cells).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DivineError {
    OffGrid,
    Cleared,
    Finished,
}

/// The minigame (C# `CrystalSphereMinigame`).
#[derive(Clone, Debug, PartialEq)]
pub struct CrystalSphere {
    /// `hidden[x][y]`.
    hidden: [[bool; GRID]; GRID],
    /// `item_at[x][y]`: the index into `items` occupying the cell.
    item_at: [[Option<usize>; GRID]; GRID],
    items: Vec<Item>,
    /// Item indices in the order they were revealed (C# `_revealed`).
    revealed: Vec<usize>,
    divinations_left: u32,
    tool: Tool,
    /// The event's RNG, carried on into the rewards roll.
    pub rng: Rng,
    placed_all: bool,
}

impl CrystalSphere {
    /// Build the grid (C# constructor): clear the corners' L-shapes, place the
    /// items (retrying up to ten times as the game does), select the Big tool.
    /// `rng` is the event's RNG after its `CalculateVars` draw.
    #[must_use]
    pub fn new(rng: Rng, divinations: u32) -> Self {
        let mut s = Self {
            hidden: [[true; GRID]; GRID],
            item_at: [[None; GRID]; GRID],
            items: Vec::new(),
            revealed: Vec::new(),
            divinations_left: 0,
            tool: Tool::Big,
            rng,
            placed_all: false,
        };
        // The four corners, then twice: add each cell's horizontal and vertical
        // neighbours (duplicates and all: clearing is idempotent).
        let far = GRID - 1;
        let mut cells: Vec<(usize, usize)> = vec![(0, 0), (far, 0), (far, far), (0, far)];
        for _ in 0..2 {
            let mut next = cells.clone();
            next.extend(cells.iter().flat_map(|&(x, y)| horizontal(x, y)));
            next.extend(cells.iter().flat_map(|&(x, y)| vertical(x, y)));
            cells = next;
        }
        for (x, y) in cells {
            s.hidden[x][y] = false;
        }
        let mut tries = 0;
        loop {
            s.placed_all = s.populate_items();
            tries += 1;
            if s.placed_all || tries >= 10 {
                break;
            }
        }
        s.divinations_left = divinations;
        s
    }

    /// C# `PopulateItems`: the placement order, `flag &&` short-circuiting the
    /// placement (not the list) once one item found no room.
    fn populate_items(&mut self) -> bool {
        let order = [
            ItemKind::Relic,
            ItemKind::Potion { rare: false },
            ItemKind::Potion { rare: false },
            ItemKind::Potion { rare: true },
            ItemKind::CardReward(SphereCardRarity::Common),
            ItemKind::CardReward(SphereCardRarity::Uncommon),
            ItemKind::CardReward(SphereCardRarity::Rare),
            ItemKind::Curse,
            ItemKind::Gold { big: false },
            ItemKind::Gold { big: false },
            ItemKind::Gold { big: false },
            ItemKind::Gold { big: false },
            ItemKind::Gold { big: false },
            ItemKind::Gold { big: true },
            ItemKind::Gold { big: true },
        ];
        let mut flag = true;
        for kind in order {
            let idx = self.items.len();
            let mut item = Item {
                kind,
                x: 0,
                y: 0,
                placed: false,
            };
            if flag {
                flag = self.place(idx, &mut item);
            }
            self.items.push(item);
        }
        flag
    }

    /// C# `CrystalSphereItem.PlaceItem`: every top-left cell the item fits on
    /// (x outer, y inner), one `NextItem` over them.
    fn place(&mut self, idx: usize, item: &mut Item) -> bool {
        let (w, h) = item.kind.size();
        let mut spots: Vec<(usize, usize)> = Vec::new();
        for x in 0..GRID {
            for y in 0..GRID {
                if self.fits(x, y, w, h) {
                    spots.push((x, y));
                }
            }
        }
        if spots.is_empty() {
            return false;
        }
        let at = self.rng.next_int_exclusive(0, spots.len() as i32) as usize;
        let (px, py) = spots[at];
        item.x = px;
        item.y = py;
        item.placed = true;
        for dx in 0..w {
            for dy in 0..h {
                self.item_at[px + dx][py + dy] = Some(idx);
            }
        }
        true
    }

    fn fits(&self, x: usize, y: usize, w: usize, h: usize) -> bool {
        for dx in 0..w {
            for dy in 0..h {
                let (cx, cy) = (x + dx, y + dy);
                if cx >= GRID
                    || cy >= GRID
                    || !self.hidden[cx][cy]
                    || self.item_at[cx][cy].is_some()
                {
                    return false;
                }
            }
        }
        true
    }

    /// Divinations left (the minigame is finished at 0).
    #[must_use]
    pub fn divinations_left(&self) -> u32 {
        self.divinations_left
    }

    /// True once the last divination is spent (C# `IsFinished`).
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.divinations_left == 0
    }

    /// The selected tool.
    #[must_use]
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Whether the cell is still under the fog (false off the grid).
    #[must_use]
    pub fn is_hidden(&self, x: i32, y: i32) -> bool {
        (0..GRID as i32).contains(&x)
            && (0..GRID as i32).contains(&y)
            && self.hidden[x as usize][y as usize]
    }

    /// The cleared cells, x-major as `(x, y)`... listed row by row (y outer).
    #[must_use]
    pub fn cleared_cells(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..GRID {
            for x in 0..GRID {
                if !self.hidden[x][y] {
                    out.push((x, y));
                }
            }
        }
        out
    }

    /// Every item, placed or not, in the game's generation order. The
    /// engine's own view: a player must go through [`Self::visible_items`].
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The item indices revealed so far, in the order they were revealed.
    #[must_use]
    pub fn revealed(&self) -> &[usize] {
        &self.revealed
    }

    /// The items a player sees: those with at least one cleared cell, as
    /// `(index, revealed)`, in the game's generation order.
    #[must_use]
    pub fn visible_items(&self) -> Vec<(usize, bool)> {
        let mut out = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            if !item.placed {
                continue;
            }
            let (w, h) = item.kind.size();
            let (mut seen, mut total) = (0, 0);
            for dx in 0..w {
                for dy in 0..h {
                    total += 1;
                    if !self.hidden[item.x + dx][item.y + dy] {
                        seen += 1;
                    }
                }
            }
            if seen > 0 {
                out.push((i, seen == total));
            }
        }
        out
    }

    /// The item's card-reward banner shows its rarity once any cell of its top
    /// row is clear (the banner lies in the card's top half).
    #[must_use]
    pub fn banner_shown(&self, idx: usize) -> bool {
        let item = &self.items[idx];
        let (w, _) = item.kind.size();
        item.placed && (0..w).any(|dx| !self.hidden[item.x + dx][item.y])
    }

    /// C# `SetTool`: free on the screen, so it rides along with a click.
    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
    }

    /// C# `CellClicked` with the tool set first: one divination spent, the cell
    /// (Small) or the cell and its neighbours (Big, in `GetAdjacentCells`
    /// order) cleared. Returns the items this click revealed, in order.
    ///
    /// # Errors
    /// [`DivineError::OffGrid`] for a cell outside the grid,
    /// [`DivineError::Cleared`] for a cell already clear (the screen ignores
    /// the click), [`DivineError::Finished`] with no divination left.
    pub fn divine(&mut self, x: i32, y: i32, tool: Tool) -> Result<Vec<usize>, DivineError> {
        if self.is_finished() {
            return Err(DivineError::Finished);
        }
        if !(0..GRID as i32).contains(&x) || !(0..GRID as i32).contains(&y) {
            return Err(DivineError::OffGrid);
        }
        let (x, y) = (x as usize, y as usize);
        if !self.hidden[x][y] {
            return Err(DivineError::Cleared);
        }
        self.tool = tool;
        self.divinations_left -= 1;
        let cells = match tool {
            Tool::Small => vec![(x, y)],
            Tool::Big => adjacent(x, y),
        };
        let mut newly = Vec::new();
        for (cx, cy) in cells {
            if !self.hidden[cx][cy] {
                continue;
            }
            self.hidden[cx][cy] = false;
            if let Some(i) = self.item_at[cx][cy] {
                if self.all_clear(i) {
                    self.revealed.push(i);
                    newly.push(i);
                }
            }
        }
        Ok(newly)
    }

    fn all_clear(&self, idx: usize) -> bool {
        let item = &self.items[idx];
        let (w, h) = item.kind.size();
        (0..w).all(|dx| (0..h).all(|dy| !self.hidden[item.x + dx][item.y + dy]))
    }
}

fn horizontal(x: usize, y: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if x >= 1 {
        out.push((x - 1, y));
    }
    if x + 1 < GRID {
        out.push((x + 1, y));
    }
    out
}

fn vertical(x: usize, y: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if y >= 1 {
        out.push((x, y - 1));
    }
    if y + 1 < GRID {
        out.push((x, y + 1));
    }
    out
}

fn diagonal(x: usize, y: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for dx in [-1i32, 1] {
        for dy in [-1i32, 1] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if (0..GRID as i32).contains(&nx) && (0..GRID as i32).contains(&ny) {
                out.push((nx as usize, ny as usize));
            }
        }
    }
    out
}

/// C# `GetAdjacentCells`: horizontal, vertical, diagonal neighbours, then the cell.
fn adjacent(x: usize, y: usize) -> Vec<(usize, usize)> {
    let mut out = horizontal(x, y);
    out.extend(vertical(x, y));
    out.extend(diagonal(x, y));
    out.push((x, y));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host's recording (seed 3, `crystal_sphere_payment_plan` /
    /// `crystal_sphere_uncover_future`): the event's `CalculateVars` draw
    /// (`50 + NextInt(1, 50)` = 53), then the placement of the 15 items.
    fn seed3() -> CrystalSphere {
        // The recording's seed is the string "3": the run seed is its hash.
        let seed = crate::deterministic_hash_code(&crate::canonicalize_seed("3"));
        let mut rng = Rng::with_name(seed, "CRYSTAL_SPHERE");
        assert_eq!(50 + rng.next_int_exclusive(1, 50), 53);
        CrystalSphere::new(rng, 6)
    }

    fn at(s: &CrystalSphere, kind: ItemKind) -> Vec<(usize, usize)> {
        s.items()
            .iter()
            .filter(|i| i.kind == kind)
            .map(|i| (i.x, i.y))
            .collect()
    }

    #[test]
    fn placement_matches_the_games_recording_for_seed_3() {
        let s = seed3();
        assert!(s.placed_all);
        assert_eq!(s.items().len(), 15);
        assert_eq!(at(&s, ItemKind::Relic), vec![(7, 3)]);
        assert_eq!(at(&s, ItemKind::Curse), vec![(4, 6)]);
        assert_eq!(
            at(&s, ItemKind::CardReward(SphereCardRarity::Rare)),
            vec![(3, 4)]
        );
        assert_eq!(at(&s, ItemKind::Potion { rare: true }), vec![(1, 5)]);
        let mut potions = at(&s, ItemKind::Potion { rare: false });
        potions.sort_unstable();
        assert_eq!(potions, vec![(5, 0), (6, 4)]);
        assert!(at(&s, ItemKind::Gold { big: false }).contains(&(4, 3)));
        assert!(
            at(&s, ItemKind::CardReward(SphereCardRarity::Common)).contains(&(1, 3))
                || at(&s, ItemKind::CardReward(SphereCardRarity::Uncommon)).contains(&(1, 3))
        );
    }

    #[test]
    fn the_corners_start_clear_and_no_item_sits_on_them() {
        let s = seed3();
        assert_eq!(s.cleared_cells().len(), 24);
        for (x, y) in s.cleared_cells() {
            assert!(s.item_at[x][y].is_none());
        }
        assert!(!s.is_hidden(0, 0) && !s.is_hidden(2, 0) && !s.is_hidden(10, 10));
        assert!(s.is_hidden(5, 5));
    }

    #[test]
    fn big_clears_three_by_three_and_reveals_a_whole_item_in_the_order_cleared() {
        let mut s = seed3();
        // The recording: Big at (5,5) clears 9 cells; the 1x3 potion at (6,4)
        // is wholly inside (revealed), the card and curse only partly.
        let revealed = s.divine(5, 5, Tool::Big).unwrap();
        assert_eq!(s.cleared_cells().len(), 33);
        let kinds: Vec<_> = revealed.iter().map(|&i| s.items()[i].kind).collect();
        assert_eq!(kinds, vec![ItemKind::Potion { rare: false }]);
        let visible = s.visible_items();
        assert_eq!(visible.len(), 3);
        assert!(s.banner_shown(
            s.items()
                .iter()
                .position(|i| i.kind == ItemKind::CardReward(SphereCardRarity::Rare))
                .unwrap()
        ));
        assert_eq!(s.divinations_left(), 5);
    }

    #[test]
    fn a_cleared_cell_or_the_edge_is_refused_and_the_last_divination_finishes() {
        let mut s = seed3();
        assert_eq!(s.divine(0, 0, Tool::Small), Err(DivineError::Cleared));
        assert_eq!(s.divine(11, 5, Tool::Big), Err(DivineError::OffGrid));
        assert_eq!(s.divine(3, -1, Tool::Big), Err(DivineError::OffGrid));
        assert_eq!(s.divinations_left(), 6, "a refusal spends nothing");
        for (x, y) in [(5, 5), (3, 3), (2, 5), (8, 5), (5, 2), (5, 8)] {
            s.divine(x, y, Tool::Big).unwrap_or_default();
        }
        assert!(s.is_finished());
        assert_eq!(s.divine(5, 5, Tool::Big), Err(DivineError::Finished));
    }
}
