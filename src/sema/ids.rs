//! Index newtypes for the semantic tables.

macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        $(
            $(#[$doc])*
            #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
            pub struct $name(pub u32);

            impl $name {
                pub fn index(self) -> usize {
                    self.0 as usize
                }
            }
        )*
    };
}

ids! {
    /// A loaded source module (index in dependency order).
    ModuleId,
    FnId,
    /// A struct or enum.
    AdtId,
    TraitId,
    ImplId,
    /// A module-level `let` or `const`.
    GlobalId,
    /// A local variable, parameter or pattern binding.
    LocalId,
    /// Any named entity, for definition/reference queries.
    DefId,
}
