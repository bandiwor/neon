//! A small lattice of *effects* that code can have, used by the optimizer.
//!
//! Every builtin has a fixed set of effects ([`Builtin::effects`](crate::builtin::Builtin::effects));
//! every IR instruction has one ([`Inst::effects`](crate::ir::Inst::effects)); every function gets
//! the union over its body and its callees ([`crate::ir::effects::analyze`]).
//!
//! The effects form a powerset lattice ordered by inclusion: `NONE` at the bottom, the join of two
//! pieces of code is the union of their effects, and "more effects" is always the safe answer, so a
//! fixed-point iteration over the call graph converges to the least sound summary.
//!
//! | effect       | meaning                                                                         |
//! |--------------|----------------------------------------------------------------------------------|
//! | `READS`      | reads memory that other code can change (fields, elements, globals, cells)       |
//! | `WRITES`     | changes such memory                                                              |
//! | `IO`         | talks to the outside world (printing, reading input)                             |
//! | `TRAPS`      | may stop the program with a run-time error (overflow, bounds, parse error, ...)  |
//! | `ALLOCS`     | creates a fresh heap object, so two calls never return the same one              |
//! | `DIVERGES`   | may run forever (a loop, recursion)                                              |
//! | `FRAGILE`    | only valid under a condition checked elsewhere (reading a variant's payload)     |
//!
//! From the set follows a [`Purity`] in the sense of GCC's attributes: **const** (the result depends
//! only on the arguments), **pure** (it may also read memory) and **impure** (it writes or does I/O).

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Effects(u8);

impl Effects {
    pub const NONE: Effects = Effects(0);
    pub const READS: Effects = Effects(1);
    pub const WRITES: Effects = Effects(1 << 1);
    pub const IO: Effects = Effects(1 << 2);
    pub const TRAPS: Effects = Effects(1 << 3);
    pub const ALLOCS: Effects = Effects(1 << 4);
    pub const DIVERGES: Effects = Effects(1 << 5);
    pub const FRAGILE: Effects = Effects(1 << 6);
    /// Everything: what an unknown callee might do.
    pub const ALL: Effects = Effects(0x7f);

    const NAMES: [(Effects, &'static str); 7] = [
        (Effects::READS, "reads"),
        (Effects::WRITES, "writes"),
        (Effects::IO, "io"),
        (Effects::TRAPS, "traps"),
        (Effects::ALLOCS, "allocates"),
        (Effects::DIVERGES, "may-diverge"),
        (Effects::FRAGILE, "fragile"),
    ];

    /// The join of two effect sets.
    pub const fn union(self, other: Effects) -> Effects {
        Effects(self.0 | other.0)
    }

    pub const fn contains(self, other: Effects) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Effects) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn reads(self) -> bool {
        self.contains(Effects::READS)
    }

    pub const fn writes(self) -> bool {
        self.contains(Effects::WRITES)
    }

    pub const fn allocates(self) -> bool {
        self.contains(Effects::ALLOCS)
    }

    pub const fn traps(self) -> bool {
        self.contains(Effects::TRAPS)
    }

    pub const fn diverges(self) -> bool {
        self.contains(Effects::DIVERGES)
    }

    /// const / pure / impure.
    pub const fn purity(self) -> Purity {
        if self.intersects(Effects::WRITES.union(Effects::IO)) {
            Purity::Impure
        } else if self.reads() {
            Purity::Pure
        } else {
            Purity::Const
        }
    }

    /// Dropping the code is invisible when its result is unused: it writes nothing, prints
    /// nothing, and neither traps nor runs forever. (Allocation is unobservable.)
    pub const fn removable(self) -> bool {
        !self.intersects(Effects::WRITES.union(Effects::IO).union(Effects::TRAPS).union(Effects::DIVERGES))
    }

    /// Running it where the program would not have (hoisted out of a loop that may not execute it)
    /// is invisible: removable, and no hidden precondition.
    pub const fn speculatable(self) -> bool {
        self.removable() && !self.contains(Effects::FRAGILE)
    }

