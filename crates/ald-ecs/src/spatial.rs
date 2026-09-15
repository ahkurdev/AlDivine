//! Spatial partitioning and interest management.
//!
//! Grid-based partition chosen over quadtree/BVH for the steady-state
//! multiplayer case: entity density is roughly uniform across the map and
//! queries are radius-based, so a fixed grid gives O(1) inserts and cache-
//! friendly linear scans per cell. The cell size is a compile-time constant
//! tuned for GTA-V-scale worlds; a hybrid region/quadtree layer can be added
//! later for uneven density without changing the query API.
//!
//! Interest management policy (per spec):
//!   near    -> high update rate
//!   medium  -> medium update rate
//!   far     -> low update rate
//!   out of range -> not replicated at all

use std::collections::HashMap;

/// Square cell size in world units. Larger = fewer cells, more entries per
/// scan; smaller = more cells, tighter queries.
const CELL: f32 = 64.0;

/// Interest radii (world units). Entities beyond FAR are not replicated.
pub const RADIUS_NEAR: f32 = 128.0;
pub const RADIUS_MEDIUM: f32 = 384.0;
pub const RADIUS_FAR: f32 = 1024.0;

/// Replication tier assigned by distance from the observing player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterestTier {
    /// Within RADIUS_NEAR: highest update rate.
    High,
    /// Within RADIUS_MEDIUM: medium update rate.
    Medium,
    /// Within RADIUS_FAR: low update rate.
    Low,
    /// Beyond RADIUS_FAR: not replicated.
    None,
}

