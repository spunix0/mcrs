//! The player's inventory as the server reports it, and the item stack wire format.
//!
//! Item stacks end with a component patch that has no length prefix, so we can only skip components we know how
//! to decode. The common ones (damage, names, lore, enchantments, ...) are handled; on anything else the stack
//! itself is still known, but the rest of that packet is dropped.

use mc_protocol::Reader;
use std::collections::HashMap;

/// Player inventory menu layout (container 0): 0 craft result, 1-4 craft grid, 5-8 armor (head..feet),
/// 9-35 main, 36-44 hotbar, 45 offhand.
pub const SLOTS: usize = 46;
pub const HOTBAR_START: usize = 36;

#[derive(Clone, Debug, PartialEq)]
pub struct Stack {
    pub item: u32,
    pub count: u32,
    pub damage: Option<u32>,
    pub max_damage: Option<u32>,
    pub name: Option<String>,
    pub enchanted: bool,
    /// (enchantment registry ID, level).
    pub enchantments: Vec<(u32, u32)>,
}

#[derive(Debug)]
pub struct UnknownComponent(pub u32);

/// Component type IDs we know how to skip or read, looked up by name from the registry report.
pub struct Components {
    ids: HashMap<u32, Kind>,
}

#[derive(Clone, Copy)]
enum Kind {
    Damage,
    MaxDamage,
    VarInt,
    Unit,
    Bool,
    Int,
    Text,
    CustomName,
    Lore,
    Enchantments,
    Nbt,
}

impl Components {
    pub fn new(by_name: &HashMap<String, u32>) -> Self {
        let table = [
            ("damage", Kind::Damage),
            ("max_damage", Kind::MaxDamage),
            ("max_stack_size", Kind::VarInt),
            ("repair_cost", Kind::VarInt),
            ("map_id", Kind::VarInt),
            ("rarity", Kind::VarInt),
            ("unbreakable", Kind::Unit),
            ("glider", Kind::Unit),
            ("enchantment_glint_override", Kind::Bool),
            ("dyed_color", Kind::Int),
            ("item_name", Kind::Text),
            ("custom_name", Kind::CustomName),
            ("lore", Kind::Lore),
            ("enchantments", Kind::Enchantments),
            ("stored_enchantments", Kind::Enchantments),
            ("custom_data", Kind::Nbt),
        ];
        let ids = table.iter().filter_map(|(n, k)| Some((*by_name.get(*n)?, *k))).collect();
        Self { ids }
    }
}

type Res<T> = Result<T, UnknownComponent>;

fn io<T>(r: std::io::Result<T>) -> Res<T> {
    r.map_err(|_| UnknownComponent(u32::MAX))
}

/// Reads `ItemStack.OPTIONAL_STREAM_CODEC`. On an unknown component, returns what it got so far alongside the error.
pub fn read_stack(r: &mut Reader, comps: &Components) -> Result<Option<Stack>, (Option<Stack>, UnknownComponent)> {
    let count = io(r.varint()).map_err(|e| (None, e))?;
    if count <= 0 {
        return Ok(None);
    }
    let item = io(r.varint()).map_err(|e| (None, e))? as u32;
    let mut stack = Stack { item, count: count as u32, damage: None, max_damage: None, name: None, enchanted: false, enchantments: Vec::new() };
    match read_patch(r, comps, &mut stack) {
        Ok(()) => Ok(Some(stack)),
        Err(e) => Err((Some(stack), e)),
    }
}

fn read_patch(r: &mut Reader, comps: &Components, s: &mut Stack) -> Res<()> {
    let added = io(r.varint())?;
    let removed = io(r.varint())?;
    for _ in 0..added {
        let id = io(r.varint())? as u32;
        let kind = *comps.ids.get(&id).ok_or(UnknownComponent(id))?;
        match kind {
            Kind::Damage => s.damage = Some(io(r.varint())? as u32),
            Kind::MaxDamage => s.max_damage = Some(io(r.varint())? as u32),
            Kind::VarInt => {
                io(r.varint())?;
            }
            Kind::Unit => {}
            Kind::Bool => s.enchanted |= io(r.bool())?,
            Kind::Int => {
                io(r.i32())?;
            }
            Kind::Text | Kind::Nbt => {
                io(r.nbt())?;
            }
            Kind::CustomName => {
                if let Some(tag) = io(r.nbt())? {
                    s.name = Some(mc_protocol::text::plain(&tag));
                }
            }
            Kind::Lore => {
                for _ in 0..io(r.len())? {
                    io(r.nbt())?;
                }
            }
            Kind::Enchantments => {
                let n = io(r.len())?;
                for _ in 0..n {
                    let (id, level) = (io(r.varint())? as u32, io(r.varint())? as u32);
                    s.enchantments.push((id, level));
                }
                s.enchanted |= n > 0;
            }
        }
    }
    // Removed components are just their type IDs.
    for _ in 0..removed {
        io(r.varint())?;
    }
    Ok(())
}