    /// May be moved to run once instead of many times: speculatable, and not a fresh object each time.
    pub const fn hoistable(self) -> bool {
        self.speculatable() && !self.allocates()
    }

    /// Two executions with the same arguments (and the same memory, if it reads any) give the same
    /// result, so the second can reuse the first. A trap or an endless loop is reproduced by the
    /// first execution, so they do not matter here.
    pub const fn reusable(self) -> bool {
        !self.intersects(Effects::WRITES.union(Effects::IO).union(Effects::ALLOCS))
    }
}

impl std::ops::BitOr for Effects {
    type Output = Effects;
    fn bitor(self, rhs: Effects) -> Effects {
        self.union(rhs)
    }
}

impl std::ops::BitOrAssign for Effects {
    fn bitor_assign(&mut self, rhs: Effects) {
        *self = self.union(rhs);
    }
}

impl fmt::Debug for Effects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for Effects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = Effects::NAMES.iter().filter(|(e, _)| self.contains(*e)).map(|(_, n)| *n).collect();
        if names.is_empty() {
            f.write_str("-")
        } else {
            f.write_str(&names.join(", "))
        }
    }
}

/// How much a computation depends on and changes the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Purity {
    /// The result depends only on the arguments.
    Const,
    /// The result may also depend on memory, which nothing is written to.
    Pure,
    /// Writes memory or does I/O.
    Impure,
}

impl fmt::Display for Purity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Purity::Const => "const",
            Purity::Pure => "pure",
            Purity::Impure => "impure",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purity_follows_the_effects() {
        assert_eq!(Effects::NONE.purity(), Purity::Const);
        assert_eq!((Effects::TRAPS | Effects::ALLOCS).purity(), Purity::Const);
        assert_eq!(Effects::READS.purity(), Purity::Pure);
        assert_eq!((Effects::READS | Effects::TRAPS).purity(), Purity::Pure);
        assert_eq!(Effects::WRITES.purity(), Purity::Impure);
        assert_eq!((Effects::READS | Effects::IO).purity(), Purity::Impure);
        assert_eq!(Effects::ALL.purity(), Purity::Impure);
        assert!(Purity::Const < Purity::Pure && Purity::Pure < Purity::Impure);
    }

    #[test]
    fn what_may_be_done_with_unused_code() {
        // removable: nothing observable
        assert!(Effects::NONE.removable() && Effects::READS.removable() && Effects::ALLOCS.removable());
        assert!(!Effects::TRAPS.removable() && !Effects::DIVERGES.removable());
        assert!(!Effects::WRITES.removable() && !Effects::IO.removable());
        // speculation additionally needs the precondition to hold
        assert!(Effects::READS.speculatable() && !Effects::FRAGILE.speculatable());
        // hoisting must not merge distinct allocations
        assert!(Effects::READS.hoistable() && !Effects::ALLOCS.hoistable());
        // reuse: allocations are distinct objects, writes and I/O are events
        assert!((Effects::READS | Effects::TRAPS | Effects::DIVERGES).reusable());
        assert!(!Effects::ALLOCS.reusable() && !Effects::WRITES.reusable() && !Effects::IO.reusable());
    }

    #[test]
    fn the_join_is_a_union() {
        let a = Effects::READS | Effects::TRAPS;
        let b = Effects::TRAPS | Effects::IO;
        assert_eq!(a.union(b), Effects::READS | Effects::TRAPS | Effects::IO);
        assert!(a.union(b).contains(a) && a.union(b).contains(b));
        assert_eq!(a.union(a), a);
        assert_eq!(a.union(Effects::NONE), a);
        assert!(Effects::ALL.contains(a.union(b)));
        assert_eq!(format!("{}", a), "reads, traps");
        assert_eq!(format!("{}", Effects::NONE), "-");
    }
}
