use orv_diagnostics::DiagnosticBag;
use orv_resolve::{ResolveResult, ScopeId, resolve};
use orv_syntax::ast;

use crate::lower::lower_module;
use crate::validate::validate;

/// Semantic analysis output for a single parsed module.
#[derive(Debug)]
pub struct Analysis {
    pub hir: orv_hir::Module,
    pub symbols: orv_resolve::SymbolTable,
    pub scopes: orv_resolve::ScopeMap,
    pub root_scope: ScopeId,
}

/// Runs semantic analysis for a parsed module.
pub fn analyze(module: &ast::Module) -> (Analysis, DiagnosticBag) {
    let (result, diagnostics) = resolve(module);
    let validation = validate(module);
    let hir = lower_module(&result, module);

    let analysis = build_analysis(result, hir);
    (analysis, merge_diagnostics(diagnostics, validation))
}

fn build_analysis(result: ResolveResult, hir: orv_hir::Module) -> Analysis {
    Analysis {
        hir,
        symbols: result.symbols,
        scopes: result.scopes,
        root_scope: result.root_scope,
    }
}

fn merge_diagnostics(mut initial: DiagnosticBag, next: DiagnosticBag) -> DiagnosticBag {
    for diagnostic in next.into_vec() {
        initial.push(diagnostic);
    }
    initial
}
