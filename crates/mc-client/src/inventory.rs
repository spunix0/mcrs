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
    let mut stack = Stack { item, count: count as u32, damage: None, max_damage: None, name: None, enchanted: false };
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
                    io(r.varint())?;
                    io(r.varint())?;
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
}

impl Default for Inventory {
    fn default() -> Self {
        Self { slots: vec![None; SLOTS] }
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
        let s = Stack { item: 1, count: 1, damage: None, max_damage: None, name: None, enchanted: false };
        inv.set_player_slot(0, Some(s.clone()));
        inv.set_player_slot(39, Some(s.clone()));
        inv.set_player_slot(40, Some(s));
        assert!(inv.slots[36].is_some() && inv.slots[5].is_some() && inv.slots[45].is_some());
    }
}
