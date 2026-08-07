//! ambient の実行時契約。要求推論の結果を、コンパイル可能な引数と直接呼び出しへ
//! 落とすための内部表現(design.md 決定11)。
//!
//! プログラムは whole-program で、提供される実装は全て HIR に載っている。だから
//! 到達した本体を「どの実装の組み合わせで呼ばれたか」ごとに複製すれば、スロット
//! 呼び出しは具体的な `CallableId` への直接呼び出しになり、vtable は要らない。
//!
//! ここが持つのは**特殊化の情報だけ**で、式の形は HIR のまま。後段の Wasm 生成は
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
use std::collections::{BTreeMap, BTreeSet};
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
    /// この呼び出しで選ばれた callback。同じ本体でも callback が違えば
    /// 要求も生成コードも別物になる(design.md 決定3)
    pub bindings: hir::Bindings,
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

    /// 予約済み instance の個数。backend は `InstanceId` を関数番号へ写す前に、
    /// 計画が範囲内を指していることを検証する。
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn layouts(&self) -> impl Iterator<Item = (RecordLayoutId, &RecordLayout)> {
        self.layouts
            .iter()
            .enumerate()
            .map(|(index, layout)| (RecordLayoutId::from_index(index), layout))
    }

    #[cfg(test)]
    pub(crate) fn instance_mut_for_test(&mut self, id: InstanceId) -> &mut Instance {
        &mut self.instances[id.index()]
    }

    #[cfg(test)]
    pub(crate) fn instance_id_for_test(index: usize) -> InstanceId {
        InstanceId::from_index(index)
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
    /// 間接呼び出しの呼び先が、この特殊化の callback 束縛から決まらない。
    /// 型検査が callable 値の置き場所を絞っているので通常は起きない
    UnresolvedCallback { body: hir::BodyId },
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
            PlanError::UnresolvedCallback { body } => format!(
                "{}: 間接呼び出しの呼び先が決まりません",
                program.show_body(*body)
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// 需要駆動の計画
// ---------------------------------------------------------------------------

/// いま何がどの実装で提供されているか。`with` の本体へ入るときだけ差し替える。
pub type ProviderContext = BTreeMap<hir::SlotId, ProviderBinding>;

/// エントリと全 test を根に、到達した本体を実装の組み合わせごとに単相化する。
///
/// 根は空の提供文脈から始まる。要求が残っていれば提供忘れだが、それは前段の
/// 診断が先に止めるので、ここへ来たら不変条件の破れとして返す。
pub fn plan(
    checked: &crate::ownership::CheckedProgram,
    analysis: &crate::requirement::Analysis,
    entry: hir::CallableId,
) -> Result<Plan, PlanError> {
    let program = &checked.hir;
    let roots: Vec<_> = std::iter::once(hir::BodyId::Callable(entry))
        .chain(program.tests.ids().map(hir::BodyId::Test))
        .map(|body| (body, ProviderContext::new()))
        .collect();
    plan_roots(program, analysis, &roots)
}

/// 生産ビルドの計画。根は `main` と、公開された関数だけ。
///
/// test は根に入らない。宣言されているだけの test が生産物を膨らませたり、
/// scalar 外の機能で生産ビルドを止めたりしないため(core-wasm-build spec)
#[derive(Debug)]
pub struct ProductionPlan {
    pub plan: Plan,
    /// エントリの instance
    pub entry: InstanceId,
    /// 公開名 → その根の instance。公開名の昇順。別名が同じ関数を指した
    /// ときは同じ instance が並ぶ(ラッパは名前ごと、実装は1つ)
    pub exports: Vec<(String, InstanceId)>,
}

/// `main` と公開関数それぞれを、空の提供文脈から独立に計画する。
///
/// `exports` は公開名の昇順で渡す。`main` の提供状態が後続の公開呼び出しへ
/// 引き継がれることはない
pub fn plan_production(
    checked: &crate::ownership::CheckedProgram,
    analysis: &crate::requirement::Analysis,
    entry: hir::CallableId,
    exports: &[(String, hir::CallableId)],
) -> Result<ProductionPlan, PlanError> {
    let program = &checked.hir;
    let mut seen = BTreeSet::from([entry]);
    let mut roots = vec![(hir::BodyId::Callable(entry), ProviderContext::new())];
    for (_, callable) in exports {
        if seen.insert(*callable) {
            roots.push((hir::BodyId::Callable(*callable), ProviderContext::new()));
        }
    }

    let plan = plan_roots(program, analysis, &roots)?;
    let instance_of: BTreeMap<hir::BodyId, InstanceId> = plan.roots.iter().copied().collect();
    Ok(ProductionPlan {
        entry: instance_of[&hir::BodyId::Callable(entry)],
        exports: exports
            .iter()
            .map(|(name, callable)| (name.clone(), instance_of[&hir::BodyId::Callable(*callable)]))
            .collect(),
        plan,
    })
}

/// 単相化アルゴリズムの単体テスト用。通常経路では使わない。
#[cfg(test)]
pub(crate) fn plan_hir_for_test(
    program: &hir::Program,
    analysis: &crate::requirement::Analysis,
    entry: hir::CallableId,
) -> Result<Plan, PlanError> {
    let roots: Vec<_> = std::iter::once(hir::BodyId::Callable(entry))
        .chain(program.tests.ids().map(hir::BodyId::Test))
        .map(|body| (body, ProviderContext::new()))
        .collect();
    plan_roots(program, analysis, &roots)
}

/// 生産根の単相化アルゴリズムの単体テスト用。通常経路では使わない。
#[cfg(test)]
pub(crate) fn plan_production_hir_for_test(
    program: &hir::Program,
    analysis: &crate::requirement::Analysis,
    entry: hir::CallableId,
    exports: &[(String, hir::CallableId)],
) -> Result<ProductionPlan, PlanError> {
    let mut seen = BTreeSet::from([entry]);
    let mut roots = vec![(hir::BodyId::Callable(entry), ProviderContext::new())];
    for (_, callable) in exports {
        if seen.insert(*callable) {
            roots.push((hir::BodyId::Callable(*callable), ProviderContext::new()));
        }
    }
    let plan = plan_roots(program, analysis, &roots)?;
    let instance_of: BTreeMap<hir::BodyId, InstanceId> = plan.roots.iter().copied().collect();
    Ok(ProductionPlan {
        entry: instance_of[&hir::BodyId::Callable(entry)],
        exports: exports
            .iter()
            .map(|(name, callable)| (name.clone(), instance_of[&hir::BodyId::Callable(*callable)]))
            .collect(),
        plan,
    })
}

/// 根の並びを呼び出し側が決める計画。
///
/// 根は与えられた順に確保されるので、instance の番号もその順で決まる。
/// 同じ本体を指す根が複数あっても intern が1つに寄せる(別名の再エクスポートが
/// 同じ実装を共有するのはこの性質)
fn plan_roots(
    program: &hir::Program,
    analysis: &crate::requirement::Analysis,
    roots: &[(hir::BodyId, ProviderContext)],
) -> Result<Plan, PlanError> {
    let mut planner = Planner {
        program,
        analysis,
        plan: Plan::default(),
        pending: std::collections::VecDeque::new(),
    };

    for (root, context) in roots {
        let (instance, _) = planner.request(*root, hir::Bindings::new(), context, *root)?;
        planner.plan.roots.push((*root, instance));
    }

    // 確保だけして中身が空の instance を、確保順に片づける
    while let Some((instance, context)) = planner.pending.pop_front() {
        let key = planner.plan.instance(instance).key.clone();
        let body = planner.program.body(key.body);
        let bindings = hir::resolve_bindings(body, &key.bindings);
        for root in &body.root {
            planner.walk(instance, body, *root, &context, &bindings)?;
        }
    }

    Ok(planner.plan)
}

struct Planner<'a> {
    program: &'a hir::Program,
    analysis: &'a crate::requirement::Analysis,
    plan: Plan,
    /// まだ本体を歩いていない instance と、その中での提供文脈
    pending: std::collections::VecDeque<(InstanceId, ProviderContext)>,
}

impl<'a> Planner<'a> {
    /// 呼び先の instance を要求する。
    ///
    /// 呼び先の**既存の要求**に文脈を制限してから鍵にする(design.md 決定4)。
    /// 選ばれた実装ならもっと少ないスロットで足りる場合でも、要求は保守的な
    /// ままにするので、使わない欄が record に残ることがある。
    ///
    /// 返る射影は呼び出し**元**から見た handle の出どころで、並びは呼び先の
    /// layout と同じ。
    fn request(
        &mut self,
        callee: hir::BodyId,
        bindings: hir::Bindings,
        caller: &ProviderContext,
        caller_body: hir::BodyId,
    ) -> Result<(InstanceId, Vec<(hir::SlotId, ValueSource)>), PlanError> {
        let mut providers = Vec::new();
        let mut fields = Vec::new();
        let mut projection = Vec::new();
        let mut inner = ProviderContext::new();

        // 要求は `SlotId` 順。layout・鍵・射影の並びはここで一度に決まる
        for (slot, requirement) in self.analysis.requirements(callee, &bindings) {
            let Some(binding) = caller.get(slot) else {
                return Err(PlanError::MissingProvider {
                    body: caller_body,
                    slot: *slot,
                });
            };
            providers.push((*slot, binding.implementation));
            let value = match requirement.level {
                // 型要求は鍵と呼び先を変えるだけ。実行時の欄は作らない
                crate::requirement::SlotLevel::Type => None,
                crate::requirement::SlotLevel::Value => {
                    let Some(source) = binding.value else {
                        return Err(PlanError::TypeOnlyProvider {
                            body: caller_body,
                            slot: *slot,
                        });
                    };
                    projection.push((*slot, source));
                    fields.push((
                        *slot,
                        self.program.trait_impls[binding.implementation].type_,
                    ));
                    // 呼び先の中では、その handle は自分の record の欄から来る
                    Some(ValueSource::Incoming(*slot))
                }
            };
            inner.insert(
                *slot,
                ProviderBinding {
                    implementation: binding.implementation,
                    value,
                },
            );
        }

        let layout = self.plan.intern_layout(fields);
        let (instance, fresh) = self.plan.intern(
            InstanceKey {
                body: callee,
                bindings,
                providers,
            },
            layout,
        );
        if fresh {
            self.pending.push_back((instance, inner));
        }
        Ok((instance, projection))
    }

    /// 式を1つ歩く。分岐・ループ・guard・arm・提供式は**全部**通る。
    /// 実行時にどれが選ばれるかは計画に関係しない(design.md 決定2)。
    fn walk(
        &mut self,
        instance: InstanceId,
        body: &'a hir::Body,
        id: hir::ExprId,
        context: &ProviderContext,
        bindings: &hir::Bindings,
    ) -> Result<(), PlanError> {
        let expr = body.expr(id);
        macro_rules! walk {
            ($child:expr) => {
                self.walk(instance, body, *$child, context, bindings)?
            };
        }
        match &expr.kind {
            // 所有権修飾は場所を包むだけなので、計画は変わらない。場所の中に
            // 呼び出しも提供も現れない(tasks 5.5)。提供の所有モード(共有・
            // 排他・move・一時所有)は所有権解析が閉じるもので、provider の
            // 選択にも slot/callable の同一性にも効かない。所有か借用かを
            // record の欄に載せるのはデータ下ろしの段(design.md 決定11)
            hir::ExprKind::Access { place, .. } => walk!(place),

            // 組み込みの `push` は葉。呼び出し先も提供も持たないので、計画に
            // 足すのは部分式だけ(MAP-075 決定1)
            hir::ExprKind::Push { array, value } => {
                walk!(array);
                walk!(value);
            }

            hir::ExprKind::With {
                provisions,
                body: inner,
            } => {
                // 提供値は全て外側の文脈で評価してから、まとめて内側の写しへ
                // 置く。同じ `with` の提供は互いを見ない(design.md 決定9)
                for provision in provisions {
                    if let Some(value) = &provision.value {
                        walk!(value);
                    }
                }
                let mut replaced = context.clone();
                for provision in provisions {
                    replaced.insert(
                        provision.slot,
                        ProviderBinding {
                            implementation: provision.implementation,
                            value: provision.value.map(ValueSource::Provision),
                        },
                    );
                }
                self.walk(instance, body, *inner, &replaced, bindings)?;
            }

            hir::ExprKind::Call(call) => {
                self.plan_call(instance, id, call, context, bindings)?;
                if let hir::Call::Method { recv, .. } = call {
                    walk!(recv);
                }
                for arg in call_args(call) {
                    walk!(arg);
                }
            }

            hir::ExprKind::Match { subject, arms } => {
                walk!(subject);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        walk!(guard);
                    }
                    walk!(&arm.body);
                }
            }

            hir::ExprKind::Int(_)
            | hir::ExprKind::Str(_)
            | hir::ExprKind::Bool(_)
            | hir::ExprKind::Nil
            | hir::ExprKind::Local(_)
            | hir::ExprKind::UnitStruct(_)
            | hir::ExprKind::Variant(_)
            | hir::ExprKind::Function(_)
            | hir::ExprKind::Return(None)
            | hir::ExprKind::Poison => {}

            hir::ExprKind::Field { recv, .. } => walk!(recv),
            hir::ExprKind::StructLit { fields, .. } => {
                for (_, value) in fields {
                    walk!(value);
                }
            }
            hir::ExprKind::Array(items) | hir::ExprKind::Block(items) => {
                for item in items {
                    walk!(item);
                }
            }
            hir::ExprKind::Let { value, .. } | hir::ExprKind::AssignLocal { value, .. } => {
                walk!(value)
            }
            hir::ExprKind::AssignField { recv, value, .. } => {
                walk!(recv);
                walk!(value);
            }
            hir::ExprKind::Neg(inner)
            | hir::ExprKind::Assert(inner)
            | hir::ExprKind::Clone(inner)
            | hir::ExprKind::Return(Some(inner)) => walk!(inner),
            hir::ExprKind::Arith { lhs, rhs, .. }
            | hir::ExprKind::Eq { lhs, rhs }
            | hir::ExprKind::Coalesce { lhs, rhs } => {
                walk!(lhs);
                walk!(rhs);
            }
            hir::ExprKind::If { cond, then, orelse } => {
                walk!(cond);
                walk!(then);
                if let Some(orelse) = orelse {
                    walk!(orelse);
                }
            }
            hir::ExprKind::While { cond, body: inner } => {
                walk!(cond);
                walk!(inner);
            }
            hir::ExprKind::For {
                iter, body: inner, ..
            } => {
                walk!(iter);
                walk!(inner);
            }
        }
        Ok(())
    }

    /// 呼び出し1つを、具体的な `CallableId` の instance への辺にする。
    ///
    /// スロット呼び出しも例外ではない。提供が持つ `TraitImplId` から実装本体を
    /// 引くので、実行時の分岐表は要らない(design.md 決定8)。
    fn plan_call(
        &mut self,
        instance: InstanceId,
        id: hir::ExprId,
        call: &hir::Call,
        context: &ProviderContext,
        bindings: &hir::Bindings,
    ) -> Result<(), PlanError> {
        let body = self.program.body(self.plan.instance(instance).key.body);
        let caller = self.plan.instance(instance).key.body;
        let (callee, receiver) = match call {
            hir::Call::Direct { callable, .. }
            | hir::Call::Associated { callable, .. }
            | hir::Call::Method { callable, .. } => (*callable, None),
            // 間接呼び出しは、この特殊化で選ばれている名前付き関数へ直に向かう
            hir::Call::Indirect { callee, .. } => match hir::callable_of(body, *callee, bindings) {
                Some(callable) => (callable, None),
                None => return Err(PlanError::UnresolvedCallback { body: caller }),
            },
            hir::Call::Slot {
                slot,
                method,
                receiver,
                ..
            } => {
                let Some(binding) = context.get(slot) else {
                    return Err(PlanError::MissingProvider {
                        body: caller,
                        slot: *slot,
                    });
                };
                let Some(callable) = self
                    .program
                    .implementation_of(binding.implementation, *method)
                else {
                    return Err(PlanError::UnimplementedMethod {
                        implementation: binding.implementation,
                        method: *method,
                    });
                };
                // 値射影は provider の実体を `self` に渡す。型射影は渡さない
                let receiver = match receiver {
                    hir::SlotReceiver::Value => match binding.value {
                        Some(source) => Some(source),
                        None => {
                            return Err(PlanError::TypeOnlyProvider {
                                body: caller,
                                slot: *slot,
                            });
                        }
                    },
                    hir::SlotReceiver::Type => None,
                };
                (callable, receiver)
            }
            // 構築は本体を持たない
            hir::Call::Ctor { .. } => return Ok(()),
        };

        let inner = hir::callee_bindings(self.program, body, callee, call_args(call), bindings);
        let (target, projection) =
            self.request(hir::BodyId::Callable(callee), inner, context, caller)?;
        self.plan.instances[instance.index()].calls.insert(
            id,
            PlannedCall {
                target,
                receiver,
                projection,
            },
        );
        Ok(())
    }
}

