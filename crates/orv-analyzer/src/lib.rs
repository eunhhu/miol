//! Semantic analysis for the orv language.

mod analysis;
mod lower;
mod validate;

pub use analysis::{Analysis, analyze};
pub use orv_hir::{
    AssignOp, BinaryOp, Expr, Module as HirModule, ScopeRef, SymbolRef, Type, dump_hir,
};

#[cfg(test)]
mod tests;
