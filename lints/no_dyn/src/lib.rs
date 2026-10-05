#![feature(rustc_private)]

extern crate rustc_attr_ir;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_hir_analysis;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

use rustc_errors::DiagDecorator;
use rustc_hir::def_id::LocalDefId;
use rustc_hir::{AmbigArg, Body, Expr, FnDecl, Pat, Ty as HirTy, TyKind, intravisit::FnKind};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_middle::ty::{self, Ty, TypeVisitableExt, Unnormalized};
use rustc_span::{ExpnKind, MacroKind, Span, hygiene::AstPass, sym};

dylint_linting::dylint_library!();

rustc_session::declare_lint! {
    /// Reject trait objects and function pointers, including aliases and coercions.
    pub NO_DYN,
    Forbid,
    "dynamic dispatch is forbidden; use concrete types, generics or enums",
    report_in_external_macro
}

rustc_session::declare_lint_pass!(NoDyn => [NO_DYN]);

#[unsafe(no_mangle)]
pub fn register_lints(session: &rustc_session::Session, store: &mut rustc_lint::LintStore) {
    dylint_linting::init_config(session);
    store.register_lints(&[NO_DYN]);
    store.register_late_lint_pass(Box::new(|_| Box::new(NoDyn)));
}

fn has_dynamic_dispatch<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>) -> bool {
    let ty = if ty.has_escaping_bound_vars() {
        ty
    } else {
        cx.tcx
            .try_normalize_erasing_regions(cx.typing_env(), Unnormalized::new_wip(ty))
            .unwrap_or(ty)
    };
    let mut walk = ty.walk();
    while let Some(arg) = walk.next() {
        let Some(ty) = arg.as_type() else { continue };
        match ty.kind() {
            ty::Dynamic(..) | ty::FnPtr(..) => return true,
            ty::Closure(_, args) => {
                // rustc encodes a statically dispatched closure's signature as a
                // synthetic FnPtr. Inspect its real inputs/output and captures.
                walk.skip_current_subtree();
                let closure = args.as_closure();
                if has_dynamic_dispatch(cx, closure.tupled_upvars_ty())
                    || closure
                        .parent_args()
                        .iter()
                        .filter_map(|arg| arg.as_type())
                        .any(|ty| has_dynamic_dispatch(cx, ty))
                    || closure
                        .sig()
                        .skip_binder()
                        .inputs_and_output
                        .iter()
                        .any(|ty| has_dynamic_dispatch(cx, ty))
                {
                    return true;
                }
            }
            ty::CoroutineClosure(_, args) => {
                walk.skip_current_subtree();
                let closure = args.as_coroutine_closure();
                if has_dynamic_dispatch(cx, closure.tupled_upvars_ty())
                    || closure
                        .parent_args()
                        .iter()
                        .filter_map(|arg| arg.as_type())
                        .any(|ty| has_dynamic_dispatch(cx, ty))
                    || [
                        closure.signature_parts_ty(),
                        closure.coroutine_captures_by_ref_ty(),
                    ]
                    .into_iter()
                    .any(|signature| {
                        if let ty::FnPtr(types, _) = signature.kind() {
                            types
                                .skip_binder()
                                .inputs_and_output
                                .iter()
                                .any(|ty| has_dynamic_dispatch(cx, ty))
                        } else {
                            has_dynamic_dispatch(cx, signature)
                        }
                    })
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn check<'tcx>(cx: &LateContext<'tcx>, span: Span, ty: Ty<'tcx>) {
    if has_dynamic_dispatch(cx, ty) {
        report(cx, span);
    }
}

fn compiler_support(cx: &LateContext<'_>, span: Span) -> bool {
    // Inspect only this node's expansion. A user expression nested inside a
    // compiler macro keeps its own context and must still be checked.
    let expansion = span.ctxt().outer_expn_data();
    match expansion.kind {
        ExpnKind::AstPass(AstPass::TestHarness | AstPass::ProcMacroHarness) => true,
        ExpnKind::Macro(kind, name)
            if (kind == MacroKind::Derive && name == sym::Debug)
                || (kind == MacroKind::Attr && (name == sym::test || name == sym::bench)) =>
        {
            expansion.macro_def_id.is_some_and(|def_id| {
                rustc_attr_ir::find_attr!(cx.tcx, def_id, RustcBuiltinMacro { .. })
            })
        }
        _ => false,
    }
}

fn report(cx: &LateContext<'_>, span: Span) {
    if compiler_support(cx, span) {
        return;
    }
    cx.emit_span_lint(
        NO_DYN,
        span,
        DiagDecorator(|diag| {
            diag.primary_message("dynamic dispatch is forbidden");
            diag.help("use a concrete type, a generic parameter / impl Trait, or an enum");
        }),
    );
}

impl<'tcx> LateLintPass<'tcx> for NoDyn {
    fn check_ty(&mut self, cx: &LateContext<'tcx>, ty: &'tcx HirTy<'tcx, AmbigArg>) {
        if matches!(ty.kind, TyKind::TraitObject(..) | TyKind::FnPtr(..)) {
            report(cx, ty.span);
            return;
        }
        // Item types are lowered globally; body types belong to their typeck table.
        // Inferred `_` inside a body cannot be passed to the global lowering query.
        let lowered = if let Some(typeck) = cx.typeck_results {
            if typeck.hir_owner == ty.hir_id.owner {
                typeck.node_type_opt(ty.hir_id)
            } else {
                // Nested item types are outside the enclosing function body.
                Some(rustc_hir_analysis::lower_ty(cx.tcx, ty.as_unambig_ty()))
            }
        } else {
            Some(rustc_hir_analysis::lower_ty(cx.tcx, ty.as_unambig_ty()))
        };
        if let Some(lowered) = lowered {
            check(cx, ty.span, lowered);
        }
    }

    fn check_fn(
        &mut self,
        cx: &LateContext<'tcx>,
        kind: FnKind<'tcx>,
        decl: &'tcx FnDecl<'tcx>,
        _body: &'tcx Body<'tcx>,
        _span: Span,
        def_id: LocalDefId,
    ) {
        if !matches!(kind, FnKind::Closure) {
            let signature = cx.tcx.liberate_late_bound_regions(
                def_id.to_def_id(),
                cx.tcx.fn_sig(def_id).instantiate_identity().skip_norm_wip(),
            );
            for (input, ty) in decl.inputs.iter().zip(signature.inputs()) {
                check(cx, input.span, *ty);
            }
            check(cx, decl.output.span(), signature.output());
        }
    }

    fn check_pat(&mut self, cx: &LateContext<'tcx>, pat: &'tcx Pat<'tcx>) {
        check(cx, pat.span, cx.typeck_results().pat_ty(pat));
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let typeck = cx.typeck_results();
        let candidate = std::iter::once(typeck.expr_ty(expr))
            .chain(
                typeck
                    .expr_adjustments(expr)
                    .iter()
                    .map(|adjustment| adjustment.target),
            )
            .find(|ty| has_dynamic_dispatch(cx, *ty));
        if let Some(ty) = candidate {
            check(cx, expr.span, ty);
        }
    }
}

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
