//! Register bytecode: 32-bit instructions with wide 64-bit forms.
//!
//! * [`opcode`] — the instruction set (numbering is part of the format)
//! * [`insn`] — structured instructions with symbolic labels
//! * [`asm`] — encoding (with branch relaxation) and decoding
//! * [`chunk`] — compiled functions and programs
//! * [`disasm`] — listings
//! * [`analysis`] — liveness over instruction lists
//! * [`peephole`] — move elimination and compare/branch fusion

pub mod analysis;
pub mod asm;
pub mod chunk;
pub mod disasm;
pub mod insn;
pub mod opcode;
pub mod peephole;
pub mod verify;

pub use asm::{assemble, decode, decode_one, AsmError, Assembled, DecodeError, NO_DESC};
pub use chunk::{BytecodeProgram, Chunk};
pub use disasm::{disassemble, disassemble_chunk, format_insn};
pub use insn::{Capture, Insn, Item, Label, Reg};
pub use opcode::{CmpKind, FusedKind, Opcode};
pub use verify::{verify_chunk, verify_program};

#[cfg(test)]
mod tests;