pub struct Inventory {
    pub slots: Vec<Option<Stack>>,
    /// Echoed in clicks so the server can tell whether we saw its latest state.
    pub state_id: i32,
}

impl Default for Inventory {
    fn default() -> Self {
        Self { slots: vec![None; SLOTS], state_id: 0 }
    }
}

impl Inventory {
    pub fn hotbar(&self, i: usize) -> Option<&Stack> {
        self.slots.get(HOTBAR_START + i)?.as_ref()
    }

    /// `set_player_inventory` uses the inventory's own order: 0-8 hotbar, 9-35 main, 36-39 armor feet..head, 40 offhand.
    pub fn set_player_slot(&mut self, index: usize, stack: Option<Stack>) {
        let menu = match index {
            0..=8 => HOTBAR_START + index,
            9..=35 => index,
            36..=39 => 8 - (index - 36),
            40 => 45,
            _ => return,
        };
        self.slots[menu] = stack;
    }
}


/// Click types of `container_click` (vanilla's `ContainerInput`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Click {
    Pickup = 0,
    QuickMove = 1,
    Swap = 2,
    Throw = 4,
    PickupAll = 6,
}

/// Sends a click. We don't predict the result: the changed-slots map is left empty, so the server sends back every
/// slot that changed, exactly as when a client's guess was wrong. The carried item is sent as a stack that can't
/// match (item 1, count 0), so the server always sends the real cursor back; an honest "empty" left the old
/// cursor stuck after placing an item.
pub fn send_click(tx: &mc_protocol::Sender, container: i32, state_id: i32, slot: i16, button: u8, click: Click) {
    let mut w = mc_protocol::Writer::new();
    w.varint(container).varint(state_id).u16(slot as u16).u8(button).varint(click as i32).varint(0);
    w.bool(true).varint(1).varint(0).varint(0).varint(0);
    let _ = tx.send(mc_protocol::ids::play::c2s::CONTAINER_CLICK, &w.buf);
}

/// What kind of screen a container opens, from its menu type name (`minecraft:generic_9x3`, `minecraft:furnace`, ...).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MenuKind {
    /// Chests, barrels, ender chests: rows of 9.
    Chest(u8),
    Dispenser,
    Crafting,
    /// furnace, blast furnace or smoker (by background name).
    Furnace(&'static str),
    Hopper,
    Shulker,
    /// Anything else: the container's slots in rows of 9 above the inventory.
    Other(u8),
}

impl MenuKind {
    pub fn from_name(name: &str) -> Self {
        match name.strip_prefix("minecraft:").unwrap_or(name) {
            "generic_9x1" => Self::Chest(1),
            "generic_9x2" => Self::Chest(2),
            "generic_9x3" => Self::Chest(3),
            "generic_9x4" => Self::Chest(4),
            "generic_9x5" => Self::Chest(5),
            "generic_9x6" => Self::Chest(6),
            "generic_3x3" | "crafter_3x3" => Self::Dispenser,
            "crafting" => Self::Crafting,
            "furnace" => Self::Furnace("furnace"),
            "blast_furnace" => Self::Furnace("blast_furnace"),
            "smoker" => Self::Furnace("smoker"),
            "hopper" => Self::Hopper,
            "shulker_box" => Self::Shulker,
            _ => Self::Other(0),
        }
    }

