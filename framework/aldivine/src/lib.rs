//! Aldivine Framework — the primary native gameplay framework.
//!
//! Design rule: framework state is owned by services, not by script-visible
//! global tables. A resource talks to `PlayerService`, `EconomyService`,
//! `InventoryService`, `JobService` through capability-checked handles; it
//! never gets a `&mut` to framework state directly. This module defines the
//! service surfaces; the script bindings layer exposes them to Lua/JS.
//!
//! Modules are independent and own their state, so the framework can be
//! embedded in a test binary without a database or network stack.

pub mod economy;
pub mod inventory;
pub mod jobs;
pub mod players;

pub use economy::{AccountKind, EconomyError, EconomyService, Transaction};
pub use inventory::{Inventory, InventoryError, InventoryService, Item, ItemStack, StorageKind, MAX_NESTING};
pub use jobs::{DutyState, Employment, Grade, Job, JobError, JobService};
pub use players::{Character, CharacterId, Player, PlayerService, PlayerStatus};

pub const FRAMEWORK_VERSION: &str = "0.1.0";
