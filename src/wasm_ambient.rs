//! ambient record の Wasm 側の席割り当て。
//!
//! `ambient_abi` が決めた instance と provider selection をそのまま物理的な
//! Wasm local に写すだけで、ここでは名前解決・実装探索・所有権判断をしない。
//! hidden record fields も provision values も root address の `i32` で運ぶ。

use crate::ambient_abi::RecordLayout;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::Id as _;

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
}
