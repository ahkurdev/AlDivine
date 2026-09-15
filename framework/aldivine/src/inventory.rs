//! Aldivine Framework — inventory service.
//!
//! Server-authoritative item storage. Items are never created by clients:
//! every item enters the world through an explicit server-side grant, and
//! transfers are validated (weight, stack limits, container depth) before
//! they commit.

use std::collections::HashMap;

use ald_core::AldivinePlayerId;

/// Item metadata. Weight in grams; durability 0..=100 (100 = pristine).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Stable item definition id, e.g. "ald:bandage".
    pub def_id: String,
    pub name: String,
    pub weight: u32,
    pub stackable: bool,
    pub max_stack: u32,
    /// Whether instances of this item carry a durability value.
    pub has_durability: bool,
}

/// One stack inside an inventory. Weight is snapshotted from the definition
/// at creation time so total_weight never depends on a later registry edit.
#[derive(Debug, Clone)]
pub struct ItemStack {
    pub def_id: String,
    pub count: u32,
    pub weight_per_unit: u32,
    pub durability: Option<u8>,
}

impl ItemStack {
    fn new(def: &Item, count: u32) -> Self {
        ItemStack {
            def_id: def.def_id.clone(),
            count,
            weight_per_unit: def.weight,
            durability: if def.has_durability { Some(100) } else { None },
        }
    }

    pub fn total_weight(&self) -> u32 {
        self.weight_per_unit * self.count
    }
}

/// Where an inventory lives. Limits container nesting depth explicitly so a
/// malicious resource cannot build a recursion bomb.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StorageKind {
    Player(AldivinePlayerId),
    Vehicle(u64),
    Property(u64),
    /// Custom storage provider registered by a resource (e.g. a backpack).
    Custom(String),
}

#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    #[error("item definition '{0}' is not registered")]
    UnknownItem(String),
    #[error("weight limit exceeded: adding {adding}g would total {total}g over {limit}g")]
    OverWeight { adding: u32, total: u32, limit: u32 },
    #[error("stack limit exceeded for '{item}'")]
    OverStack { item: String },
    #[error("container nesting depth limit reached")]
    NestingTooDeep,
    #[error("not enough '{item}' to remove {want}")]
    Insufficient { item: String, want: u32 },
}

/// Maximum container nesting depth (backpack-in-backpack).
pub const MAX_NESTING: usize = 3;

#[derive(Debug, Clone)]
pub struct Inventory {
    pub kind: StorageKind,
    pub weight_limit: u32,
    pub slots: Vec<ItemStack>,
    /// Nesting level of this inventory (0 = player/vehicle/property root).
    pub depth: usize,
}

impl Inventory {
    pub fn total_weight(&self) -> u32 {
        self.slots.iter().map(|s| s.total_weight()).sum()
    }
}

/// Inventory service: owns all storages and the item registry.
#[derive(Debug, Default)]
pub struct InventoryService {
    registry: HashMap<String, Item>,
    storages: HashMap<StorageKind, Inventory>,
}

impl InventoryService {
    pub fn new() -> Self {
        InventoryService::default()
    }

    /// Register an item definition. Required before any item of that id exists.
    pub fn register(&mut self, item: Item) {
        self.registry.insert(item.def_id.clone(), item);
    }

    /// Fetch a storage, creating an empty one on first access.
    pub fn storage(&mut self, kind: StorageKind, weight_limit: u32) -> &mut Inventory {
        let depth = self.storages.get(&kind).map(|i| i.depth).unwrap_or(0);
        self.storages.entry(kind.clone()).or_insert(Inventory { kind, weight_limit, slots: Vec::new(), depth })
    }