impl InterestTier {
    /// Suggested update interval (seconds between full state refreshes).
    /// Smaller = more frequent. `None` means do not replicate.
    pub fn update_interval(self) -> Option<f32> {
        match self {
            InterestTier::High => Some(1.0 / 20.0),   // 20 Hz
            InterestTier::Medium => Some(1.0 / 10.0), // 10 Hz
            InterestTier::Low => Some(1.0 / 2.0),     // 2 Hz
            InterestTier::None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }

    /// Horizontal (XZ) distance — the plane GTA worlds are built on. Using
    /// 2D distance avoids penalizing entities at different heights.
    pub fn horizontal_distance(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

/// Cell key in the grid.
type CellKey = (i32, i32);

fn cell_of(pos: Vec3) -> CellKey {
    // Floor toward negative infinity so negative coordinates land in the
    // correct cell (f32::floor, not `as i32` truncation).
    ((pos.x / CELL).floor() as i32, (pos.y / CELL).floor() as i32)
}

/// Uniform-grid spatial index over entity positions.
#[derive(Debug, Default)]
pub struct SpatialGrid {
    /// Cell -> entity ids in that cell.
    cells: HashMap<CellKey, Vec<u64>>,
    /// Last known position per entity, so removal is O(1).
    positions: HashMap<u64, Vec3>,
}

impl SpatialGrid {
    pub fn new() -> Self {
        SpatialGrid::default()
    }

    /// Insert or move an entity to its current position.
    pub fn upsert(&mut self, entity: u64, pos: Vec3) {
        let key = cell_of(pos);
        if let Some(old) = self.positions.get(&entity) {
            let old_key = cell_of(*old);
            if old_key == key {
                self.positions.insert(entity, pos);
                return;
            }
            // Moved cells: remove from the old one.
            if let Some(cell) = self.cells.get_mut(&old_key) {
                cell.retain(|&e| e != entity);
                if cell.is_empty() {
                    self.cells.remove(&old_key);
                }
            }
        }
        self.cells.entry(key).or_default().push(entity);
        self.positions.insert(entity, pos);
    }

    /// Remove an entity from the index.
    pub fn remove(&mut self, entity: u64) {
        if let Some(pos) = self.positions.remove(&entity) {
            let key = cell_of(pos);
            if let Some(cell) = self.cells.get_mut(&key) {
                cell.retain(|&e| e != entity);
                if cell.is_empty() {
                    self.cells.remove(&key);
                }
            }
        }
    }

    /// All entities whose position is within `radius` (horizontal) of `center`.
    /// The scan covers exactly the cells overlapping the query circle.
    pub fn query_radius(&self, center: Vec3, radius: f32) -> Vec<u64> {
        let mut out = Vec::new();
        let min = cell_of(Vec3::new(center.x - radius, center.y - radius, center.z));
        let max = cell_of(Vec3::new(center.x + radius, center.y + radius, center.z));
        for cx in min.0..=max.0 {
            for cy in min.1..=max.1 {
                if let Some(cell) = self.cells.get(&(cx, cy)) {
                    for &e in cell {
                        if let Some(&pos) = self.positions.get(&e) {
                            if pos.horizontal_distance(center) <= radius {
                                out.push(e);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Number of indexed entities.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Position of an indexed entity, if known.
    pub fn position(&self, entity: u64) -> Option<Vec3> {
        self.positions.get(&entity).copied()
    }

    /// Number of occupied cells (diagnostic for tuning CELL).
    pub fn occupied_cells(&self) -> usize {
        self.cells.len()
    }
}

/// Assign the interest tier for an entity at `pos` relative to an observer
/// at `observer`. Distances use the horizontal plane.
pub fn interest_tier(observer: Vec3, pos: Vec3) -> InterestTier {
    let d = observer.horizontal_distance(pos);
    if d <= RADIUS_NEAR {
        InterestTier::High
    } else if d <= RADIUS_MEDIUM {
        InterestTier::Medium
    } else if d <= RADIUS_FAR {
        InterestTier::Low
    } else {
        InterestTier::None
    }
}

/// Compute the replication set for one observer: every indexed entity within
/// RADIUS_FAR, each tagged with its interest tier. Entities outside the far
/// radius are excluded entirely (not replicated), per the interest policy.
pub fn replication_set(grid: &SpatialGrid, observer: Vec3) -> Vec<(u64, InterestTier)> {
    grid.query_radius(observer, RADIUS_FAR)
        .into_iter()
        .filter_map(|e| {
            let pos = grid.position(e)?;
            // Exclude the observer's own entity body from its own set.
            if pos == observer {
                return None;
            }
            Some((e, interest_tier(observer, pos)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_query() {
        let mut g = SpatialGrid::new();
        g.upsert(1, Vec3::new(0.0, 0.0, 0.0));
        g.upsert(2, Vec3::new(10.0, 10.0, 0.0));
        g.upsert(3, Vec3::new(500.0, 500.0, 0.0));
        assert_eq!(g.len(), 3);
        let near = g.query_radius(Vec3::new(0.0, 0.0, 0.0), 50.0);
        assert!(near.contains(&1));
        assert!(near.contains(&2));
        assert!(!near.contains(&3));
    }

    #[test]
    fn negative_coordinates() {
        let mut g = SpatialGrid::new();
        g.upsert(9, Vec3::new(-70.0, -70.0, 0.0));
        let r = g.query_radius(Vec3::new(-70.0, -70.0, 0.0), 10.0);
        assert_eq!(r, vec![9]);
    }

    #[test]
    fn move_between_cells() {
        let mut g = SpatialGrid::new();
        g.upsert(1, Vec3::new(0.0, 0.0, 0.0));
        g.upsert(1, Vec3::new(1000.0, 1000.0, 0.0));
        let at_origin = g.query_radius(Vec3::new(0.0, 0.0, 0.0), 50.0);
        let at_dest = g.query_radius(Vec3::new(1000.0, 1000.0, 0.0), 50.0);
        assert!(at_origin.is_empty());
        assert_eq!(at_dest, vec![1]);
    }

    #[test]
    fn remove_cleans_cell() {
        let mut g = SpatialGrid::new();
        g.upsert(1, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(g.occupied_cells(), 1);
        g.remove(1);
        assert!(g.is_empty());
        assert_eq!(g.occupied_cells(), 0);
    }

    #[test]
    fn interest_tiers_by_distance() {
        let o = Vec3::new(0.0, 0.0, 0.0);
        assert_eq!(interest_tier(o, Vec3::new(10.0, 0.0, 0.0)), InterestTier::High);
        assert_eq!(interest_tier(o, Vec3::new(200.0, 0.0, 0.0)), InterestTier::Medium);
        assert_eq!(interest_tier(o, Vec3::new(800.0, 0.0, 0.0)), InterestTier::Low);
        assert_eq!(interest_tier(o, Vec3::new(2000.0, 0.0, 0.0)), InterestTier::None);
    }

    #[test]
    fn update_intervals_monotonic() {
        let high = InterestTier::High.update_interval().unwrap();
        let med = InterestTier::Medium.update_interval().unwrap();
        let low = InterestTier::Low.update_interval().unwrap();
        assert!(high < med && med < low);
        assert_eq!(InterestTier::None.update_interval(), None);
    }

    #[test]
    fn replication_set_excludes_far_and_self() {
        let mut g = SpatialGrid::new();
        g.upsert(1, Vec3::new(0.0, 0.0, 0.0)); // observer body
        g.upsert(2, Vec3::new(50.0, 0.0, 0.0));
        g.upsert(3, Vec3::new(300.0, 0.0, 0.0));
        g.upsert(4, Vec3::new(3000.0, 0.0, 0.0)); // beyond far
        let set = replication_set(&g, Vec3::new(0.0, 0.0, 0.0));
        let ids: Vec<u64> = set.iter().map(|(e, _)| *e).collect();
        assert!(!ids.contains(&1)); // self excluded
        assert!(ids.contains(&2));
        assert!(ids.contains(&3));
        assert!(!ids.contains(&4)); // beyond far radius
                                    // Tier assignment within the set.
        let tier_of = |id: u64| set.iter().find(|(e, _)| *e == id).map(|(_, t)| *t).unwrap();
        assert_eq!(tier_of(2), InterestTier::High);
        assert_eq!(tier_of(3), InterestTier::Medium);
    }

    #[test]
    fn horizontal_distance_ignores_height() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(3.0, 4.0, 999.0);
        assert_eq!(a.horizontal_distance(b), 5.0);
    }
}
