//! ambient record の Wasm 側の席割り当て。
//!
//! `ambient_abi` が決めた instance と provider selection をそのまま物理的な
//! Wasm local に写すだけで、ここでは名前解決・実装探索・所有権判断をしない。
//! hidden record fields も provision values も root address の `i32` で運ぶ。

use crate::ambient_abi::{Plan, RecordLayout, ValueSource};
use crate::diag::Diag;
use crate::hir;
use std::collections::{BTreeMap, BTreeSet};

/// 1 instance が受け取った hidden ambient field の local 番号。
///
/// 番号は `RecordLayout::fields` の順（すなわち planner が確定した `SlotId` 順）
/// に振る。`BTreeMap` は emitter 側で slot から引くためだけの索引で、並びを
/// 決め直すものではない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IncomingFields {
    locals: BTreeMap<hir::SlotId, u32>,
}

impl IncomingFields {
    /// `slot` に対応する hidden parameter の local 番号。
    pub fn get(&self, slot: hir::SlotId) -> Option<u32> {
        self.locals.get(&slot).copied()
    }
}

/// `with slot(value)` の値を留める local 番号。
///
/// provision expression ID を key にするので、同じ provider value は同じ
/// `with` body 内の複数 call が読んでも、評価・格納は一度だけで済む。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProvisionSeats {
    locals: BTreeMap<hir::ExprId, u32>,
}

impl ProvisionSeats {
    /// provision expression に対応する local 番号。
    pub fn get(&self, expression: hir::ExprId) -> Option<u32> {
        self.locals.get(&expression).copied()
    }
}

/// backend が ambient 用に追加する local の全体。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Locals {
    pub incoming: IncomingFields,
    pub provisions: ProvisionSeats,
    /// ambient fields と provision seats の直後の最初の local 番号。
    pub next: u32,
}

/// planner の runtime layout と provision expression IDs を local 番号へ写す。
///
/// `first` は receiver と declared parameter が既に占めた数。layout が無ければ
/// hidden parameter は増えない。provision IDs は ID 順に正規化するため、HIR の
/// 走査順が変わっても同じ body から同じ席が得られる。
pub fn allocate(
    layout: Option<&RecordLayout>,
    provision_ids: impl IntoIterator<Item = hir::ExprId>,
    first: u32,
) -> Locals {
    let mut next = first;
    let mut incoming = IncomingFields::default();
    if let Some(layout) = layout {
        for (slot, _) in &layout.fields {
            assert!(
                incoming.locals.insert(*slot, next).is_none(),
                "ambient record に同じ slot が二度あります: {slot:?}"
            );
            next += 1;
        }
    }

    let mut provisions = ProvisionSeats::default();
    for expression in provision_ids.into_iter().collect::<BTreeSet<_>>() {
        provisions.locals.insert(expression, next);
        next += 1;
    }

    Locals {
        incoming,
        provisions,
        next,
    }
}

