use crate::utils::{Flag, MergeWith};

/// Settings for limbo, the hidden place for windows poofed out of the way in i3 mode.
///
/// Off by default: windows set aside go to the strip of their workspace instead.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Limbo {
    /// Whether windows set aside go to limbo rather than the strip of their workspace.
    pub on: bool,
    /// Edge of the screen where the drawer of windows in limbo sits.
    pub edge: LimboEdge,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LimboPart {
    #[knuffel(child)]
    pub on: Option<Flag>,
    #[knuffel(child, unwrap(argument))]
    pub edge: Option<LimboEdge>,
}

impl MergeWith<LimboPart> for Limbo {
    fn merge_with(&mut self, part: &LimboPart) {
        merge!((self, part), on);
        merge_clone!((self, part), edge);
    }
}

#[derive(knuffel::DecodeScalar, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum LimboEdge {
    Left,
    #[default]
    Right,
    Bottom,
}