    /// Authoritative item grant. Server-side only.
    pub fn give(
        &mut self,
        storage: StorageKind,
        weight_limit: u32,
        def_id: &str,
        count: u32,
    ) -> Result<(), InventoryError> {
        let item = self.registry.get(def_id).ok_or_else(|| InventoryError::UnknownItem(def_id.to_string()))?.clone();

        let inv = self.storage(storage, weight_limit);
        let adding_weight = item.weight * count;
        let current = inv.total_weight();
        if current + adding_weight > inv.weight_limit {
            return Err(InventoryError::OverWeight {
                adding: adding_weight,
                total: current + adding_weight,
                limit: inv.weight_limit,
            });
        }

        if item.stackable {
            if let Some(stack) = inv.slots.iter_mut().find(|s| s.def_id == def_id) {
                let new_count = stack.count + count;
                if new_count > item.max_stack {
                    return Err(InventoryError::OverStack { item: def_id.to_string() });
                }
                stack.count = new_count;
                return Ok(());
            }
        }
        if count > item.max_stack {
            return Err(InventoryError::OverStack { item: def_id.to_string() });
        }
        inv.slots.push(ItemStack::new(&item, count));
        Ok(())
    }

    /// Remove items from a storage. Fails rather than silently clamping to zero.
    pub fn take(
        &mut self,
        storage: StorageKind,
        weight_limit: u32,
        def_id: &str,
        count: u32,
    ) -> Result<(), InventoryError> {
        let inv = self.storage(storage, weight_limit);
        let stack = inv
            .slots
            .iter_mut()
            .find(|s| s.def_id == def_id)
            .ok_or_else(|| InventoryError::Insufficient { item: def_id.to_string(), want: count })?;
        if stack.count < count {
            return Err(InventoryError::Insufficient { item: def_id.to_string(), want: count });
        }
        stack.count -= count;
        if stack.count == 0 {
            inv.slots.retain(|s| s.count > 0);
        }
        Ok(())
    }

    /// Move items between storages. Weight is checked at the destination.
    /// A rejected destination rolls the source back, so the operation is
    /// never observed half-applied.
    pub fn transfer(
        &mut self,
        from: StorageKind,
        to: StorageKind,
        weight_limit: u32,
        def_id: &str,
        count: u32,
    ) -> Result<(), InventoryError> {
        // Depth check first: nesting beyond MAX_NESTING is rejected.
        let to_depth = self.storage(to.clone(), weight_limit).depth;
        if to_depth >= MAX_NESTING {
            return Err(InventoryError::NestingTooDeep);
        }
        self.take(from.clone(), weight_limit, def_id, count)?;
        self.give(to.clone(), weight_limit, def_id, count).inspect_err(|_e| {
            // Roll the source back so a failed give is not an item dupe.
            let _ = self.give(from, weight_limit, def_id, count);
        })
    }