/// planner の直接 call edge を Wasm function index に写す前に検証する。
///
/// この層は provider を選び直さない。`PlannedCall` が既に選んだ target と
/// projection が、target instance の receiver と runtime record に合うかだけを
/// 確かめる。壊れた内部計画は `unreachable!` や index panic ではなく、生成前の
/// 構造化診断として返す。
pub fn validate_plan(program: &hir::Program, plan: &Plan) -> Vec<Diag> {
    let mut diagnostics = Vec::new();
    let count = plan.instance_count();

    for (_, caller) in plan.instances() {
        let hir::BodyId::Callable(caller_id) = caller.key.body else {
            diagnostics.push(Diag::msg("Wasm production plan contains a test body"));
            continue;
        };
        let body = &program.callables[caller_id].body;
        let incoming = caller.layout.map(|layout| &plan.layout(layout).fields);

        for (expr_id, call) in &caller.calls {
            if call.target.index() >= count {
                diagnostics.push(Diag::msg(format!(
                    "Wasm plan call expr#{:?} targets missing instance#{}",
                    expr_id,
                    call.target.index()
                )));
                continue;
            }
            let target = plan.instance(call.target);
            let hir::BodyId::Callable(target_id) = target.key.body else {
                diagnostics.push(Diag::msg(format!(
                    "Wasm plan call expr#{expr_id:?} targets a test body"
                )));
                continue;
            };

            let Some(source) = body
                .exprs()
                .find_map(|(id, expr)| (id == *expr_id).then_some(expr))
            else {
                diagnostics.push(Diag::msg(format!(
                    "Wasm plan contains call for missing expr#{expr_id:?}"
                )));
                continue;
            };
            let hir::ExprKind::Call(source_call) = &source.kind else {
                diagnostics.push(Diag::msg(format!(
                    "Wasm plan expr#{expr_id:?} is not a call"
                )));
                continue;
            };

            let (has_receiver, needs_planned_receiver) = match source_call {
                hir::Call::Direct { .. } | hir::Call::Associated { .. } => (false, false),
                hir::Call::Method { .. } => (true, false),
                hir::Call::Slot { receiver, .. } => (
                    matches!(receiver, hir::SlotReceiver::Value),
                    matches!(receiver, hir::SlotReceiver::Value),
                ),
                hir::Call::Ctor { .. } => {
                    diagnostics.push(Diag::msg(format!(
                        "Wasm plan must not contain constructor call expr#{expr_id:?}"
                    )));
                    continue;
                }
            };
            let callee = &program.callables[target_id];
            if callee.receiver.is_some() != has_receiver {
                diagnostics.push(Diag::msg(format!(
                    "Wasm plan receiver shape differs for call expr#{expr_id:?}"
                )));
            }
            if call.receiver.is_some() != needs_planned_receiver {
                diagnostics.push(Diag::msg(format!(
                    "Wasm plan provider receiver differs for call expr#{expr_id:?}"
                )));
            }

            let expected = target
                .layout
                .map(|layout| &plan.layout(layout).fields[..])
                .unwrap_or(&[]);
            if call.projection.len() != expected.len() {
                diagnostics.push(Diag::msg(format!(
                    "Wasm plan hidden-parameter width differs for call expr#{expr_id:?}"
                )));
                continue;
            }
            for ((slot, source), (expected_slot, expected_type)) in
                call.projection.iter().zip(expected)
            {
                if slot != expected_slot {
                    diagnostics.push(Diag::msg(format!(
                        "Wasm plan projection order differs for call expr#{expr_id:?}"
                    )));
                    continue;
                }
                let matches_type = match source {
                    ValueSource::Incoming(slot) => incoming
                        .and_then(|fields| fields.iter().find(|(found, _)| found == slot))
                        .is_some_and(|(_, found_type)| found_type == expected_type),
                    ValueSource::Provision(provision) => body
                        .exprs()
                        .find_map(|(id, expr)| (id == *provision).then_some(expr))
                        .and_then(|expr| expr.result.ty())
                        .is_some_and(|ty| {
                            matches!(ty.kind, hir::TypeKind::Struct(found) if found == *expected_type)
                        }),
                };
                if !matches_type {
                    diagnostics.push(Diag::msg(format!(
                        "Wasm plan projection type differs for call expr#{expr_id:?}"
                    )));
                }
            }
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::Id as _;

    fn canonical_plan() -> crate::ambient_abi::ProductionPlan {
        let src = std::fs::read_to_string("examples/canonical.rd").expect("fixture を読めるはず");
        crate::wasm::tests::plan_of(&src, &[]).1
    }

    #[test]
    fn incomingとprovisionの席は計画のidから決定的に振る() {
        let first = 4;
        let db = hir::SlotId::from_index(1);
        let clock = hir::SlotId::from_index(3);
        let db_type = hir::StructId::from_index(0);
        let clock_type = hir::StructId::from_index(1);
        let early = hir::ExprId::from_index(2);
        let later = hir::ExprId::from_index(9);
        let layout = RecordLayout {
            fields: vec![(db, db_type), (clock, clock_type)],
        };

        let locals = allocate(Some(&layout), [later, early, later], first);

        assert_eq!(locals.incoming.get(db), Some(4));
        assert_eq!(locals.incoming.get(clock), Some(5));
        assert_eq!(locals.provisions.get(early), Some(6));
        assert_eq!(locals.provisions.get(later), Some(7));
        assert_eq!(locals.next, 8);
    }

    #[test]
    fn empty_recordとprovision無しはlocalを増やさない() {
        let locals = allocate(None, [], 12);
        assert_eq!(
            locals,
            Locals {
                next: 12,
                ..Locals::default()
            }
        );
    }

    #[test]
    fn validatorは壊れたtargetとprojectionを拒否する() {
        let mut production = canonical_plan();
        let entry = production.entry;
        let expr = *production
            .plan
            .instance(entry)
            .calls
            .keys()
            .next()
            .expect("main は call を持つ");
        production
            .plan
            .instance_mut_for_test(entry)
            .calls
            .get_mut(&expr)
            .expect("call がある")
            .target = Plan::instance_id_for_test(production.plan.instance_count());
        let diagnostics = validate_plan(
            &crate::wasm::tests::plan_of(
                &std::fs::read_to_string("examples/canonical.rd").expect("fixture を読めるはず"),
                &[],
            )
            .0
            .hir,
            &production.plan,
        );
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.msg.contains("targets missing")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn validatorはreceiverとprojection順を拒否する() {
        let (checked, mut production) = crate::wasm::tests::plan_of(
            &std::fs::read_to_string("examples/canonical.rd").expect("fixture を読めるはず"),
            &[],
        );
        let entry = production.entry;
        let expr = *production
            .plan
            .instance(entry)
            .calls
            .iter()
            .find(|(_, call)| call.projection.len() > 1)
            .map(|(expr, _)| expr)
            .expect("main は二つの provider を forward する");
        let call = production
            .plan
            .instance_mut_for_test(entry)
            .calls
            .get_mut(&expr)
            .expect("call がある");
        call.receiver = Some(ValueSource::Incoming(hir::SlotId::from_index(0)));
        call.projection.swap(0, 1);

        let diagnostics = validate_plan(&checked.hir, &production.plan);
        let messages = diagnostics
            .iter()
            .map(|diag| diag.msg.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        assert!(messages.contains("provider receiver"), "{messages}");
        assert!(messages.contains("projection order"), "{messages}");
    }

    #[test]
    fn validatorはhidden幅とprojection型を拒否する() {
        let source =
            std::fs::read_to_string("examples/canonical.rd").expect("fixture を読めるはず");
        let (checked, mut production) = crate::wasm::tests::plan_of(&source, &[]);
        let entry = production.entry;
        let expr = *production
            .plan
            .instance(entry)
            .calls
            .iter()
            .find(|(_, call)| call.projection.len() > 1)
            .map(|(expr, _)| expr)
            .expect("main は二つの provider を forward する");
        production
            .plan
            .instance_mut_for_test(entry)
            .calls
            .get_mut(&expr)
            .expect("call がある")
            .projection
            .pop();
        let diagnostics = validate_plan(&checked.hir, &production.plan);
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.msg.contains("hidden-parameter width")),
            "{diagnostics:?}"
        );

        let (checked, mut production) = crate::wasm::tests::plan_of(&source, &[]);
        let entry = production.entry;
        let expr = *production
            .plan
            .instance(entry)
            .calls
            .iter()
            .find(|(_, call)| call.projection.len() > 1)
            .map(|(expr, _)| expr)
            .expect("main は二つの provider を forward する");
        production
            .plan
            .instance_mut_for_test(entry)
            .calls
            .get_mut(&expr)
            .expect("call がある")
            .projection[0]
            .1 = ValueSource::Incoming(hir::SlotId::from_index(0));
        let diagnostics = validate_plan(&checked.hir, &production.plan);
        assert!(
            diagnostics
                .iter()
                .any(|diag| diag.msg.contains("projection type")),
            "{diagnostics:?}"
        );
    }
}