fn call_args(call: &hir::Call) -> &[hir::ExprId] {
    match call {
        hir::Call::Direct { args, .. }
        | hir::Call::Associated { args, .. }
        | hir::Call::Method { args, .. }
        | hir::Call::Slot { args, .. }
        | hir::Call::Indirect { args, .. }
        | hir::Call::Ctor { args, .. } => args,
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

    /// provider mode まで検査済みの HIR。計画 API 自体を `CheckedProgram` に
    /// 切り替えるのは task 8.1 なので、ここではその前段で確定した mode が
    /// 既存の ambient 計画の同一性を動かさないことだけを確認する。
    fn ownership_checked_of(src: &str) -> hir::Program {
        crate::ownership::check(lowered_of(src))
            .expect("所有権検査を通るはず")
            .hir
    }

    // ---- callback 特殊化の計画(tasks 3.3 / 3.4) ----

    const CALLBACK_SRC: &str = "fn ticked(value: int -> int) { value + clock.now() }\n\
         fn plain(value: int -> int) { value + 1 }\n\
         fn apply(f: fn(int -> int), value: int -> int) { f(value) }\n";

    /// 名前で instance を全部拾い、それぞれの ambient layout の有無を返す
    fn callback_instances(program: &hir::Program, plan: &Plan, name: &str) -> Vec<bool> {
        plan.instances()
            .filter(|(_, instance)| program.show_body(instance.key.body).ends_with(name))
            .map(|(_, instance)| instance.layout.is_some())
            .collect()
    }

    /// slot を要る callback と要らない callback で、`apply` の instance が割れる。
    /// no-slot 側は ambient record を持たない
    #[test]
    fn callbackごとにapplyのinstanceが分かれる() {
        let (program, plan) = plan_of(&format!(
            "{CALLBACK_SRC}fn main(-> int) {{\n\
             \x20 let quiet = apply(plain, 1)\n\
             \x20 with clock(Frozen {{ t = 1000 }}) {{ apply(ticked, quiet) }}\n\
             }}\n"
        ));
        let mut ambient = callback_instances(&program, &plan, "apply");
        ambient.sort_unstable();
        assert_eq!(ambient, vec![false, true]);
    }

    /// 同じ callback・同じ provider なら instance は1つに畳まれる
    #[test]
    fn 同じcallbackの計画は1つに畳まれる() {
        let (program, plan) = plan_of(&format!(
            "{CALLBACK_SRC}fn main(-> int) {{\n\
             \x20 with clock(Frozen {{ t = 1 }}) {{ apply(ticked, 1) + apply(ticked, 2) }}\n\
             }}\n"
        ));
        assert_eq!(callback_instances(&program, &plan, "apply").len(), 1);
    }

    /// provider が違えば、同じ callback でも instance は分かれる(既存の規則)
    #[test]
    fn callbackが同じでもproviderが違えば分かれる() {
        let (program, plan) = plan_of(&format!(
            "{CALLBACK_SRC}fn main(-> int) {{\n\
             \x20 let a = with clock(Frozen {{ t = 1 }}) {{ apply(ticked, 1) }}\n\
             \x20 let b = with clock(Zero {{}}) {{ apply(ticked, 2) }}\n\
             \x20 a + b\n\
             }}\n"
        ));
        assert_eq!(callback_instances(&program, &plan, "apply").len(), 2);
    }

    /// 間接呼び出しにも `PlannedCall` が付き、行き先は選ばれた callback
    #[test]
    fn 間接呼び出しは選ばれたcallbackへ向かう() {
        let (program, plan) = plan_of(&format!(
            "{CALLBACK_SRC}fn main(-> int) {{ apply(plain, 1) }}\n"
        ));
        let targets: BTreeSet<String> = plan
            .instances()
            .filter(|(_, instance)| program.show_body(instance.key.body).ends_with("apply"))
            .flat_map(|(_, instance)| instance.calls.values())
            .map(|call| program.show_body(plan.instance(call.target).key.body))
            .collect();
        assert!(
            targets.iter().any(|name| name.ends_with("plain")),
            "{targets:?}"
        );
    }

    /// 提供忘れは計画の前段(要求解析)で止まる。ここでは計画が通らないこと
    #[test]
    fn callback越しの提供忘れは計画で落ちる() {
        let program = lowered_of(&format!(
            "{CALLBACK_SRC}fn main(-> int) {{ apply(ticked, 1) }}\n"
        ));
        let analysis = crate::requirement::analyze_hir_for_test(&program);
        let entry = program.free_callable("main").expect("main がない");
        let clock = slot(&program, "clock");
        assert_eq!(
            plan_hir_for_test(&program, &analysis, entry),
            Err(PlanError::MissingProvider {
                body: hir::BodyId::Callable(entry),
                slot: clock
            })
        );
    }

    // ---- generic な具体化の計画(MAP-050 tasks 2.x) ----

    /// 上と同じ形の helper を generic にしたもの。具体化は宣言名をそのまま
    /// 名乗るので、instance は表示名ではなく鍵で言い分かれる
    const GENERIC_CALLBACK_SRC: &str = "fn ticked(value: int -> int) { value + clock.now() }\n\
         fn plain(value: int -> int) { value + 1 }\n\
         fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }\n";

    /// slot を要る callback と要らない callback で、generic な `apply` の
    /// 具体化ごとに instance が割れる。no-slot 側は record も射影も持たない
    #[test]
    fn generic_の具体化ごとにinstanceが分かれる() {
        let (program, plan) = plan_of(&format!(
            "{GENERIC_CALLBACK_SRC}fn main(-> int) {{\n\
             \x20 let quiet = apply(plain, 1)\n\
             \x20 with clock(Frozen {{ t = 1000 }}) {{ apply(ticked, quiet) }}\n\
             }}\n"
        ));
        let mut ambient = callback_instances(&program, &plan, "apply");
        ambient.sort_unstable();
        assert_eq!(ambient, vec![false, true]);

        let quiet: Vec<&Instance> = plan
            .instances()
            .map(|(_, instance)| instance)
            .filter(|instance| {
                program.show_body(instance.key.body) == "apply" && instance.layout.is_none()
            })
            .collect();
        assert_eq!(quiet.len(), 1, "callback-free の具体化が1つ");
        assert!(quiet[0].key.providers.is_empty(), "{:?}", quiet[0].key);
        assert!(
            quiet[0]
                .calls
                .values()
                .all(|call| call.projection.is_empty()),
            "{:?}",
            quiet[0].calls
        );
    }

    /// 同じ具体化・同じ provider への2つの呼び出しは1つの instance へ寄る
    #[test]
    fn 同じgeneric具体化への呼び出しは1つのinstanceに畳まれる() {
        let (program, plan) = plan_of(&format!(
            "{GENERIC_CALLBACK_SRC}fn main(-> int) {{\n\
             \x20 with clock(Frozen {{ t = 1 }}) {{ apply(ticked, 1) + apply(ticked, 2) }}\n\
             }}\n"
        ));
        assert_eq!(callback_instances(&program, &plan, "apply").len(), 1);
    }

    /// generic な具体化へ届く要求の提供忘れも、非 generic と同じ形で落ちる
    #[test]
    fn generic越しの提供忘れは計画で落ちる() {
        let program = lowered_of(&format!(
            "{GENERIC_CALLBACK_SRC}fn main(-> int) {{ apply(ticked, 1) }}\n"
        ));
        let analysis = crate::requirement::analyze_hir_for_test(&program);
        let entry = program.free_callable("main").expect("main がない");
        let clock = slot(&program, "clock");
        assert_eq!(
            plan_hir_for_test(&program, &analysis, entry),
            Err(PlanError::MissingProvider {
                body: hir::BodyId::Callable(entry),
                slot: clock
            })
        );
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

    /// 前置き付きで下ろして計画する。エントリは `main`
    pub(super) fn plan_of(src: &str) -> (hir::Program, Plan) {
        let program = lowered_of(src);
        let plan = plan_program(&program);
        (program, plan)
    }

    fn plan_program(program: &hir::Program) -> Plan {
        let analysis = crate::requirement::analyze_hir_for_test(program);
        let entry = program.free_callable("main").expect("main がない");
        plan_hir_for_test(program, &analysis, entry).expect("計画できるはず")
    }

    /// 前置きを付けずに下ろして計画する(正典など完結したソース用)
    pub(super) fn plan_source(src: &str) -> (hir::Program, Plan) {
        let parsed = crate::parse::parse(&crate::lex::join(crate::lex::lex(src).unwrap()))
            .expect("パースできるはず");
        let program = crate::typecheck::check_and_lower(&parsed).expect("型検査を通るはず");
        let plan = plan_program(&program);
        (program, plan)
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
            bindings: hir::Bindings::new(),
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

    // ---- 需要駆動の計画 ----

    /// 表示名で instance を数える。特殊化されていれば同じ名前が複数出る
    fn instances_named(program: &hir::Program, plan: &Plan, name: &str) -> Vec<InstanceId> {
        plan.instances()
            .filter(|(_, instance)| program.show_body(instance.key.body) == name)
            .map(|(id, _)| id)
            .collect()
    }

    /// 根はエントリと全 test。どれも空の提供文脈から始まるので隠し引数を持たない
    #[test]
    fn 根はエントリと全testになる() {
        let (program, plan) = plan_of(
            "fn main(-> int) { 0 }\n\
             test \"a\" { assert true }\n\
             test \"b\" { assert true }\n",
        );

        assert_eq!(plan.roots.len(), 3);
        assert_eq!(program.show_body(plan.roots[0].0), "main");
        for (_, instance) in &plan.roots {
            assert_eq!(
                plan.instance(*instance).layout,
                None,
                "根は record を持たない"
            );
        }
    }

    /// 到達しない宣言は型検査を通るが instance を持たない
    #[test]
    fn 到達しない関数は計画に出ない() {
        let (program, plan) = plan_of(
            "fn reached(-> int) { 1 }\n\
             fn unreached(-> int) { clock.now() }\n\
             fn main(-> int) { reached() }\n",
        );

        assert_eq!(instances_named(&program, &plan, "reached").len(), 1);
        assert!(instances_named(&program, &plan, "unreached").is_empty());
    }

    /// 呼び出し元が持っていても、呼び先の要求に無いスロットは鍵に入らない。
    /// 無関係なスロットで instance が増えないのはこの制限のため
    #[test]
    fn 呼び出し元の無関係なスロットは鍵に入らない() {
        let (program, plan) = plan_of(
            "fn ticks(-> int) { clock.now() }\n\
             fn main(-> int) {\n\
             \x20 with clock(Frozen { t = 1 }), db(InMemoryDb {}) { ticks() }\n\
             }\n",
        );

        let ticks = instances_named(&program, &plan, "ticks");
        assert_eq!(ticks.len(), 1);
        let key = &plan.instance(ticks[0]).key;
        assert_eq!(key.providers.len(), 1, "clock だけが鍵に入る");
        assert_eq!(key.providers[0].0, slot(&program, "clock"));
    }

    /// 実行時の値が違っても実装が同じなら同じコードを共有する。
    /// 違うのは record に載る handle だけ(design.md 決定3)
    #[test]
    fn 実行時の値が違っても同じinstanceを共有する() {
        let (program, plan) = plan_of(
            "fn ticks(-> int) { clock.now() }\n\
             fn main(-> int) {\n\
             \x20 let first = with clock(Frozen { t = 1 }) { ticks() }\n\
             \x20 let second = with clock(Frozen { t = 2 }) { ticks() }\n\
             \x20 first + second\n\
             }\n",
        );

        assert_eq!(instances_named(&program, &plan, "ticks").len(), 1);
        let root = plan.instance(plan.roots[0].1);
        let sources: Vec<ValueSource> = root
            .calls
            .values()
            .map(|call| call.projection[0].1)
            .collect();
        assert_eq!(sources.len(), 2);
        assert_ne!(
            sources[0], sources[1],
            "handle の出どころは呼び出しごとに違う"
        );
    }

    /// 実行時にどの枝が走るかは計画に関係しない。分岐・guard・arm・ループの
    /// 中の呼び出しも全部 instance を作る
    #[test]
    fn 全ての枝を保守的に歩く() {
        let (program, plan) = plan_of(
            "enum Pick { A B }\n\
             fn in_then(-> int) { 1 }\n\
             fn in_else(-> int) { 2 }\n\
             fn in_guard(-> bool) { true }\n\
             fn in_arm(-> int) { 3 }\n\
             fn in_loop(-> int) { 4 }\n\
             fn main(p: Pick -> int) {\n\
             \x20 let n = if true: in_then() else: in_else()\n\
             \x20 while false { in_loop() }\n\
             \x20 match p {\n\
             \x20   Pick::A if in_guard(): in_arm()\n\
             \x20   _: n\n\
             \x20 }\n\
             }\n",
        );

        for name in ["in_then", "in_else", "in_guard", "in_arm", "in_loop"] {
            assert_eq!(
                instances_named(&program, &plan, name).len(),
                1,
                "{name} が計画に出ていない"
            );
        }
    }

    /// 合成した提供文脈が壊れていたら、部分的な計画を作らずに構造化して返す
    #[test]
    fn 要求を満たさない合成文脈は構造化エラーになる() {
        let program = lowered_of(
            "fn ticks(-> int) { clock.now() }\n\
             fn main(-> int) { 0 }\n",
        );
        let analysis = crate::requirement::analyze_hir_for_test(&program);
        let ticks = hir::BodyId::Callable(program.free_callable("ticks").unwrap());
        let clock = slot(&program, "clock");

        let mut planner = Planner {
            program: &program,
            analysis: &analysis,
            plan: Plan::default(),
            pending: std::collections::VecDeque::new(),
        };

        let empty = ProviderContext::new();
        assert_eq!(
            planner.request(ticks, hir::Bindings::new(), &empty, ticks),
            Err(PlanError::MissingProvider {
                body: ticks,
                slot: clock
            })
        );

        // 型提供だけでは値要求を満たせない。逆は満たせる(決定5)
        let mut type_only = ProviderContext::new();
        type_only.insert(
            clock,
            ProviderBinding {
                implementation: implementation(&program, "Frozen"),
                value: None,
            },
        );
        assert_eq!(
            planner.request(ticks, hir::Bindings::new(), &type_only, ticks),
            Err(PlanError::TypeOnlyProvider {
                body: ticks,
                slot: clock
            })
        );
        assert_eq!(
            planner.plan.instances().count(),
            0,
            "壊れた文脈で instance を作らない"
        );
    }

    // ---- 提供・スロット呼び出し・`with` ----

    /// instance の実装選択を読める形で
    fn providers_of(program: &hir::Program, plan: &Plan, id: InstanceId) -> Vec<String> {
        plan.instance(id)
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
            .collect()
    }

    fn only(program: &hir::Program, plan: &Plan, name: &str) -> InstanceId {
        let found = instances_named(program, plan, name);
        assert_eq!(found.len(), 1, "{name} の instance が1つではない");
        found[0]
    }

    /// provider の mode は loan/drop の事実であり、specialization の鍵と
    /// record layout は従来どおり callable / slot / implementation ID だけで
    /// 決まる。値を作った expr ID は各 mode で異なり得るため比較しない(tasks 5.5)。
    #[test]
    fn provider_modeはambientのcanonical_plan_factを変えない() {
        let variants = [
            "let store = SharedFrozen { t = 1 }\n with shared_clock(store) { ticks() }",
            "let mut store = SharedFrozen { t = 1 }\n with shared_clock(&mut store) { ticks() }",
            "let store = SharedFrozen { t = 1 }\n with shared_clock(move store) { ticks() }",
            "with shared_clock(SharedFrozen { t = 1 }) { ticks() }",
        ];
        let mut expected = None;
        for provision in variants {
            let program = ownership_checked_of(&format!(
                "trait SharedClock {{ fn now(&self -> int) }}\n\
                 struct SharedFrozen {{ t: int }}\n\
                 impl SharedClock for SharedFrozen {{ fn now(&self -> int) {{ self.t }} }}\n\
                 effect shared_clock: SharedClock\n\
                 fn ticks(-> int) {{ shared_clock.now() }}\n\
                 fn main(-> int) {{ {provision} }}\n"
            ));
            let analysis = crate::requirement::analyze_hir_for_test(&program);
            let entry = program.free_callable("main").unwrap();
            let plan = plan_hir_for_test(&program, &analysis, entry).expect("計画できるはず");
            let ticks = only(&program, &plan, "ticks");
            let layout = plan.layout(plan.instance(ticks).layout.expect("値要求の欄"));
            let facts = (
                plan.instance(ticks).key.clone(),
                layout.fields.clone(),
                providers_of(&program, &plan, ticks),
                plan.instances().count(),
            );
            if let Some(expected) = &expected {
                assert_eq!(
                    &facts, expected,
                    "provider mode must not alter ambient facts"
                );
            } else {
                expected = Some(facts);
            }
        }
    }

    /// 型提供は鍵と呼び先を変えるが、実行時の欄は作らない(決定6)
    #[test]
    fn 型提供は隠し引数を作らない() {
        let (program, plan) = plan_of(
            "fn zeroed(-> int) { clock::zero() }\n\
             fn main(-> int) { with clock<Frozen> { zeroed() } }\n",
        );

        let zeroed = only(&program, &plan, "zeroed");
        assert_eq!(providers_of(&program, &plan, zeroed), vec!["clock=Frozen"]);
        assert_eq!(plan.instance(zeroed).layout, None, "型提供は運ばない");

        // 型射影の呼び出しはレシーバを渡さず、実装本体を直接指す
        let call = plan
            .instance(zeroed)
            .calls
            .values()
            .next()
            .expect("呼び出し");
        assert_eq!(call.receiver, None);
        assert_eq!(
            program.show_body(plan.instance(call.target).key.body),
            "impl Frozen::zero"
        );
    }

    /// 値提供は具体型の handle を1欄だけ載せ、値射影はそれを `self` に渡す
    #[test]
    fn 値提供はhandleの欄を作りselfに渡す() {
        let (program, plan) = plan_of(
            "fn ticks(-> int) { clock.now() }\n\
             fn main(-> int) { with clock(Frozen { t = 1 }) { ticks() } }\n",
        );

        let ticks = only(&program, &plan, "ticks");
        let layout = plan.layout(plan.instance(ticks).layout.expect("record を持つ"));
        assert_eq!(
            layout.fields,
            vec![(slot(&program, "clock"), struct_(&program, "Frozen"))]
        );

        let call = plan
            .instance(ticks)
            .calls
            .values()
            .next()
            .expect("呼び出し");
        assert_eq!(
            call.receiver,
            Some(ValueSource::Incoming(slot(&program, "clock"))),
            "provider の実体をレシーバとして渡す"
        );
        assert_eq!(
            program.show_body(plan.instance(call.target).key.body),
            "impl Frozen::now",
            "vtable ではなく実装本体への直接呼び出し"
        );
    }

    /// 値提供は型要求も満たす。ただし型要求は欄にならないので射影は空になる
    #[test]
    fn 値提供は型要求を満たすが欄にはならない() {
        let (program, plan) = plan_of(
            "fn zeroed(-> int) { clock::zero() }\n\
             fn main(-> int) { with clock(Frozen { t = 1 }) { zeroed() } }\n",
        );

        let zeroed = only(&program, &plan, "zeroed");
        assert_eq!(providers_of(&program, &plan, zeroed), vec!["clock=Frozen"]);
        assert_eq!(plan.instance(zeroed).layout, None);

        let root = plan.instance(plan.roots[0].1);
        let call = root.calls.values().next().expect("呼び出し");
        assert!(call.projection.is_empty(), "型要求に record は要らない");
    }

    /// 内側の `with` は外側の record を書き換えずに置き換える。ブロックを抜ければ
    /// 外側の選択が戻る(決定9)
    #[test]
    fn 入れ子のwithは置換であって書き換えではない() {
        let (program, plan) = plan_of(
            "fn ticks(-> int) { clock.now() }\n\
             fn main(-> int) {\n\
             \x20 with clock(Frozen { t = 1 }) {\n\
             \x20   let inner = with clock(Zero {}) { ticks() }\n\
             \x20   inner + ticks()\n\
             \x20 }\n\
             }\n",
        );

        let mut selections: Vec<Vec<String>> = instances_named(&program, &plan, "ticks")
            .into_iter()
            .map(|id| providers_of(&program, &plan, id))
            .collect();
        selections.sort();
        assert_eq!(
            selections,
            vec![
                vec!["clock=Frozen".to_string()],
                vec!["clock=Zero".to_string()]
            ],
            "内側と外側で別の instance になる"
        );
    }

    /// 同じ `with` の提供は互いを見ない。提供式は全て外側の文脈で評価する
    #[test]
    fn 提供式は外側の提供で計画される() {
        let (program, plan) = plan_of(
            "fn make(-> InMemoryDb) {\n\
             \x20 let at = clock.now()\n\
             \x20 InMemoryDb {}\n\
             }\n\
             fn saves(-> unit) { db.save(1) }\n\
             fn main(-> unit) {\n\
             \x20 with clock(Frozen { t = 1 }) {\n\
             \x20   with clock(Zero {}), db(make()) { saves() }\n\
             \x20 }\n\
             }\n",
        );

        let make = only(&program, &plan, "make");
        assert_eq!(
            providers_of(&program, &plan, make),
            vec!["clock=Frozen"],
            "同じ with の clock(Zero) も、内側の提供も見えない"
        );

        let saves = only(&program, &plan, "saves");
        assert_eq!(
            providers_of(&program, &plan, saves),
            vec!["db=InMemoryDb"],
            "提供は全部まとめて内側に効く"
        );
    }

    /// 提供された値の handle は、`with` の提供式から呼び先の record へ渡る
    #[test]
    fn 提供値のhandleは提供式から呼び先の欄へ渡る() {
        let (program, plan) = plan_of(
            "fn ticks(-> int) { clock.now() }\n\
             fn main(-> int) { with clock(Frozen { t = 1 }) { ticks() } }\n",
        );

        let root = plan.instance(plan.roots[0].1);
        let call = root
            .calls
            .values()
            .find(|call| program.show_body(plan.instance(call.target).key.body) == "ticks")
            .expect("ticks の呼び出し");
        assert_eq!(call.projection.len(), 1);
        assert!(
            matches!(call.projection[0].1, ValueSource::Provision(_)),
            "根は提供式から取る"
        );

        // 呼び先の中では、同じ handle が自分の record の欄から来る
        let ticks = plan.instance(only(&program, &plan, "ticks"));
        let inner = ticks.calls.values().next().expect("呼び出し");
        assert_eq!(
            inner.receiver,
            Some(ValueSource::Incoming(slot(&program, "clock")))
        );
    }

    // ---- 再帰と完全な計画 ----

    /// 直接再帰は自分自身を指す。鍵を歩く前に確保しているので止まる(決定10)
    #[test]
    fn 直接再帰は同じinstanceを指す() {
        let (program, plan) = plan_of(
            "fn ping(n: int -> int) { if n == 0: clock.now() else: ping(n - 1) }\n\
             fn main(-> int) { with clock(Frozen { t = 1 }) { ping(1) } }\n",
        );

        let ping = only(&program, &plan, "ping");
        let recursive = plan
            .instance(ping)
            .calls
            .values()
            .find(|call| call.target == ping)
            .expect("自分自身への辺");
        assert_eq!(
            recursive.projection,
            vec![(
                slot(&program, "clock"),
                ValueSource::Incoming(slot(&program, "clock"))
            )],
            "受け取った record をそのまま渡す"
        );
    }

    /// 相互再帰も同じ仕掛けで閉じる。別の呼び出し規約は要らない
    #[test]
    fn 相互再帰は有限で閉じる() {
        let (program, plan) = plan_of(
            "fn ping(n: int -> int) { if n == 0: clock.now() else: pong(n - 1) }\n\
             fn pong(n: int -> int) { ping(n) }\n\
             fn main(-> int) { with clock(Frozen { t = 1 }) { ping(2) } }\n",
        );

        let ping = only(&program, &plan, "ping");
        let pong = only(&program, &plan, "pong");
        assert!(plan.instance(ping).calls.values().any(|c| c.target == pong));
        assert!(plan.instance(pong).calls.values().any(|c| c.target == ping));
    }

    /// 入れ子の提供が実装を変えると別の instance になるが、組み合わせは有限
    #[test]
    fn 提供が変わる再帰も有限のinstanceに収束する() {
        let (program, plan) = plan_of(
            "fn spin(n: int -> int) {\n\
             \x20 if n == 0: clock.now() else: with clock(Zero {}) { spin(n - 1) }\n\
             }\n\
             fn main(-> int) { with clock(Frozen { t = 1 }) { spin(3) } }\n",
        );

        let mut selections: Vec<Vec<String>> = instances_named(&program, &plan, "spin")
            .into_iter()
            .map(|id| providers_of(&program, &plan, id))
            .collect();
        selections.sort();
        assert_eq!(
            selections,
            vec![
                vec!["clock=Frozen".to_string()],
                vec!["clock=Zero".to_string()]
            ],
            "Zero の中の Zero は同じ鍵なので増えない"
        );
    }

    /// 別の根から同じ組み合わせで届いた本体は1つに寄る
    #[test]
    fn 根をまたいでも同じ鍵は共有する() {
        let (program, plan) = plan_of(
            "fn ticks(-> int) { clock.now() }\n\
             fn main(-> int) { with clock(Frozen { t = 1 }) { ticks() } }\n\
             test \"同じ提供\" { with clock(Frozen { t = 2 }) { assert ticks() == 2 } }\n",
        );

        assert_eq!(plan.roots.len(), 2);
        assert_eq!(
            instances_named(&program, &plan, "ticks").len(),
            1,
            "実装が同じなら値が違っても共有する"
        );
    }

    /// 要求は保守的なまま(決定4)。選ばれた実装が使わないスロットでも、
    /// 呼び出し元の record には欄が残る。ここを勝手に削ると言語の振る舞いが変わる
    #[test]
    fn 選ばれた実装が使わない欄も残る() {
        let (program, plan) = plan_of(
            "struct Logged {}\n\
             impl Database for Logged {\n\
             \x20 fn save(self, n: int -> unit) { let at = clock.now() }\n\
             }\n\
             fn saves(-> unit) { db.save(1) }\n\
             fn main(-> unit) {\n\
             \x20 with db(InMemoryDb {}), clock(Frozen { t = 1 }) { saves() }\n\
             }\n",
        );

        let saves = only(&program, &plan, "saves");
        let layout = plan.layout(plan.instance(saves).layout.expect("record を持つ"));
        assert_eq!(
            layout.fields,
            vec![
                (slot(&program, "clock"), struct_(&program, "Frozen")),
                (slot(&program, "db"), struct_(&program, "InMemoryDb")),
            ],
            "clock を使わない InMemoryDb を選んでも欄は残る"
        );

        // 呼び先の具体実装は clock を要求しないので、その record は空になる
        let call = plan
            .instance(saves)
            .calls
            .values()
            .next()
            .expect("呼び出し");
        assert_eq!(
            program.show_body(plan.instance(call.target).key.body),
            "impl InMemoryDb::save"
        );
        assert_eq!(plan.instance(call.target).layout, None);
        assert!(call.projection.is_empty());
    }

    /// 正典プログラム全体の計画。本番とテストの提供の組み合わせ、具体的な
    /// スロット呼び出し先、record layout、射影、型消去が1枚に出る
    #[test]
    fn 正典の計画は決定的な全体像になる() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let (program, plan) = plan_source(&src);

        assert_eq!(
            plan.render(&program),
            "root main -> instance#0\n\
             root test \"昇格すると Gold になり時刻が刻まれる\" -> instance#1\n\
             layout#0 { db: Postgres, clock: SystemClock }\n\
             layout#1 { db: InMemoryDb, clock: Frozen }\n\
             layout#2 { clock: SystemClock }\n\
             layout#3 { clock: Frozen }\n\
             instance#0 main [] ambient -\n\
             \x20 expr#3 -> instance#2 {}\n\
             \x20 expr#6 -> instance#3 { db <- provision expr#3, clock <- provision expr#4 }\n\
             instance#1 test \"昇格すると Gold になり時刻が刻まれる\" [] ambient -\n\
             \x20 expr#7 -> instance#4 {}\n\
             \x20 expr#12 -> instance#5 {}\n\
             \x20 expr#14 -> instance#6 { db <- provision expr#10, clock <- provision expr#12 }\n\
             \x20 expr#21 -> instance#7 {}\n\
             instance#2 impl Postgres::new [] ambient -\n\
             instance#3 handle [db=Postgres, clock=SystemClock] ambient layout#0\n\
             \x20 expr#1 -> instance#8 { db <- field db, clock <- field clock }\n\
             instance#4 impl InMemoryDb::new [] ambient -\n\
             instance#5 impl Frozen::at [] ambient -\n\
             instance#6 handle [db=InMemoryDb, clock=Frozen] ambient layout#1\n\
             \x20 expr#1 -> instance#9 { db <- field db, clock <- field clock }\n\
             instance#7 impl InMemoryDb::get [] ambient -\n\
             instance#8 promote [db=Postgres, clock=SystemClock] ambient layout#0\n\
             \x20 expr#1 -> instance#10 self=field db {}\n\
             \x20 expr#11 -> instance#11 { clock <- field clock }\n\
             \x20 expr#14 -> instance#12 self=field db {}\n\
             instance#9 promote [db=InMemoryDb, clock=Frozen] ambient layout#1\n\
             \x20 expr#1 -> instance#13 self=field db {}\n\
             \x20 expr#11 -> instance#14 { clock <- field clock }\n\
             \x20 expr#14 -> instance#15 self=field db {}\n\
             instance#10 impl Postgres::find [] ambient -\n\
             instance#11 stamp [clock=SystemClock] ambient layout#2\n\
             \x20 expr#1 -> instance#16 self=field clock {}\n\
             instance#12 impl Postgres::save [] ambient -\n\
             instance#13 impl InMemoryDb::find [] ambient -\n\
             instance#14 stamp [clock=Frozen] ambient layout#3\n\
             \x20 expr#1 -> instance#17 self=field clock {}\n\
             instance#15 impl InMemoryDb::save [] ambient -\n\
             instance#16 impl SystemClock::now [] ambient -\n\
             instance#17 impl Frozen::now [] ambient -\n"
        );
    }

    // -----------------------------------------------------------------------
    // 生産の根 (`main` + 公開関数)
    // -----------------------------------------------------------------------

    /// `公開名=関数名` で別名を書ける。別名を書かなければ両方同じ綴り
    fn production(src: &str, exports: &[&str]) -> (hir::Program, ProductionPlan) {
        let program = lowered_of(src);
        let analysis = crate::requirement::analyze_hir_for_test(&program);
        let entry = program.free_callable("main").expect("main がない");
        let exports: Vec<(String, hir::CallableId)> = exports
            .iter()
            .map(|spelling| {
                let (public, target) = spelling.split_once('=').unwrap_or((spelling, spelling));
                (
                    public.to_string(),
                    program.free_callable(target).expect("その関数がない"),
                )
            })
            .collect();
        let planned = plan_production_hir_for_test(&program, &analysis, entry, &exports)
            .expect("計画できるはず");
        (program, planned)
    }

    /// 宣言されているだけの test は生産物に入らない。scalar 外の機能を使う
    /// test があっても生産ビルドは進める
    #[test]
    fn 生産の根にtestは入らない() {
        let (program, planned) = production(
            "fn only_in_test(-> int) { 1 }\n\
             fn main(-> int) { 0 }\n\
             test \"t\" { assert only_in_test() == 1 }\n",
            &[],
        );

        assert_eq!(planned.plan.roots.len(), 1, "根は main だけ");
        assert!(
            instances_named(&program, &planned.plan, "only_in_test").is_empty(),
            "test からしか届かない本体は計画に入らない"
        );
    }

    /// 別名は公開名を増やすだけ。実装は1つに寄る
    #[test]
    fn 別名の根は同じinstanceを共有する() {
        let (_, planned) = production(
            "fn find(-> int) { 1 }\n\
             fn main(-> int) { 0 }\n",
            &["alpha=find", "zebra=find"],
        );

        assert_eq!(planned.exports.len(), 2, "ラッパは公開名ごと");
        assert_eq!(planned.exports[0].1, planned.exports[1].1, "実装は1つ");
        assert_eq!(planned.plan.roots.len(), 2, "根は main と find の2つ");
    }

    /// 別の根から同じ本体へ届いたときも instance は1つ
    #[test]
    fn 根を跨いで同じinstanceに寄る() {
        let (program, planned) = production(
            "fn shared(-> int) { 1 }\n\
             fn left(-> int) { shared() }\n\
             fn main(-> int) { shared() }\n",
            &["left"],
        );

        assert_eq!(
            instances_named(&program, &planned.plan, "shared").len(),
            1,
            "共有された本体の実装は1つ"
        );
    }

    /// 公開関数はそれぞれ空の提供文脈から始まる。`main` の提供は引き継がない
    #[test]
    fn 公開関数の要求は自分で閉じる() {
        let program = lowered_of(
            "fn ticks(-> int) { clock.now() }\n\
             fn main(-> int) { with clock(Frozen { t = 1 }) { ticks() } }\n",
        );
        let analysis = crate::requirement::analyze_hir_for_test(&program);
        let entry = program.free_callable("main").expect("main がない");
        let ticks = program.free_callable("ticks").expect("ticks がない");

        // main からは提供されているので閉じる
        assert!(analysis.errors_for_roots(&["main".to_string()]).is_empty());
        // 同じ本体でも、公開の根として単体で立てば要求が残る
        assert_eq!(analysis.errors_for_roots(&["ticks".to_string()]).len(), 1);
        assert_eq!(
            plan_production_hir_for_test(
                &program,
                &analysis,
                entry,
                &[("ticks".to_string(), ticks)]
            )
            .err(),
            Some(PlanError::MissingProvider {
                body: hir::BodyId::Callable(ticks),
                slot: slot(&program, "clock"),
            })
        );
    }

    /// 根の並びは渡された公開名の順そのまま。instance 番号もそれで決まる
    #[test]
    fn 根の順は決定的() {
        let (program, planned) = production(
            "fn alpha(-> int) { 1 }\n\
             fn zebra(-> int) { 2 }\n\
             fn main(-> int) { 0 }\n",
            &["alpha", "zebra"],
        );

        let names: Vec<String> = planned
            .plan
            .roots
            .iter()
            .map(|(body, _)| program.show_body(*body))
            .collect();
        assert_eq!(names, ["main", "alpha", "zebra"]);
        assert_eq!(planned.entry, planned.plan.roots[0].1);
    }
}