    pub fn count(&self, storage: &StorageKind, def_id: &str) -> u32 {
        self.storages
            .get(storage)
            .and_then(|inv| inv.slots.iter().find(|s| s.def_id == def_id))
            .map(|s| s.count)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bandage() -> Item {
        Item {
            def_id: "ald:bandage".into(),
            name: "Bandage".into(),
            weight: 100,
            stackable: true,
            max_stack: 10,
            has_durability: false,
        }
    }

    fn rifle() -> Item {
        Item {
            def_id: "ald:rifle".into(),
            name: "Rifle".into(),
            weight: 3500,
            stackable: false,
            max_stack: 1,
            has_durability: true,
        }
    }

    fn water() -> Item {
        Item {
            def_id: "ald:water".into(),
            name: "Water".into(),
            weight: 500,
            stackable: true,
            max_stack: 5,
            has_durability: false,
        }
    }

    #[test]
    fn give_and_count() {
        let mut s = InventoryService::new();
        s.register(bandage());
        let p = StorageKind::Player(AldivinePlayerId::new());
        s.give(p.clone(), 100_000, "ald:bandage", 5).unwrap();
        assert_eq!(s.count(&p, "ald:bandage"), 5);
    }

    #[test]
    fn stacking_respects_max() {
        let mut s = InventoryService::new();
        s.register(bandage());
        let p = StorageKind::Player(AldivinePlayerId::new());
        s.give(p.clone(), 1_000_000, "ald:bandage", 10).unwrap();
        assert!(matches!(s.give(p, 1_000_000, "ald:bandage", 1), Err(InventoryError::OverStack { .. })));
    }

    #[test]
    fn weight_limit_enforced() {
        let mut s = InventoryService::new();
        s.register(rifle());
        let p = StorageKind::Player(AldivinePlayerId::new());
        let err = s.give(p, 3000, "ald:rifle", 1).unwrap_err();
        assert!(matches!(err, InventoryError::OverWeight { .. }));
    }

    #[test]
    fn mixed_items_weight_accumulates() {
        // A rifle (3500g) already in the bag leaves room for one water (500g)
        // but not three (1500g) under a 4200g limit.
        let mut s = InventoryService::new();
        s.register(rifle());
        s.register(water());
        let p = StorageKind::Player(AldivinePlayerId::new());
        s.give(p.clone(), 4200, "ald:rifle", 1).unwrap();
        assert_eq!(s.count(&p, "ald:rifle"), 1);
        s.give(p.clone(), 4200, "ald:water", 1).unwrap();
        assert!(s.give(p, 4200, "ald:water", 3).is_err());
    }

    #[test]
    fn take_refuses_insufficient() {
        let mut s = InventoryService::new();
        s.register(bandage());
        let p = StorageKind::Player(AldivinePlayerId::new());
        s.give(p.clone(), 100_000, "ald:bandage", 2).unwrap();
        assert!(s.take(p.clone(), 100_000, "ald:bandage", 5).is_err());
        assert_eq!(s.count(&p, "ald:bandage"), 2);
    }

    #[test]
    fn transfer_between_storages() {
        let mut s = InventoryService::new();
        s.register(bandage());
        let a = StorageKind::Player(AldivinePlayerId::new());
        let v = StorageKind::Vehicle(42);
        s.give(a.clone(), 100_000, "ald:bandage", 4).unwrap();
        s.transfer(a.clone(), v.clone(), 100_000, "ald:bandage", 3).unwrap();
        assert_eq!(s.count(&a, "ald:bandage"), 1);
        assert_eq!(s.count(&v, "ald:bandage"), 3);
    }

    #[test]
    fn transfer_rolls_back_on_failure() {
        let mut s = InventoryService::new();
        s.register(bandage());
        let a = StorageKind::Player(AldivinePlayerId::new());
        let v = StorageKind::Vehicle(42);
        s.give(a.clone(), 100_000, "ald:bandage", 4).unwrap();
        // Destination weight limit too small for the transfer.
        let err = s.transfer(a.clone(), v.clone(), 100, "ald:bandage", 4).unwrap_err();
        assert!(matches!(err, InventoryError::OverWeight { .. }));
        // Source items must still be there — no dupe, no loss.
        assert_eq!(s.count(&a, "ald:bandage"), 4);
        assert_eq!(s.count(&v, "ald:bandage"), 0);
    }

    #[test]
    fn unknown_item_rejected() {
        let mut s = InventoryService::new();
        let p = StorageKind::Player(AldivinePlayerId::new());
        assert!(matches!(s.give(p, 100_000, "ald:nonexistent", 1), Err(InventoryError::UnknownItem(_))));
    }

    #[test]
    fn durability_only_when_declared() {
        let mut s = InventoryService::new();
        s.register(rifle());
        s.register(bandage());
        let p = StorageKind::Player(AldivinePlayerId::new());
        s.give(p.clone(), 1_000_000, "ald:rifle", 1).unwrap();
        s.give(p.clone(), 1_000_000, "ald:bandage", 1).unwrap();
        let inv = s.storage(p, 1_000_000);
        let rifle_stack = inv.slots.iter().find(|x| x.def_id == "ald:rifle").unwrap();
        let bandage_stack = inv.slots.iter().find(|x| x.def_id == "ald:bandage").unwrap();
        assert_eq!(rifle_stack.durability, Some(100));
        assert_eq!(bandage_stack.durability, None);
    }
}
