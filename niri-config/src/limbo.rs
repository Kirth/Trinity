use crate::utils::MergeWith;

/// Settings for limbo, the hidden place for windows poofed out of the way in i3 mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Limbo {
    /// Edge of the screen where the drawer of windows in limbo sits.
    pub edge: LimboEdge,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LimboPart {
    #[knuffel(child, unwrap(argument))]
    pub edge: Option<LimboEdge>,
}

impl MergeWith<LimboPart> for Limbo {
    fn merge_with(&mut self, part: &LimboPart) {
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
