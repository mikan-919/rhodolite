//! ambient の実行時契約。要求推論の結果を、コンパイル可能な引数と直接呼び出しへ
//! 落とすための内部表現(design.md 決定11)。
//!
//! プログラムは whole-program で、提供される実装は全て HIR に載っている。だから
//! 到達した本体を「どの実装の組み合わせで呼ばれたか」ごとに複製すれば、スロット
//! 呼び出しは具体的な `CallableId` への直接呼び出しになり、vtable は要らない。
//!
//! ここが持つのは**特殊化の情報だけ**で、式の形は HIR のまま。後段の C 生成は
//! 普通の式を HIR から読み、呼び先・record・レシーバをこの計画から読む。
//!
//! 語彙は4つ:
//!
//!   - `InstanceKey` … 本体 + 要求に制限した実装選択。**提供値は入らない**
//!   - `RecordLayout` … 値要求だけが席を持つ ambient record の形
//!   - `ValueSource`  … その席の handle をどこから取るか
//!   - `PlannedCall`  … 呼び出し地点1つ分の行き先・レシーバ・射影

use crate::hir;
use crate::hir::Id as _;
use std::collections::BTreeMap;
use std::fmt::Write as _;

// ---------------------------------------------------------------------------
// ID
// ---------------------------------------------------------------------------

/// 特殊化された本体1つ。確保順に振るので、同じプログラムからは同じ番号になる。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct InstanceId(u32);

/// ambient record の形1つ。値要求の並びが同じ instance は共有する。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RecordLayoutId(u32);

macro_rules! plan_ids {
    ($($name:ident),+ $(,)?) => { $(
        impl $name {
            fn from_index(index: usize) -> Self {
                match u32::try_from(index) {
                    Ok(raw) => Self(raw),
                    Err(_) => panic!(concat!(stringify!($name), " の個数が u32 を超えました")),
                }
            }

            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    )+ };
}

plan_ids!(InstanceId, RecordLayoutId);

// ---------------------------------------------------------------------------
// 語彙
// ---------------------------------------------------------------------------

/// instance の同一性(design.md 決定3)。
///
/// 実装選択だけが入る。実行時の値・それを作った式・呼び出し地点は入らないので、
/// 別々の `InMemoryDb` を渡す2つの呼び出しは同じコードを共有し、record だけが
/// 違う handle を運ぶ。要求の強さは本体の要求結果が決めるので鍵には持たない。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct InstanceKey {
    pub body: hir::BodyId,
    /// `SlotId` 順。その本体の要求に制限したものだけが入る
    pub providers: Vec<(hir::SlotId, hir::TraitImplId)>,
}

/// ambient record の形(design.md 決定6)。
///
/// 値要求だけが席を持つ。型提供は instance の鍵と直接呼び出し先を変えるが、
/// 実行時には何も運ばないので欄にならない。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct RecordLayout {
    /// `SlotId` 順の (スロット, 具体的な provider の型)
    pub fields: Vec<(hir::SlotId, hir::StructId)>,
}

/// record の欄に載せる handle の出どころ。
///
/// provider の実体そのものは複製しない。record を作る・写す・渡すのは handle の
/// 複製で、変更は別名からも見える(design.md 決定7)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ValueSource {
    /// 自分が受け取った ambient record の欄
    Incoming(hir::SlotId),
    /// この本体の `with` が外側で評価した提供式
    Provision(hir::ExprId),
}

/// 提供文脈の1スロット分(design.md 決定5)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProviderBinding {
    pub implementation: hir::TraitImplId,
    /// 型提供は `None`。実行時表現を持たない
    pub value: Option<ValueSource>,
}

/// 呼び出し地点1つ分の計画。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlannedCall {
    pub target: InstanceId,
    /// 値スロット呼び出しで具体メソッドの `self` に渡す handle。
    /// 型スロット呼び出しと通常の呼び出しは持たない
    pub receiver: Option<ValueSource>,
    /// 呼び先の record を作る材料。呼び先の layout と同じ並び
    pub projection: Vec<(hir::SlotId, ValueSource)>,
}

/// 特殊化された本体1つ。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Instance {
    pub key: InstanceKey,
    /// 値要求が1つも無ければ隠し ambient 引数を持たない
    pub layout: Option<RecordLayoutId>,
    /// 本体の `ExprId` → その呼び出しの計画
    pub calls: BTreeMap<hir::ExprId, PlannedCall>,
}