    /// Background sprite and its size in GUI pixels.
    pub fn background(self) -> (&'static str, f32, f32) {
        match self {
            Self::Chest(rows) => ("generic_54", 176.0, 114.0 + rows as f32 * 18.0),
            Self::Other(rows) => ("generic_54", 176.0, 114.0 + rows.clamp(1, 6) as f32 * 18.0),
            Self::Dispenser => ("dispenser", 176.0, 166.0),
            Self::Crafting => ("crafting_table", 176.0, 166.0),
            Self::Furnace(bg) => (bg, 176.0, 166.0),
            Self::Hopper => ("hopper", 176.0, 133.0),
            Self::Shulker => ("shulker_box", 176.0, 166.0),
        }
    }

    /// Top-left of every slot (container first, then 27 main and 9 hotbar), relative to the background.
    pub fn slots(self, container_slots: usize) -> Vec<(f32, f32)> {
        let grid = |x0: f32, y0: f32, cols: usize, n: usize| (0..n).map(move |i| (x0 + (i % cols) as f32 * 18.0, y0 + (i / cols) as f32 * 18.0));
        let (mut out, player_y): (Vec<(f32, f32)>, f32) = match self {
            Self::Chest(rows) | Self::Other(rows) => {
                let rows = rows.clamp(1, 6) as f32;
                (grid(8.0, 18.0, 9, container_slots).collect(), 18.0 + rows * 18.0 + 13.0)
            }
            Self::Shulker => (grid(8.0, 18.0, 9, container_slots).collect(), 84.0),
            Self::Dispenser => (grid(62.0, 17.0, 3, 9).collect(), 84.0),
            Self::Crafting => {
                let mut v = vec![(124.0, 35.0)];
                v.extend(grid(30.0, 17.0, 3, 9));
                (v, 84.0)
            }
            Self::Furnace(_) => (vec![(56.0, 17.0), (56.0, 53.0), (116.0, 35.0)], 84.0),
            Self::Hopper => (grid(44.0, 20.0, 5, 5).collect(), 51.0),
        };
        out.truncate(container_slots);
        while out.len() < container_slots {
            out.push((-1000.0, -1000.0));
        }
        out.extend(grid(8.0, player_y, 9, 27));
        out.extend(grid(8.0, player_y + 58.0, 9, 9));
        out
    }
}

/// An open container screen (chest, furnace, ...), as the server describes it.
pub struct Menu {
    pub id: i32,
    pub kind: MenuKind,
    pub title: String,
    /// Container slots followed by the player's 36.
    pub slots: Vec<Option<Stack>>,
    pub state_id: i32,
    /// `container_set_data` properties (furnace progress).
    pub data: HashMap<i16, i16>,
}

impl Menu {
    pub fn container_slots(&self) -> usize {
        self.slots.len().saturating_sub(36)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_protocol::Writer;

    fn comps() -> Components {
        Components::new(&HashMap::from([("damage".to_string(), 3), ("custom_name".to_string(), 6)]))
    }

    #[test]
    fn reads_plain_damaged_and_empty_stacks() {
        let mut w = Writer::new();
        w.varint(0); // empty
        w.varint(12).varint(42).varint(0).varint(0); // 12x item 42
        w.varint(1).varint(900).varint(1).varint(0).varint(3).varint(17); // damaged tool
        let mut r = Reader::new(&w.buf);
        let c = comps();
        assert_eq!(read_stack(&mut r, &c).unwrap(), None);
        assert_eq!(read_stack(&mut r, &c).unwrap().map(|s| (s.item, s.count)), Some((42, 12)));
        assert_eq!(read_stack(&mut r, &c).unwrap().and_then(|s| s.damage), Some(17));
    }

    #[test]
    fn unknown_component_keeps_the_stack() {
        let mut w = Writer::new();
        w.varint(3).varint(7).varint(1).varint(0).varint(99);
        let (stack, err) = read_stack(&mut Reader::new(&w.buf), &comps()).unwrap_err();
        assert_eq!(stack.map(|s| s.count), Some(3));
        assert_eq!(err.0, 99);
    }

    #[test]
    fn maps_player_inventory_indices() {
        let mut inv = Inventory::default();
        let s = Stack { item: 1, count: 1, damage: None, max_damage: None, name: None, enchanted: false, enchantments: Vec::new() };
        inv.set_player_slot(0, Some(s.clone()));
        inv.set_player_slot(39, Some(s.clone()));
        inv.set_player_slot(40, Some(s));
        assert!(inv.slots[36].is_some() && inv.slots[5].is_some() && inv.slots[45].is_some());
    }
}
