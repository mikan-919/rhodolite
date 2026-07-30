//! 要求推論。「この関数はどのスロットを外から要求するか」を求める。
//!
//! 完成形のアルゴリズム(docs/adr/0003 より):
//!
//!   req(f) = (直接使用 ∪ ⋃ req(呼び出し先)) − そのスコープで提供されているもの
//!
//! 依存が呼ばれる側にしか向かわないので、Tarjan で強連結成分に縮約すれば
//! 逆位相順の1パスで済む(不動点反復は不要)。
//!
//! 段は6つ:
//!
//!   1. collect_slots  … スロット表を作る
//!   2. 直接使用      ┐
//!   3. 呼び出し辺    ├ scan が本体を1回歩いて同時に集める
//!   4. 提供による打ち消し ┘
//!   5. 要求の伝播(いまは素朴な不動点反復。将来 SCC 縮約 + 逆位相1パス)
//!   6. 到達経路
//!
//! 手順2と3は最初 `direct_uses` / `calls` として別々に書いたが、手順4で
//! 「いま提供されているスロットの集合」を持ち回る必要が出た時点で、
//! 同じ走査に畳んだ。`scan` の1回の再帰が2・3・4を兼ねている。

use crate::diag::Diag;
use crate::hir;
use crate::lex::Span;
use std::collections::{BTreeMap, BTreeSet, HashMap};

// ---------------------------------------------------------------------------
// 手順1: スロット表
// ---------------------------------------------------------------------------

/// `effect db: Database` から作る、スロット名 → trait 名の表。
///
/// これがないと `db.save(u)` を見たとき、`db` がスロットなのか
/// ただのローカル変数なのか区別がつかない。
#[derive(Debug, Default)]
pub struct Slots {
    pub(crate) map: HashMap<String, String>,
}

impl Slots {
    pub fn trait_of(&self, slot: &str) -> Option<&str> {
        self.map.get(slot).map(|s| s.as_str())
    }

    pub fn is_slot(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    pub fn names(&self) -> BTreeSet<&str> {
        self.map.keys().map(|s| s.as_str()).collect()
    }
}

// ---------------------------------------------------------------------------
// 要求の語彙
// ---------------------------------------------------------------------------

/// スロットの型だけが要るか、実体まで要るか。
///
/// 宣言順が強さの順でもある。実体があれば具体型も分かるため `Value > Type`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlotLevel {
    Type,
    Value,
}

/// 漏れたスロットの使用。`span` は最初に見つけた使用地点で、提供忘れの
/// 診断はここを主 span に取る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlotUse {
    level: SlotLevel,
    span: Span,
}

/// スロット名 → その要求がどこから来たかの経路。
///
/// 経路は呼び出しの連なりで、**末尾が実際に使っている本体**。
/// 空なら「この本体自身が使っている」。
/// `handle` が持つ `{"clock": ["promote", "stamp"]}` は
/// 「clock が要る ← stamp ← promote ← handle」と表示される
/// (`←` は「その要求元」なので、実際の使用者から呼び出し元へさかのぼる向き)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub level: SlotLevel,
    /// スロットを実際に使っている地点。提供忘れの主 span
    pub span: Span,
    pub path: Vec<Hop>,
}

/// 到達経路の1ホップ。名前だけを読む用途(一覧・1行表現)は従来どおり、
/// 位置を読む用途(診断の従属エントリ)はこの span を使う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub name: String,
    /// 要求を運ぶ呼び出しそのものの位置。呼び出し**元**のファイルにある
    pub span: Span,
}

impl Requirement {
    /// 経路の本体名だけ。1行表現と既存の一覧はこれで足りる。
    pub fn path_names(&self) -> Vec<&str> {
        self.path.iter().map(|hop| hop.name.as_str()).collect()
    }
}

pub type Reqs = BTreeMap<String, Requirement>;

#[derive(Debug)]
pub struct Analysis {
    pub slots: Slots,
    /// 本体名 → その本体が外へ要求するもの
    pub reqs: BTreeMap<String, Reqs>,
    /// 出力の並び順(宣言順)。BTreeMap の辞書順だと読みにくいため
    pub order: Vec<String>,
    diagnostics: Vec<Diag>,
}

// ---------------------------------------------------------------------------
// HIR を入力にした要求解析
// ---------------------------------------------------------------------------

/// 要求解析が本体を指す同一性。
///
/// スロット経由の呼び出しは、実行時にどの実装が走るかを提供が決めるので、
/// 契約メソッドという**仮想の本体**へ向かう。その契約を実装する全ての本体の
/// 要求がそこで合流する(design.md 決定8)。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum BodyKey {
    Body(hir::BodyId),
    TraitMethod(hir::TraitMethodId),
}

/// 呼び出し1つ分の記録。「その地点で何が提供されていたか」を一緒に覚える。
#[derive(Debug, Clone)]
struct Call {
    callee: BodyKey,
    provided: BTreeMap<hir::SlotId, SlotLevel>,
    /// 呼び出しそのものの位置。提供忘れの到達経路はこのホップをここで指す
    span: Span,
}

/// 本体を1回歩いて分かること。
#[derive(Debug, Default, Clone)]
struct Facts {
    /// 提供されないまま漏れたスロットの使用
    escaping: BTreeMap<hir::SlotId, SlotUse>,
    calls: Vec<Call>,
}

/// 本体を1回歩いて `Facts` を作る。
///
/// AST を歩いていた頃はスロット名とローカル名を字句スコープで言い分ける必要が
/// あったが、HIR ではスロットの使用が `Call::Slot` と `With` の提供にしか
/// 現れない(名前は既に解決済み)。字句スコープの手当てはここには要らない。
fn scan_body(body: &hir::Body) -> Facts {
    let mut facts = Facts::default();
    let nothing = BTreeMap::new();
    for id in &body.root {
        scan(body, *id, &nothing, &mut facts);
    }
    facts
}