/// 計画そのもの。根から到達した instance だけが入る。
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Plan {
    /// 根(エントリと各 test)。宣言順
    pub roots: Vec<(hir::BodyId, InstanceId)>,
    instances: Vec<Instance>,
    layouts: Vec<RecordLayout>,
    /// 同じ鍵は同じ instance。再帰はこの表が有限に閉じる(design.md 決定10)
    interned: BTreeMap<InstanceKey, InstanceId>,
    layout_ids: BTreeMap<Vec<(hir::SlotId, hir::StructId)>, RecordLayoutId>,
}

impl Plan {
    pub fn instance(&self, id: InstanceId) -> &Instance {
        &self.instances[id.index()]
    }

    pub fn layout(&self, id: RecordLayoutId) -> &RecordLayout {
        &self.layouts[id.index()]
    }

    /// 確保順の全 instance。出力の並びは常にこれで決める
    pub fn instances(&self) -> impl Iterator<Item = (InstanceId, &Instance)> {
        self.instances
            .iter()
            .enumerate()
            .map(|(index, instance)| (InstanceId::from_index(index), instance))
    }

    pub fn layouts(&self) -> impl Iterator<Item = (RecordLayoutId, &RecordLayout)> {
        self.layouts
            .iter()
            .enumerate()
            .map(|(index, layout)| (RecordLayoutId::from_index(index), layout))
    }

    /// 鍵を instance へ寄せる。**本体を歩く前に**確保するので、再帰の辺は
    /// まだ空の自分自身を指せる(design.md 決定10)。
    ///
    /// 返る `bool` は「初めて作った」。真のときだけ呼び出し元が本体を歩く
    fn intern(&mut self, key: InstanceKey, layout: Option<RecordLayoutId>) -> (InstanceId, bool) {
        if let Some(found) = self.interned.get(&key) {
            return (*found, false);
        }
        let id = InstanceId::from_index(self.instances.len());
        self.interned.insert(key.clone(), id);
        self.instances.push(Instance {
            key,
            layout,
            calls: BTreeMap::new(),
        });
        (id, true)
    }

    /// 値要求の並びを layout へ寄せる。空なら隠し引数を持たない
    fn intern_layout(
        &mut self,
        fields: Vec<(hir::SlotId, hir::StructId)>,
    ) -> Option<RecordLayoutId> {
        if fields.is_empty() {
            return None;
        }
        if let Some(found) = self.layout_ids.get(&fields) {
            return Some(*found);
        }
        let id = RecordLayoutId::from_index(self.layouts.len());
        self.layout_ids.insert(fields.clone(), id);
        self.layouts.push(RecordLayout { fields });
        Some(id)
    }
}

// ---------------------------------------------------------------------------
// 計画の失敗
// ---------------------------------------------------------------------------

/// 前段を通ったのに計画できない状態。ここへ来るのは不変条件の破れなので、
/// panic ではなく構造化して返す(design.md リスク)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PlanError {
    /// 提供文脈に、呼び先が要求するスロットが無い
    MissingProvider {
        body: hir::BodyId,
        slot: hir::SlotId,
    },
    /// 値要求に対して型提供しか無い。逆(値提供で型要求)は満たせる
    TypeOnlyProvider {
        body: hir::BodyId,
        slot: hir::SlotId,
    },
    /// 選ばれた実装がその契約メソッドを実装していない
    UnimplementedMethod {
        implementation: hir::TraitImplId,
        method: hir::TraitMethodId,
    },
}

impl PlanError {
    /// 表示の境界。ID を名前へ戻すのはここだけ
    pub fn show(&self, program: &hir::Program) -> String {
        match self {
            PlanError::MissingProvider { body, slot } => format!(
                "{}: `{}` の提供が計画時に見つかりません",
                program.show_body(*body),
                program.slots[*slot].name
            ),
            PlanError::TypeOnlyProvider { body, slot } => format!(
                "{}: `{}` は実体を要求しますが、型だけが提供されています",
                program.show_body(*body),
                program.slots[*slot].name
            ),
            PlanError::UnimplementedMethod {
                implementation,
                method,
            } => format!(
                "impl {} は `{}` を実装していません",
                program.structs[program.trait_impls[*implementation].type_].name,
                program.trait_methods[*method].name
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// 決定的な描画
// ---------------------------------------------------------------------------

impl Plan {
    /// 同じ計画からは常に同じ文字列が出る。ID を名前へ戻すのはここだけで、
    /// 計画そのものの辺には文字列が入らない(design.md 決定11)。
    pub fn render(&self, program: &hir::Program) -> String {
        let mut out = String::new();
        for (body, instance) in &self.roots {
            let _ = writeln!(
                out,
                "root {} -> instance#{}",
                program.show_body(*body),
                instance.index()
            );
        }
        for (id, layout) in self.layouts() {
            let fields: Vec<String> = layout
                .fields
                .iter()
                .map(|(slot, type_)| {
                    format!(
                        "{}: {}",
                        program.slots[*slot].name, program.structs[*type_].name
                    )
                })
                .collect();
            let _ = writeln!(out, "layout#{} {{ {} }}", id.index(), fields.join(", "));
        }
        for (id, instance) in self.instances() {
            let providers: Vec<String> = instance
                .key
                .providers
                .iter()
                .map(|(slot, implementation)| {
                    format!(
                        "{}={}",
                        program.slots[*slot].name,
                        program.structs[program.trait_impls[*implementation].type_].name
                    )
                })
                .collect();
            let ambient = match instance.layout {
                Some(layout) => format!("layout#{}", layout.index()),
                None => "-".to_string(),
            };
            let _ = writeln!(
                out,
                "instance#{} {} [{}] ambient {ambient}",
                id.index(),
                program.show_body(instance.key.body),
                providers.join(", ")
            );
            for (expr, call) in &instance.calls {
                let projection: Vec<String> = call
                    .projection
                    .iter()
                    .map(|(slot, source)| {
                        format!(
                            "{} <- {}",
                            program.slots[*slot].name,
                            show_source(program, *source)
                        )
                    })
                    .collect();
                let receiver = match call.receiver {
                    Some(source) => format!(" self={}", show_source(program, source)),
                    None => String::new(),
                };
                let record = if projection.is_empty() {
                    "{}".to_string()
                } else {
                    format!("{{ {} }}", projection.join(", "))
                };
                let _ = writeln!(
                    out,
                    "  expr#{} -> instance#{}{receiver} {record}",
                    expr.index(),
                    call.target.index()
                );
            }
        }
        out
    }
}

fn show_source(program: &hir::Program, source: ValueSource) -> String {
    match source {
        ValueSource::Incoming(slot) => format!("field {}", program.slots[slot].name),
        ValueSource::Provision(expr) => format!("provision expr#{}", expr.index()),
    }
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 語彙のテストが使う宣言。同じ契約に実装が2つずつあるので、実装選択の
    /// 違いが instance を分けることを言える
    pub(super) const PRELUDE: &str = "trait Clock { fn now(self -> int)\n fn zero(-> int) }\n\
         trait Database { fn save(self, n: int -> unit) }\n\
         struct Frozen { t: int }\n\
         struct Zero {}\n\
         struct InMemoryDb {}\n\
         struct Postgres {}\n\
         impl Clock for Frozen { fn now(self -> int) { self.t }\n fn zero(-> int) { 0 } }\n\
         impl Clock for Zero { fn now(self -> int) { 0 }\n fn zero(-> int) { 0 } }\n\
         impl Database for InMemoryDb { fn save(self, n: int -> unit) { let ignored = n } }\n\
         impl Database for Postgres { fn save(self, n: int -> unit) { let ignored = n } }\n\
         effect clock: Clock\n\
         effect db: Database\n";

    pub(super) fn lowered_of(src: &str) -> hir::Program {
        let source = format!("{PRELUDE}{src}");
        let parsed = crate::parse::parse(&crate::lex::join(crate::lex::lex(&source).unwrap()))
            .expect("パースできるはず");
        crate::typecheck::check_and_lower(&parsed).expect("型検査を通るはず")
    }

    pub(super) fn slot(program: &hir::Program, name: &str) -> hir::SlotId {
        program
            .slots
            .iter()
            .find(|(_, slot)| slot.name == name)
            .map(|(id, _)| id)
            .expect("そのスロットがない")
    }

    /// `impl Trait for Struct` を実装先の型名で引く
    pub(super) fn implementation(program: &hir::Program, type_: &str) -> hir::TraitImplId {
        program
            .trait_impls
            .iter()
            .find(|(_, decl)| program.structs[decl.type_].name == type_)
            .map(|(id, _)| id)
            .expect("その実装がない")
    }

    pub(super) fn struct_(program: &hir::Program, name: &str) -> hir::StructId {
        program
            .structs
            .iter()
            .find(|(_, decl)| decl.name == name)
            .map(|(id, _)| id)
            .expect("その型がない")
    }

    fn body(program: &hir::Program, name: &str) -> hir::BodyId {
        hir::BodyId::Callable(program.free_callable(name).expect("その関数がない"))
    }

    fn key(program: &hir::Program, name: &str, providers: &[(&str, &str)]) -> InstanceKey {
        let mut providers: Vec<(hir::SlotId, hir::TraitImplId)> = providers
            .iter()
            .map(|(s, i)| (slot(program, s), implementation(program, i)))
            .collect();
        providers.sort();
        InstanceKey {
            body: body(program, name),
            providers,
        }
    }

    const SOURCE: &str = "fn used(-> int) { clock.now() }\n\
                          fn main(-> int) { 0 }\n";

    /// 同じ鍵は同じ instance。2度目は「初めて作った」を返さない
    #[test]
    fn 同じ鍵は同じinstanceへ寄る() {
        let program = lowered_of(SOURCE);
        let mut plan = Plan::default();

        let (first, fresh) = plan.intern(key(&program, "used", &[("clock", "Frozen")]), None);
        assert!(fresh, "初めての鍵");
        let (second, fresh) = plan.intern(key(&program, "used", &[("clock", "Frozen")]), None);
        assert_eq!(first, second);
        assert!(!fresh, "2度目は本体を歩き直さない");
        assert_eq!(plan.instances().count(), 1);
    }

    /// 実装選択が違えば別の instance。呼び先が変わるので共有できない
    #[test]
    fn 実装選択が違えば別のinstance() {
        let program = lowered_of(SOURCE);
        let mut plan = Plan::default();

        let (frozen, _) = plan.intern(key(&program, "used", &[("clock", "Frozen")]), None);
        let (zero, _) = plan.intern(key(&program, "used", &[("clock", "Zero")]), None);
        assert_ne!(frozen, zero);
        assert_eq!(plan.instances().count(), 2);
    }

    /// 値要求の並びが同じなら layout は共有する。型だけの選択が違っても同じ
    #[test]
    fn 同じ欄を持つinstanceはlayoutを共有する() {
        let program = lowered_of(SOURCE);
        let mut plan = Plan::default();
        let fields = vec![(slot(&program, "clock"), struct_(&program, "Frozen"))];

        let first = plan.intern_layout(fields.clone());
        let second = plan.intern_layout(fields);
        assert_eq!(first, second);
        assert_eq!(plan.layouts().count(), 1);
    }

    /// 値要求が無ければ隠し ambient 引数を持たない(型提供は欄にならない)
    #[test]
    fn 値要求が無ければlayoutを持たない() {
        let mut plan = Plan::default();
        assert_eq!(plan.intern_layout(Vec::new()), None);
        assert_eq!(plan.layouts().count(), 0);
    }

    /// 欄は具体的な provider の型を持つ。handle の出どころは欄と提供式の2つ
    #[test]
    fn 描画は決定的でidを名前へ戻す() {
        let program = lowered_of(SOURCE);
        let mut plan = Plan::default();
        let clock = slot(&program, "clock");
        let layout = plan.intern_layout(vec![(clock, struct_(&program, "Frozen"))]);

        let (root, _) = plan.intern(key(&program, "main", &[]), None);
        let (used, _) = plan.intern(key(&program, "used", &[("clock", "Frozen")]), layout);
        plan.roots.push((body(&program, "main"), root));
        plan.instances[root.index()].calls.insert(
            hir::ExprId::from_index(3),
            PlannedCall {
                target: used,
                receiver: None,
                projection: vec![(clock, ValueSource::Provision(hir::ExprId::from_index(1)))],
            },
        );
        plan.instances[used.index()].calls.insert(
            hir::ExprId::from_index(0),
            PlannedCall {
                target: used,
                receiver: Some(ValueSource::Incoming(clock)),
                projection: Vec::new(),
            },
        );

        let rendered = plan.render(&program);
        assert_eq!(rendered, plan.render(&program), "同じ計画からは同じ文字列");
        assert_eq!(
            rendered,
            "root main -> instance#0\n\
             layout#0 { clock: Frozen }\n\
             instance#0 main [] ambient -\n\
             \x20 expr#3 -> instance#1 { clock <- provision expr#1 }\n\
             instance#1 used [clock=Frozen] ambient layout#0\n\
             \x20 expr#0 -> instance#1 self=field clock {}\n"
        );
    }
}