/// **スコープの終わりを書く必要がない**ことに注目。`scan(.., &inner, ..)` から
/// 戻った時点で `inner` は消えていて、呼び出し元は元の `provided` を持ったまま。
/// 字句スコープを呼び出しスタックがそのまま表現している。
fn scan(
    body: &hir::Body,
    id: hir::ExprId,
    provided: &BTreeMap<hir::SlotId, SlotLevel>,
    out: &mut Facts,
) {
    let expr = body.expr(id);
    // 部分式をたどる。`provided` は枝ごとに差し替えて下へ運ぶ
    macro_rules! walk {
        ($child:expr) => {
            scan(body, *$child, provided, out)
        };
    }
    match &expr.kind {
        hir::ExprKind::With {
            provisions,
            body: inner,
        } => {
            // 提供値は全て外側で評価する(`with db(make())` の make は db が
            // 立つ前に走る)
            for provision in provisions {
                if let Some(value) = &provision.value {
                    walk!(value);
                }
            }
            let mut inner_provided = provided.clone();
            for provision in provisions {
                let level = match provision.value {
                    Some(_) => SlotLevel::Value,
                    None => SlotLevel::Type,
                };
                inner_provided.insert(provision.slot, level);
            }
            scan(body, *inner, &inner_provided, out);
        }

        hir::ExprKind::Call(call) => {
            match call {
                hir::Call::Direct { callable, .. } | hir::Call::Associated { callable, .. } => {
                    out.calls.push(Call {
                        callee: BodyKey::Body(hir::BodyId::Callable(*callable)),
                        provided: provided.clone(),
                        span: expr.span,
                    });
                }
                // スロット経由は契約へ向かう。実行時に選ばれる実装は提供が決める
                hir::Call::Slot {
                    slot,
                    method,
                    receiver,
                    slot_span,
                    ..
                } => {
                    let level = match receiver {
                        hir::SlotReceiver::Value => SlotLevel::Value,
                        hir::SlotReceiver::Type => SlotLevel::Type,
                    };
                    // 使用地点はスロットを名指している部分。呼び出し全体ではない
                    record_access(*slot, level, *slot_span, provided, out);
                    out.calls.push(Call {
                        callee: BodyKey::TraitMethod(*method),
                        provided: provided.clone(),
                        span: expr.span,
                    });
                }
                // ponytail: 値レシーバのメソッド呼び出しは辺にしない。要求が
                // そこを通り抜けるが、AST を歩いていた頃と同じ保守的な
                // 過小近似。辺にするなら `Facts` の合流だけを直せばよい
                hir::Call::Method { .. } => {}
                hir::Call::Ctor { .. } => {}
            }
            if let hir::Call::Method { recv, .. } = call {
                walk!(recv);
            }
            for arg in call_args(call) {
                walk!(arg);
            }
        }

        hir::ExprKind::Match { subject, arms } => {
            walk!(subject);
            // どの arm も実行されうるので全 arm の guard と本体を合流する
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
        hir::ExprKind::Let { value, .. } | hir::ExprKind::AssignLocal { value, .. } => walk!(value),
        hir::ExprKind::AssignField { recv, value, .. } => {
            walk!(recv);
            walk!(value);
        }
        hir::ExprKind::Neg(inner)
        | hir::ExprKind::Assert(inner)
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
}

fn call_args(call: &hir::Call) -> &[hir::ExprId] {
    match call {
        hir::Call::Direct { args, .. }
        | hir::Call::Associated { args, .. }
        | hir::Call::Method { args, .. }
        | hir::Call::Slot { args, .. }
        | hir::Call::Ctor { args, .. } => args,
    }
}

fn record_access(
    slot: hir::SlotId,
    level: SlotLevel,
    span: Span,
    provided: &BTreeMap<hir::SlotId, SlotLevel>,
    out: &mut Facts,
) {
    if provided.get(&slot).is_some_and(|given| *given >= level) {
        return;
    }
    match out.escaping.get_mut(&slot) {
        Some(existing) if existing.level < level => *existing = SlotUse { level, span },
        Some(_) => {}
        None => {
            out.escaping.insert(slot, SlotUse { level, span });
        }
    }
}

fn merge_facts(into: &mut Facts, from: Facts) {
    for (slot, used) in from.escaping {
        match into.escaping.get_mut(&slot) {
            Some(existing) if existing.level < used.level => *existing = used,
            Some(_) => {}
            None => {
                into.escaping.insert(slot, used);
            }
        }
    }
    into.calls.extend(from.calls);
}

/// スロット名 → その要求がどこから来たか。位置と経路の意味は従来どおり。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Req {
    level: SlotLevel,
    span: Span,
    path: Vec<Hop>,
}

/// 型検査を通った HIR から要求を推論する。
pub fn analyze(program: &hir::Program) -> Analysis {
    let mut diagnostics = duplicate_slot_diagnostics(program);
    diagnostics.sort();
    diagnostics.dedup();

    // 各本体を1回だけ歩く。ここから先は式を見ない
    let mut facts: BTreeMap<BodyKey, Facts> = BTreeMap::new();
    for id in &program.bodies {
        let body_facts = scan_body(program.body(*id));
        merge_facts(facts.entry(BodyKey::Body(*id)).or_default(), body_facts);
    }
    // 契約メソッドは、それを実装する全ての本体の要求が合流した仮想の本体。
    // 文字列の鍵を作らずに `impl Trait::method` と `impl Type::method` の
    // 二重の同一性を保つ(design.md 決定8)
    for (impl_, decl) in program.trait_impls.iter() {
        let _ = impl_;
        for (method, callable) in &decl.methods {
            let concrete = facts
                .get(&BodyKey::Body(hir::BodyId::Callable(*callable)))
                .cloned()
                .unwrap_or_default();
            merge_facts(
                facts.entry(BodyKey::TraitMethod(*method)).or_default(),
                concrete,
            );
        }
    }
    // 実装が1つも無い契約も本体として存在する(要求は空)
    for method in program.trait_methods.ids() {
        facts.entry(BodyKey::TraitMethod(method)).or_default();
    }

    // 空から始めて、変化がなくなるまで回す。
    // 要求は増える一方(単調)なので必ず止まる。
    //
    // ponytail: 素朴な不動点反復。呼び出しグラフを Tarjan で SCC 縮約して
    // 逆位相順に舐めれば反復を減らせる(docs/adr/0003)。プログラムが
    // 大きくなって遅くなったら、そのときに入れ替える。
    let mut reqs: BTreeMap<BodyKey, BTreeMap<hir::SlotId, Req>> =
        facts.keys().map(|key| (*key, BTreeMap::new())).collect();

    loop {
        let mut changed = false;
        let mut updated: BTreeMap<BodyKey, BTreeMap<hir::SlotId, Req>> = BTreeMap::new();

        for (key, body) in &facts {
            let mut next = reqs[key].clone();

            // (1) この本体自身が使っていて、提供されていないもの。経路は空
            for (slot, used) in &body.escaping {
                changed |= merge_requirement(&mut next, *slot, used.level, used.span, Vec::new());
            }

            // (2) 呼び出し先から上がってくるもの
            for site in &body.calls {
                let Some(callee) = reqs.get(&site.callee) else {
                    continue;
                };
                for (slot, requirement) in callee {
                    // この呼び出し地点で提供されているなら、ここで止まる
                    if site
                        .provided
                        .get(slot)
                        .is_some_and(|given| *given >= requirement.level)
                    {
                        continue;
                    }
                    // このホップは「呼び出し元のどの呼び出しが要求を運んだか」
                    // なので、名前は呼び先、位置は呼び出し地点になる
                    let mut path = vec![Hop {
                        name: show_key(program, site.callee),
                        span: site.span,
                    }];
                    path.extend(requirement.path.iter().cloned());
                    changed |= merge_requirement(
                        &mut next,
                        *slot,
                        requirement.level,
                        requirement.span,
                        path,
                    );
                }
            }

            updated.insert(*key, next);
        }

        reqs = updated;
        if !changed {
            break;
        }
    }

    // 表示の境界で名前へ戻す。並びは宣言順の本体と、スロット名の順
    let mut order = Vec::new();
    let mut public: BTreeMap<String, Reqs> = BTreeMap::new();
    for id in &program.bodies {
        // `impl` のメソッドは一覧に出さない(従来どおり fn と test だけ)
        if let hir::BodyId::Callable(callable) = id
            && program.callables[*callable].owner != hir::CallableOwner::Free
        {
            continue;
        }
        let name = program.show_body(*id);
        let found = reqs
            .get(&BodyKey::Body(*id))
            .expect("全ての本体に要求の表がある");
        public.insert(
            name.clone(),
            found
                .iter()
                .map(|(slot, req)| {
                    (
                        program.slots[*slot].name.clone(),
                        Requirement {
                            level: req.level,
                            span: req.span,
                            path: req.path.clone(),
                        },
                    )
                })
                .collect(),
        );
        order.push(name);
    }

    Analysis {
        slots: slots_of(program),
        reqs: public,
        order,
        diagnostics,
    }
}

/// 本体の表示名。到達経路のホップに載る綴り。
fn show_key(program: &hir::Program, key: BodyKey) -> String {
    match key {
        BodyKey::Body(id) => program.show_body(id),
        BodyKey::TraitMethod(id) => program.show_trait_method(id),
    }
}

/// スロット名 → trait 名。CLI の一覧表示のために持つ。
fn slots_of(program: &hir::Program) -> Slots {
    let mut map = HashMap::new();
    for (_, slot) in program.slots.iter() {
        map.insert(slot.name.clone(), program.traits[slot.trait_].name.clone());
    }
    Slots { map }
}

fn duplicate_slot_diagnostics(program: &hir::Program) -> Vec<Diag> {
    let mut declared: HashMap<&str, &str> = HashMap::new();
    let mut diagnostics = Vec::new();
    for (_, slot) in program.slots.iter() {
        let trait_name = program.traits[slot.trait_].name.as_str();
        if let Some(previous) = declared.insert(&slot.name, trait_name) {
            diagnostics.push(Diag::at(
                slot.span,
                format!(
                    "effect `{}` が重複しています (`{previous}` と `{trait_name}`)",
                    slot.name
                ),
            ));
        }
    }
    diagnostics
}

fn merge_requirement(
    reqs: &mut BTreeMap<hir::SlotId, Req>,
    slot: hir::SlotId,
    level: SlotLevel,
    span: Span,
    path: Vec<Hop>,
) -> bool {
    match reqs.get_mut(&slot) {
        Some(existing) if existing.level < level => {
            existing.level = level;
            existing.span = span;
            existing.path = path;
            true
        }
        Some(_) => false,
        None => {
            reqs.insert(slot, Req { level, span, path });
            true
        }
    }
}

impl Analysis {
    /// 要求解析より前に見つかる宣言・名前解決エラーと、提供忘れをまとめて返す。
    #[cfg(test)]
    pub fn errors(&self) -> Vec<Diag> {
        let mut errors = self.diagnostics.clone();
        errors.extend(self.unsatisfied());
        errors
    }

    pub fn errors_for(&self, entry: &str) -> Vec<Diag> {
        let mut errors = self.diagnostics.clone();
        errors.extend(self.unsatisfied_for(entry));
        errors
    }

    /// 推論結果の一覧。IDE がゴーストテキストで見せるものの、テキスト版。
    pub fn render(&self) -> String {
        let mut out = String::new();
        for name in &self.order {
            let reqs = &self.reqs[name];
            if reqs.is_empty() {
                out.push_str(&format!("  {name} / (要求なし)\n"));
            } else {
                let names: Vec<String> = reqs
                    .iter()
                    .map(|(slot, requirement)| match requirement.level {
                        SlotLevel::Type => format!("{slot} (型)"),
                        SlotLevel::Value => slot.clone(),
                    })
                    .collect();
                out.push_str(&format!("  {name} / {}\n", names.join(", ")));
            }
        }
        out
    }

    /// エントリ(`main` とテスト)に要求が残っていたら、それは提供忘れ。
    ///
    /// 到達経路を添えて返す。原因は数階層下にあるので、経路がないと直せない。
    #[cfg(test)]
    pub fn unsatisfied(&self) -> Vec<Diag> {
        self.unsatisfied_for("main")
    }

    pub fn unsatisfied_for(&self, entry: &str) -> Vec<Diag> {
        let mut errors = Vec::new();

        for name in &self.order {
            let is_entry = name == entry || name.starts_with("test \"");
            if !is_entry {
                continue;
            }

            for (slot, requirement) in &self.reqs[name] {
                let msg = match requirement.level {
                    SlotLevel::Type => {
                        format!("{name}: `{slot}` の実装型が提供されていません")
                    }
                    SlotLevel::Value => {
                        format!("{name}: `{slot}` が提供されていません")
                    }
                };
                // 使っている関数を `slot` の隣に置いて、呼び出し元へさかのぼる。
                // 経路は entry から降る順に持っているので、逆に読んで entry で閉じる。
                // 位置の要らない読み方(README・テストの部分一致)はこの1行で足りる
                let mut chain = format!("{slot} が要る");
                for name in requirement.path_names().iter().rev() {
                    chain.push_str(&format!(" ← {name}"));
                }
                chain.push_str(&format!(" ← {name}"));

                // 経路はモジュールを跨ぐので、ホップごとに自分のファイルを持たせる
                let hops = requirement
                    .path
                    .iter()
                    .rev()
                    .map(|hop| {
                        Diag::at(hop.span, format!("`{}` を呼んでいます", hop.name))
                            .label("ここが要求を運ぶ")
                    })
                    .collect();
                errors.push(
                    Diag::at(requirement.span, msg)
                        .label(format!("`{slot}` がここで要る"))
                        .help(chain)
                        .related(hops),
                );
            }
        }
        errors
    }
}

// ---------------------------------------------------------------------------
// テスト。上から順に緑にしていく
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast;
    use crate::lex::{join, lex};
    use crate::parse;

    fn program(src: &str) -> ast::Program {
        parse::parse(&join(lex(src).unwrap())).expect("パースできるはず")
    }

    /// 型検査を通してから解析する。要求解析の入力は下ろした HIR
    fn analysis_of(src: &str) -> Analysis {
        analysis_of_program(&program(src))
    }

    fn analysis_of_program(p: &ast::Program) -> Analysis {
        let lowered = crate::typecheck::check_and_lower(p).expect("型検査を通るはず");
        analyze(&lowered)
    }

    /// 文言だけを見る検査のための取り出し。
    fn messages(diagnostics: &[Diag]) -> Vec<String> {
        diagnostics.iter().map(|d| d.msg.clone()).collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// 要求解析のテストが使う宣言。要求解析の入力は型検査を通った HIR なので、
    /// スロットの trait とその実装、データ型はここでまとめて宣言しておく
    const PRELUDE: &str = "trait Database {\n\
                           \x20 fn find(self, id: int -> User?)\n\
                           \x20 fn save(self, u: User -> unit)\n\
                           }\n\
                           trait Clock {\n\
                           \x20 fn now(self -> int)\n\
                           \x20 fn zero(-> int)\n\
                           }\n\
                           trait Mailer { fn send(self, u: User -> unit) }\n\
                           enum Rank { Bronze Gold }\n\
                           struct User { id: int\nrank: Rank }\n\
                           enum Lookup { Found(Postgres) Missing }\n\
                           struct Postgres { url: str }\n\
                           impl Postgres { fn new(url: str -> Postgres) { Postgres { url = url } } }\n\
                           impl Database for Postgres {\n\
                           \x20 fn find(self, id: int -> User?) { nil }\n\
                           \x20 fn save(self, u: User -> unit) { let ignored = u }\n\
                           }\n\
                           impl Mailer for Postgres { fn send(self, u: User -> unit) { let ignored = u } }\n\
                           struct SystemClock {}\n\
                           impl Clock for SystemClock {\n\
                           \x20 fn now(self -> int) { 0 }\n\
                           \x20 fn zero(-> int) { 0 }\n\
                           }\n\
                           effect db: Database\n\
                           effect clock: Clock\n\
                           effect mail: Mailer\n";

    /// 前置きを付けて型検査を通し、下ろした HIR を返す
    fn lowered_of(src: &str) -> hir::Program {
        let p = program(&format!("{PRELUDE}{src}"));
        crate::typecheck::check_and_lower(&p).expect("型検査を通るはず")
    }

    fn scan_of(program: &hir::Program, f: &str) -> Facts {
        let id = program.free_callable(f).expect("その名前の関数がない");
        scan_body(&program.callables[id].body)
    }

    fn slot_id(program: &hir::Program, name: &str) -> hir::SlotId {
        program
            .slots
            .iter()
            .find(|(_, slot)| slot.name == name)
            .map(|(id, _)| id)
            .expect("そのスロットがない")
    }

    /// 本体1つ分の事実を名前で読む。0 が漏れたスロット(手順2)、
    /// 1 が呼び出し辺の行き先(手順3)
    fn facts_of(src: &str, f: &str) -> (BTreeSet<String>, BTreeSet<String>) {
        let lowered = lowered_of(src);
        let facts = scan_of(&lowered, f);
        (
            facts
                .escaping
                .keys()
                .map(|slot| lowered.slots[*slot].name.clone())
                .collect(),
            facts
                .calls
                .iter()
                .map(|call| show_key(&lowered, call.callee))
                .collect(),
        )
    }

    // ---- 移行前後で同じ結果になることを固定する corpus ----

    /// 要求解析の入力が AST から HIR へ変わっても結果が変わらないことを
    /// 固定する corpus。すべて型検査を通るので、どちらの実装にも掛けられる
    /// (tasks 5.5)。宣言は共通の前置きにまとめてある
    const CORPUS: [(&str, &str); 5] = [
        ("正典", ""),
        (
            "再帰",
            "trait Clock { fn now(self -> int) }\n\
             struct Frozen { t: int }\n\
             impl Clock for Frozen { fn now(self -> int) { self.t } }\n\
             effect clock: Clock\n\
             fn ping(n: int -> int) { if n == 0: clock.now() else: pong(n) }\n\
             fn pong(n: int -> int) { ping(n) }\n\
             fn main(-> int) { ping(1) }\n",
        ),
        (
            "match",
            "trait Clock { fn now(self -> int) }\n\
             struct Frozen { t: int }\n\
             impl Clock for Frozen { fn now(self -> int) { self.t } }\n\
             effect clock: Clock\n\
             enum Lookup { Found(int) Missing }\n\
             fn pick(l: Lookup -> int) {\n\
             \x20 match l {\n\
             \x20   Lookup::Found(n) if n == 1: clock.now()\n\
             \x20   _: 0\n\
             \x20 }\n\
             }\n\
             fn main(-> int) { pick(Missing) }\n",
        ),
        (
            "入れ子の提供",
            "trait Clock { fn now(self -> int)\n fn zero(-> int) }\n\
             struct Frozen { t: int }\n\
             struct Zero {}\n\
             impl Clock for Frozen { fn now(self -> int) { self.t }\n fn zero(-> int) { 0 } }\n\
             impl Clock for Zero { fn now(self -> int) { 0 }\n fn zero(-> int) { 0 } }\n\
             impl Frozen { fn at(t: int -> Frozen) { Frozen { t = t } } }\n\
             effect clock: Clock\n\
             fn deep(-> int) { clock.now() }\n\
             fn shallow(-> int) { clock::zero() }\n\
             fn main(-> int) {\n\
             \x20 with clock<Zero> {\n\
             \x20   let typed = shallow()\n\
             \x20   with clock(Frozen::at(1)) { deep() + typed }\n\
             \x20 }\n\
             }\n",
        ),
        (
            "提供忘れ",
            "trait Clock { fn now(self -> int) }\n\
             struct Frozen { t: int }\n\
             impl Clock for Frozen { fn now(self -> int) { self.t } }\n\
             effect clock: Clock\n\
             fn stamp(-> int) { clock.now() }\n\
             fn promote(-> int) { stamp() }\n\
             fn main(-> int) { promote() }\n",
        ),
    ];

    /// corpus 1件分の完全な結果。一覧・提供忘れの文言・位置・経路まで全部
    fn corpus_report(analysis: &Analysis, entry: &str) -> String {
        let mut out = analysis.render();
        for error in analysis.errors_for(entry) {
            out.push_str(&format!("! {}\n", error.msg));
            if let Some(help) = &error.help {
                out.push_str(&format!("  help {help}\n"));
            }
            if let Some(span) = error.span {
                out.push_str(&format!("  at {} {}..{}\n", span.src, span.start, span.end));
            }
            for related in &error.related {
                out.push_str(&format!("  related {}\n", related.msg));
            }
        }
        out
    }

    fn corpus_source(name: &str, src: &str) -> String {
        if name == "正典" {
            std::fs::read_to_string("examples/canonical.rd").unwrap()
        } else {
            src.to_string()
        }
    }

    /// corpus の期待値。AST を歩いていた実装が出していた結果をそのまま固定して
    /// あるので、入力が HIR へ変わって結果が動いたらここが落ちる
    const EXPECTED: [&str; 5] = [
        // 正典
        "  stamp / clock, db\n\
         \x20 promote / clock, db\n\
         \x20 handle / clock, db\n\
         \x20 main / (要求なし)\n\
         \x20 test \"昇格すると Gold になり時刻が刻まれる\" / (要求なし)\n",
        // 再帰
        "  ping / clock\n\
         \x20 pong / clock\n\
         \x20 main / clock\n\
         ! main: `clock` が提供されていません\n\
         \x20 help clock が要る ← ping ← main\n\
         \x20 at 0 174..183\n\
         \x20 related `ping` を呼んでいます\n",
        // match
        "  pick / clock\n\
         \x20 main / clock\n\
         ! main: `clock` が提供されていません\n\
         \x20 help clock が要る ← pick ← main\n\
         \x20 at 0 245..254\n\
         \x20 related `pick` を呼んでいます\n",
        // 入れ子の提供
        "  deep / clock\n\
         \x20 shallow / clock (型)\n\
         \x20 main / (要求なし)\n",
        // 提供忘れ
        "  stamp / clock\n\
         \x20 promote / clock\n\
         \x20 main / clock\n\
         ! main: `clock` が提供されていません\n\
         \x20 help clock が要る ← stamp ← promote ← main\n\
         \x20 at 0 157..166\n\
         \x20 related `stamp` を呼んでいます\n\
         \x20 related `promote` を呼んでいます\n",
    ];

    #[test]
    fn corpusの解析結果は移行前と同じ() {
        for ((name, src), expected) in CORPUS.iter().zip(EXPECTED) {
            let source = corpus_source(name, src);
            let analysis = analysis_of(&source);
            assert_eq!(
                corpus_report(&analysis, "main"),
                expected,
                "{name} の解析結果が変わっている"
            );
        }
    }

    /// 別モジュールの本体から上がる要求も、経路のホップが呼び出し元の
    /// ファイルを指したまま届く(tasks 5.5)
    #[test]
    fn 複数モジュールでも経路はホップごとのファイルを指す() {
        let loaded = crate::module::load_files(&[
            ("main.rd", "use dep::{stamp}\nfn main(-> int) { stamp() }\n"),
            (
                "dep.rd",
                "trait Clock { fn now(self -> int) }\n\
                 struct Frozen { t: int }\n\
                 impl Clock for Frozen { fn now(self -> int) { self.t } }\n\
                 effect clock: Clock\n\
                 fn stamp(-> int) { clock.now() }\n",
            ),
        ])
        .expect("ロードできる");
        let analysis = analysis_of_program(&loaded.program);

        assert_eq!(
            analysis.render(),
            "  dep::stamp / dep::clock\n  main::main / dep::clock\n"
        );
        let errors = analysis.errors_for("main::main");
        assert_eq!(errors.len(), 1, "{errors:?}");
        let error = &errors[0];
        assert_eq!(error.msg, "main::main: `dep::clock` が提供されていません");
        // 使用地点は dep、要求を運ぶ呼び出しは main
        assert_eq!(error.span.expect("位置を持つ").src, 1);
        assert_eq!(error.related.len(), 1);
        assert_eq!(
            error.related[0].span.expect("位置を持つ").src,
            0,
            "{:?}",
            error.related
        );
    }

    // ---- 手順1 ----

    /// スロット表は下ろした HIR から作る。名前で引くのは CLI の一覧表示だけ
    #[test]
    fn 手順1_スロット表を作る() {
        let lowered = lowered_of("fn main() { assert true }\n");
        let slots = analyze(&lowered).slots;

        assert_eq!(slots.trait_of("db"), Some("Database"));
        assert_eq!(slots.trait_of("clock"), Some("Clock"));
        assert_eq!(slots.trait_of("nope"), None);
        assert!(slots.is_slot("db"));
        assert!(!slots.is_slot("u"));
    }

    #[test]
    fn 手順1_正典のスロットは二つ() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let analysis = analysis_of(&src);
        assert_eq!(
            analysis.slots.names(),
            ["clock", "db"].into_iter().collect()
        );
        assert_eq!(analysis.slots.trait_of("db"), Some("Database"));
    }

    #[test]
    fn 重複したスロットを報告する() {
        let lowered = lowered_of("effect db: Database\nfn main() { assert true }\n");
        let errors = analyze(&lowered).errors();

        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(
            errors[0].msg,
            "effect `db` が重複しています (`Database` と `Database`)"
        );
        assert!(errors[0].span.is_some(), "重複した effect 宣言を指す");
    }

    /// 定義の無い直接呼び出しは、下ろす前に呼び出し先が決まらないので
    /// 型検査が止める。HIR の呼び出しは必ず宣言を指す(design.md 決定6)
    #[test]
    fn 未定義の直接関数呼び出しは下ろす前に止まる() {
        let p = program(&format!("{PRELUDE}fn main() {{ stanp() }}\n"));
        let errors = crate::typecheck::check_and_lower(&p).expect_err("解決できない");
        assert_eq!(
            messages(&errors),
            vec!["main: `stanp` の呼び出し先が決まりません".to_string()]
        );
    }

    #[test]
    fn 後で定義された関数は呼び出せる() {
        let lowered = lowered_of("fn main() { stamp() }\nfn stamp() { let n = 1 }\n");
        assert!(analyze(&lowered).errors().is_empty());
    }

    // ---- 手順2 ----

    #[test]
    fn 手順2_直接使っているスロットを集める() {
        let facts = facts_of(
            "fn stamp(u: User) {\n\
             \x20 let at = clock.now()\n\
             \x20 db.save(u)\n\
             }\n",
            "stamp",
        );
        assert_eq!(facts.0, set(&["clock", "db"]));
    }

    #[test]
    fn 手順2_ローカル変数のフィールドは要求ではない() {
        let facts = facts_of("fn f(u: User) { let r = u.rank }\n", "f");
        assert!(facts.0.is_empty());
    }

    /// HIR ではスロットの使用が呼び出しにしか現れない。フィールドの読みは
    /// 型検査が値の読みとして解決するので、スロットの使用にはならない
    #[test]
    fn 手順2_スロットのフィールドは値にならないので下ろす前に止まる() {
        let p = program(&format!("{PRELUDE}fn f(-> int) {{ db.url }}\n"));
        let errors = crate::typecheck::check_and_lower(&p).expect_err("値にならない");
        assert!(
            errors[0].msg.contains("`db` は値として読めません"),
            "{errors:?}"
        );
    }

    // ---- 手順3 ----

    #[test]
    fn 手順3_呼んでいる関数の名前を集める() {
        let facts = facts_of(
            "fn stamp(u: User) { let n = 1 }\n\
             fn audit(u: User) { let n = 2 }\n\
             fn promote(u: User -> bool) {\n\
             \x20 stamp(u)\n\
             \x20 audit(u)\n\
             \x20 true\n\
             }\n",
            "promote",
        );
        assert_eq!(facts.1, set(&["audit", "stamp"]));
    }

    #[test]
    fn 手順3_スロット呼び出しと関連関数呼び出しは辺になる() {
        let facts = facts_of(
            "fn stamp(p: Postgres) { let n = 1 }\n\
             fn f(u: User) {\n\
             \x20 db.save(u)\n\
             \x20 let x = Postgres::new(\"u\")\n\
             \x20 stamp(x)\n\
             }\n",
            "f",
        );
        assert_eq!(
            facts.1,
            set(&["impl Database::save", "impl Postgres::new", "stamp"])
        );
    }

    #[test]
    fn 手順3_提供は呼び出しではない() {
        // `with db(pg) { ... }` は提供であって呼び出しではない。辺にしてはいけない
        let facts = facts_of(
            "fn handle(u: User) { let n = 1 }\n\
             fn main(u: User) {\n\
             \x20 with db(Postgres::new(\"u\")) {\n\
             \x20   handle(u)\n\
             \x20 }\n\
             }\n",
            "main",
        );
        assert_eq!(facts.1, set(&["handle", "impl Postgres::new"]));
    }

    // ---- 手順4 ----

    #[test]
    fn 手順4_提供されていれば要求は消える() {
        let facts = facts_of(
            "fn f(u: User, pg: Postgres) {\n\
             \x20 with db(pg) {\n\
             \x20   db.save(u)\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert!(facts.0.is_empty());
    }

    #[test]
    fn 手順4_提供されていないものは残る() {
        let facts = facts_of(
            "fn f(u: User, pg: Postgres) {\n\
             \x20 with db(pg) {\n\
             \x20   db.save(u)\n\
             \x20   let at = clock.now()\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["clock"]));
    }

    #[test]
    fn 手順4_提供はブロックの中だけ() {
        let facts = facts_of(
            "fn f(u: User, pg: Postgres) {\n\
             \x20 with db(pg) {\n\
             \x20   db.save(u)\n\
             \x20 }\n\
             \x20 let found = db.find(u.id)\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["db"]));
    }

    #[test]
    fn 手順4_提供する値は提供の外で評価される() {
        // `db(make(clock.now()))` の clock.now() は、db が使えるようになる前に走る。
        // よって clock はこの提供では覆われない
        let facts = facts_of(
            "fn make(at: int -> Postgres) { Postgres { url = \"u\" } }\n\
             fn f(u: User) {\n\
             \x20 with db(make(clock.now())) {\n\
             \x20   db.save(u)\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["clock"]));
    }

    #[test]
    fn 手順4_呼び出し地点の提供が記録される() {
        let lowered = lowered_of(
            "fn handle(u: User) { let n = 1 }\n\
             fn f(u: User, pg: Postgres) {\n\
             \x20 handle(u)\n\
             \x20 with db(pg) {\n\
             \x20   handle(u)\n\
             \x20 }\n\
             }\n",
        );
        let facts = scan_of(&lowered, "f");
        let db = slot_id(&lowered, "db");

        assert_eq!(facts.calls.len(), 2);
        assert!(facts.calls[0].provided.is_empty());
        assert_eq!(facts.calls[1].provided.get(&db), Some(&SlotLevel::Value));
    }

    /// どの arm も実行されうるので、全 arm の直接使用と呼び出し辺が合流する。
    /// 対象自身の使用も落とさない(design.md 決定4)
    #[test]
    fn 全てのarmの要求と呼び出し辺が合流する() {
        let src = "fn pick(r: Rank -> Rank) { let found = db.find(1)\n r }\n\
                   fn stamp(r: Rank -> int) { 1 }\n\
                   fn f(r: Rank -> int) {\n\
                   \x20 match pick(r) {\n\
                   \x20   Rank::Bronze: clock.now()\n\
                   \x20   Rank::Gold {\n\
                   \x20     stamp(r)\n\
                   \x20   }\n\
                   \x20 }\n\
                   }\n";
        let facts = facts_of(src, "f");
        assert_eq!(facts.0, set(&["clock"]));
        assert_eq!(facts.1, set(&["impl Clock::now", "pick", "stamp"]));

        // 経路も既存の `if` と同じ形で伝わる
        let analysis = analysis_of(&format!("{PRELUDE}{src}"));
        let reqs = &analysis.reqs["f"];
        assert_eq!(reqs["db"].path_names(), vec!["pick"]);
        assert!(reqs["clock"].path.is_empty());
    }

    #[test]
    fn armの中の提供はその本体だけを覆う() {
        let facts = facts_of(
            "fn f(r: Rank, u: User, pg: Postgres -> int) {\n\
             \x20 match r {\n\
             \x20   Rank::Bronze {\n\
             \x20     with db(pg) { db.save(u) }\n\
             \x20     0\n\
             \x20   }\n\
             \x20   Rank::Gold { let found = db.find(u.id)\n 0 }\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["db"]));
    }

    /// payload の名前は同名スロットをその arm の間だけ隠す。型検査が名前を
    /// 解決するので、隠した arm の呼び出しは具体型のメソッドになる
    /// (design.md 決定5)
    #[test]
    fn payload束縛はそのarmの中だけスロットを隠す() {
        let facts = facts_of(
            "fn f(l: Lookup, u: User -> int) {\n\
             \x20 match l {\n\
             \x20   Lookup::Found(db) { db.save(u)\n 0 }\n\
             \x20   Lookup::Missing { let at = clock.now()\n 0 }\n\
             \x20 }\n\
             }\n",
            "f",
        );
        // 隠した arm は要求を作らないが、隠していない arm の分は残る
        assert_eq!(facts.0, set(&["clock"]));
        assert_eq!(facts.1, set(&["impl Clock::now"]));
    }

    #[test]
    fn payload束縛が全てのarmでスロットを隠せば要求は消える() {
        let facts = facts_of(
            "fn f(l: Lookup, u: User -> int) {\n\
             \x20 match l {\n\
             \x20   Lookup::Found(db) { db.save(u)\n 0 }\n\
             \x20   _: 0\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert!(facts.0.is_empty());
    }

    /// `_` は名前を作らないので、何も隠さない
    #[test]
    fn discardはスロットを隠さない() {
        let facts = facts_of(
            "fn f(l: Lookup, u: User -> int) {\n\
             \x20 match l {\n\
             \x20   Lookup::Found(_) { db.save(u)\n 0 }\n\
             \x20   _: 0\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["db"]));
    }

    /// 実行時に選ばれるのは1つでも、走査は保守的に全 arm を見る。`_` の本体の
    /// 要求と呼び出し辺も落とさない
    #[test]
    fn catch_allの本体の要求も合流する() {
        let src = "fn stamp(u: User) { mail.send(u) }\n\
                   fn f(r: Rank, u: User -> int) {\n\
                   \x20 match r {\n\
                   \x20   Rank::Gold { let found = db.find(u.id)\n 0 }\n\
                   \x20   _ {\n\
                   \x20     let at = clock.now()\n\
                   \x20     stamp(u)\n\
                   \x20     0\n\
                   \x20   }\n\
                   \x20 }\n\
                   }\n";
        let facts = facts_of(src, "f");
        assert_eq!(facts.0, set(&["db", "clock"]));
        assert_eq!(
            facts.1,
            set(&["impl Database::find", "impl Clock::now", "stamp"])
        );
        // 実行時に `_` が選ばれなくても、その本体の間接要求は残る
        let analysis = analysis_of(&format!("{PRELUDE}{src}"));
        assert_eq!(analysis.reqs["f"]["mail"].path_names(), vec!["stamp"]);
    }

    /// 実行時に variant が一致しなくても、guard の要求と呼び出し辺は残る
    #[test]
    fn guardの要求も合流する() {
        let facts = facts_of(
            "fn f(r: Rank, u: User -> int) {\n\
             \x20 match r {\n\
             \x20   Rank::Gold if clock.now() == 0 { let found = db.find(u.id)\n 0 }\n\
             \x20   _: 0\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["clock", "db"]));
        assert_eq!(facts.1, set(&["impl Clock::now", "impl Database::find"]));
    }

    /// payload の名前は guard の中でも同名スロットを隠す
    #[test]
    fn guardのpayload束縛はスロットを隠す() {
        let facts = facts_of(
            "fn f(l: Lookup, u: User -> int) {\n\
             \x20 match l {\n\
             \x20   Lookup::Found(db) if db.find(u.id) == nil: 1\n\
             \x20   Lookup::Missing if clock.now() == 0: 2\n\
             \x20   _: 0\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["clock"]));
    }

    /// guard の中の束縛は本体へも隣の arm へも漏らさない。漏れるとスロットを
    /// 隠してしまい、要求が消える
    #[test]
    fn guardの束縛は本体へ漏れない() {
        let facts = facts_of(
            "fn f(r: Rank, u: User, pg: Postgres -> int) {\n\
             \x20 match r {\n\
             \x20   Rank::Gold if { let db = pg\ntrue } { let found = db.find(u.id)\n 0 }\n\
             \x20   _: 0\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["db"]));
    }

    /// arm は束縛を導入しないが、本体の `let` は隣の arm へ漏らさない
    #[test]
    fn armの束縛は隣のarmへ漏れない() {
        let facts = facts_of(
            "fn f(r: Rank, u: User, pg: Postgres -> int) {\n\
             \x20 match r {\n\
             \x20   Rank::Bronze {\n\
             \x20     let db = pg\n\
             \x20     0\n\
             \x20   }\n\
             \x20   Rank::Gold { let found = db.find(u.id)\n 0 }\n\
             \x20 }\n\
             }\n",
            "f",
        );
        assert_eq!(facts.0, set(&["db"]));
    }

    // ---- 手順5 + 6 ----

    #[test]
    fn 手順5_要求は呼び出しを遡って伝わる() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let a = analysis_of(&src);

        // stamp が clock と db を使い、promote / handle は1文字も書いていないのに届く
        assert_eq!(a.reqs["stamp"].keys().count(), 2);
        assert_eq!(a.reqs["promote"].keys().count(), 2);
        assert_eq!(a.reqs["handle"].keys().count(), 2);

        // main と test は提供しているので要求ゼロ
        assert!(a.reqs["main"].is_empty());
    }

    #[test]
    fn 手順6_到達経路が付く() {
        let a = analysis_of(&format!(
            "{PRELUDE}fn stamp(u: User) {{ let at = clock.now() }}\n\
             fn promote(u: User) {{ stamp(u) }}\n\
             fn handle(u: User) {{ promote(u) }}\n"
        ));

        assert!(a.reqs["stamp"]["clock"].path_names().is_empty());
        assert_eq!(a.reqs["promote"]["clock"].path_names(), vec!["stamp"]);
        assert_eq!(
            a.reqs["handle"]["clock"].path_names(),
            vec!["promote", "stamp"]
        );
    }

    #[test]
    fn 手順6_提供忘れが経路付きで報告される() {
        let a = analysis_of(&format!(
            "{PRELUDE}fn stamp(u: User) {{ let at = clock.now() }}\n\
             fn promote(u: User) {{ stamp(u) }}\n\
             fn main(u: User) {{ promote(u) }}\n"
        ));
        let errors = a.unsatisfied();

        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].help.as_deref(),
            Some("clock が要る ← stamp ← promote ← main")
        );
    }

    /// 診断が指す範囲をソースから切り出す。テストの `program` は識別子 0 の
    /// 単一ソースなので、span はそのまま `src` の添字になる。
    fn slice(src: &str, span: Span) -> &str {
        &src[span.start as usize..span.end as usize]
    }

    #[test]
    fn 手順6_直接の提供忘れは使用地点を指しホップを持たない() {
        let src = format!("{PRELUDE}fn main(-> int) {{ clock.now() }}\n");
        let errors = analysis_of(&src).unsatisfied();

        assert_eq!(errors.len(), 1);
        assert_eq!(slice(&src, errors[0].span.unwrap()), "clock.now");
        assert!(errors[0].related.is_empty(), "{:?}", errors[0].related);
        assert_eq!(errors[0].help.as_deref(), Some("clock が要る ← main"));
    }

    /// 経路の各ホップは、その要求を運ぶ**呼び出し**を指す。使用地点は
    /// 一番下の関数の中にあり、ホップはそれを呼んだ側の位置になる。
    #[test]
    fn 手順6_経由した提供忘れはホップごとに呼び出しを指す() {
        let src = format!(
            "{PRELUDE}fn stamp(-> int) {{ clock.now() }}\n\
             fn promote(-> int) {{ stamp() }}\n\
             fn main(-> int) {{ promote() }}\n"
        );
        let errors = analysis_of(&src).unsatisfied();

        assert_eq!(errors.len(), 1);
        assert_eq!(slice(&src, errors[0].span.unwrap()), "clock.now");
        let hops: Vec<&str> = errors[0]
            .related
            .iter()
            .map(|hop| slice(&src, hop.span.unwrap()))
            .collect();
        assert_eq!(hops, vec!["stamp()", "promote()"]);
    }

    #[test]
    fn 手順6_正典には提供忘れがない() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let errors = analysis_of(&src).unsatisfied();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn 手順5_相互再帰でも止まる() {
        let a = analysis_of(&format!(
            "{PRELUDE}fn ping(n: int -> int) {{\n\
             \x20 let at = clock.now()\n\
             \x20 pong(n)\n\
             }}\n\
             fn pong(n: int -> int) {{ ping(n) }}\n"
        ));
        assert!(a.reqs["ping"].contains_key("clock"));
        assert!(a.reqs["pong"].contains_key("clock"));
    }

    #[test]
    fn 手順2_呼び出し先には潜らない() {
        // promote は db を直接使うが、stamp の中の clock は拾わない
        let facts = facts_of(
            "fn stamp(u: User) { let at = clock.now() }\n\
             fn promote(id: int -> bool) {\n\
             \x20 let u = db.find(id)\n\
             \x20 true\n\
             }\n",
            "promote",
        );
        assert_eq!(facts.0, set(&["db"]));
    }

    #[test]
    fn 同名の引数はスロットを隠す() {
        let facts = facts_of("fn f(db: Postgres, u: User) { db.save(u) }\n", "f");
        assert!(facts.0.is_empty());
    }

    #[test]
    fn withの本体では提供したスロットが同名ローカルを隠す() {
        let facts = facts_of(
            "fn f(db: Postgres, u: User) { with db(db) { db.save(u) } }\n",
            "f",
        );
        assert!(facts.0.is_empty(), "{:?}", facts.0);
    }

    #[test]
    fn 型射影と値射影は異なる強さの要求になる() {
        let a = analysis_of(&format!(
            "{PRELUDE}fn make(-> int) {{ clock::zero() }}\n\
             fn used(-> int) {{ clock.now() }}\n"
        ));
        assert_eq!(a.reqs["make"]["clock"].level, SlotLevel::Type);
        assert_eq!(a.reqs["used"]["clock"].level, SlotLevel::Value);
        assert!(a.render().contains("make / clock (型)"), "{}", a.render());
    }

    #[test]
    fn 型提供は型要求だけを満たす() {
        let a = analysis_of(&format!(
            "{PRELUDE}fn make(-> int) {{ clock::zero() }}\n\
             fn used(-> int) {{ clock.now() }}\n\
             fn typed(-> int) {{ with clock<SystemClock> {{ make() }} }}\n\
             fn valued(-> int) {{ with clock<SystemClock> {{ used() }} }}\n"
        ));
        assert!(a.reqs["typed"].is_empty());
        assert_eq!(a.reqs["valued"]["clock"].level, SlotLevel::Value);
    }

    #[test]
    fn 実体提供は型要求と値要求の両方を満たす() {
        let a = analysis_of(&format!(
            "{PRELUDE}fn make(-> int) {{ clock::zero() }}\n\
             fn used(-> int) {{ clock.now() }}\n\
             fn f(-> int) {{\n\
             \x20 with clock(SystemClock {{}}) {{\n\
             \x20   let n = make()\n\
             \x20   used()\n\
             \x20 }}\n\
             }}\n"
        ));
        assert!(a.reqs["f"].is_empty());
    }

    #[test]
    fn 実行されないhead本体のletは後続のスロットを隠さない() {
        let a = analysis_of(&format!(
            "{PRELUDE}fn f(local: Postgres, u: User) {{\n\
             \x20 if false {{ let db = local }}\n\
             \x20 db.save(u)\n\
             }}\n"
        ));
        assert_eq!(a.reqs["f"]["db"].level, SlotLevel::Value);
    }

    #[test]
    fn impl本体の要求はスロット呼び出し元へ伝わる() {
        let a = analysis_of(&format!(
            "{PRELUDE}struct Store {{ n: int }}\n\
             impl Database for Store {{\n\
             \x20 fn find(self, id: int -> User?) {{ nil }}\n\
             \x20 fn save(self, u: User -> unit) {{ let at = clock.now() }}\n\
             }}\n\
             fn main(u: User) {{\n\
             \x20 with db(Store {{ n = 1 }}) {{ db.save(u) }}\n\
             }}\n"
        ));
        assert_eq!(a.reqs["main"]["clock"].level, SlotLevel::Value);
    }

    #[test]
    fn impl本体の要求は型パス呼び出し元へ伝わる() {
        let a = analysis_of(&format!(
            "{PRELUDE}struct Store {{ n: int }}\n\
             impl Store {{\n\
             \x20 fn new(-> Store) {{\n\
             \x20   let at = clock.now()\n\
             \x20   Store {{ n = at }}\n\
             \x20 }}\n\
             }}\n\
             fn main(-> Store) {{ Store::new() }}\n"
        ));
        assert_eq!(a.reqs["main"]["clock"].level, SlotLevel::Value);
    }
}
