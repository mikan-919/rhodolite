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

use crate::ast::{Expr, ExprKind, Head, Item, MatchPattern, PatternBinding, Program, Provision};
use crate::diag::Diag;
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
    map: HashMap<String, String>,
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

/// プログラム全体から `Item::Effect` を拾って表にする。
///
/// ヒント: `program.items` を回して `Item::Effect { slot, trait_name, .. }` に
/// マッチしたものを入れるだけ。再帰も木歩きも要らない。
pub fn collect_slots(program: &Program) -> Slots {
    let mut map = HashMap::new();
    for x in program.items.iter() {
        if let Item::Effect {
            slot,
            trait_name,
            span: _,
        } = x
        {
            // 重複は analyze の診断で止めるため、ここでどちらが残るかは観測されない。
            map.insert(slot.clone(), trait_name.clone());
        }
    }
    Slots { map }
}

// ---------------------------------------------------------------------------
// 手順2〜4: 直接使用・呼び出し辺・提供による打ち消し
// ---------------------------------------------------------------------------

/// 呼び出し1つ分の記録。
///
/// 「どの関数を呼んだか」だけでは足りない。`with db(pg) { handle(id) }` のように
/// 提供の中で呼ばれていると、`handle` から来る db 要求はここで消えるため、
/// **その地点で何が提供されていたか**を一緒に覚えておく必要がある。
#[derive(Debug, Clone)]
pub struct CallSite {
    pub callee: String,
    pub provided: BTreeMap<String, SlotLevel>,
    pub kind: CallKind,
    /// 呼び出しそのものの位置。提供忘れの到達経路は、このホップをここで指す
    pub span: Span,
}

/// 名前解決で検査する必要があるのは、`stamp()` のような直接呼び出しだけ。
///
/// メソッド呼び出しは型検査が入るまで候補を確定できないため、要求伝播の辺としては
/// 記録するが、ここでは未定義エラーにしない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Direct,
    Method,
}

/// スロットの型だけが要るか、実体まで要るか。
///
/// 宣言順が強さの順でもある。実体があれば具体型も分かるため `Value > Type`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlotLevel {
    Type,
    Value,
}

/// 関数の本体を1回歩いて分かること。
///
/// 拾うのはこの2つの形だけ:
///
/// ```text
/// db.save(u)          →  Call( Field( Ident("db"), "save" ), [u] )
///                              ~~~~~~~~~~~~~~~~~~~ "db" が Slots にあれば escaping
/// stamp(u)            →  Call( Ident("stamp"), [u] )     calls の辺になる
/// with db(pg) { ... } →  Head( Ambient([Provision::Value { .. }]), .. )
///                        辺ではない。提供。ブロックの中だけ打ち消す
/// Postgres::new(url)  →  Call( Path([..]), [url] )       implへの辺
/// ```
///
/// 呼び出し先には潜らない。定義されている関数かどうかの絞り込みもしない
/// (それは `analyze` の仕事)。
#[derive(Debug, Default, Clone)]
pub struct BodyFacts {
    /// 提供されないまま漏れた直接使用
    pub escaping: BTreeMap<String, SlotUse>,
    pub calls: Vec<CallSite>,
}

/// 漏れたスロットの使用。`span` は最初に見つけた使用地点で、提供忘れの
/// 診断はここを主 span に取る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotUse {
    pub level: SlotLevel,
    pub span: Span,
}

/// 本体を1回歩いて `BodyFacts` を作る。
pub fn scan_body(body: &[Expr], slots: &Slots) -> BodyFacts {
    scan_body_with_locals(body, slots, BTreeSet::new())
}

fn scan_body_with_locals(body: &[Expr], slots: &Slots, mut locals: BTreeSet<String>) -> BodyFacts {
    let mut facts = BodyFacts::default();
    let nothing_provided = BTreeMap::new();
    scan_exprs(body, slots, &nothing_provided, &mut locals, &mut facts);
    facts
}

fn scan_exprs(
    body: &[Expr],
    slots: &Slots,
    provided: &BTreeMap<String, SlotLevel>,
    locals: &mut BTreeSet<String>,
    out: &mut BodyFacts,
) {
    for e in body {
        scan(e, slots, provided, locals, out);
    }
}

/// 汎用の木歩き(訪問関数を渡す形)ではなく自前で再帰する。理由は
/// `provided` を枝ごとに変えて下へ運ぶため — 訪問関数の形では状態を引き継げない。
///
/// **スコープの終わりを書く必要がない**ことに注目。`scan(body, &inner, ..)` から
/// 戻った時点で `inner` は消えていて、呼び出し元は元の `provided` を持ったまま。
/// 字句スコープを呼び出しスタックがそのまま表現している。
fn scan(
    e: &Expr,
    slots: &Slots,
    provided: &BTreeMap<String, SlotLevel>,
    locals: &mut BTreeSet<String>,
    out: &mut BodyFacts,
) {
    match &e.kind {
        ExprKind::Head { head, body, orelse } => {
            match head {
                Head::Ambient(binders) => {
                    // 提供値は全て外側で評価する。`with db(db)` の右の db は
                    // ここではまだローカルを指している。
                    for b in binders {
                        if let Provision::Value { value, .. } = b {
                            scan(value, slots, provided, locals, out);
                        }
                    }

                    let mut inner = provided.clone();
                    let mut inner_locals = locals.clone();
                    for b in binders {
                        let level = match b {
                            Provision::Type { .. } => SlotLevel::Type,
                            Provision::Value { .. } => SlotLevel::Value,
                        };
                        inner.insert(b.slot().to_string(), level);
                        // with の本体では、提供したスロットが同名の外側ローカルを隠す。
                        inner_locals.remove(b.slot());
                    }
                    scan(body, slots, &inner, &mut inner_locals, out);
                }
                Head::If(c) | Head::Elif(c) | Head::While(c) => {
                    scan(c, slots, provided, locals, out);
                    let mut inner_locals = locals.clone();
                    scan(body, slots, provided, &mut inner_locals, out);
                }
                Head::For { var, iter } => {
                    scan(iter, slots, provided, locals, out);
                    let mut inner_locals = locals.clone();
                    inner_locals.insert(var.clone());
                    scan(body, slots, provided, &mut inner_locals, out);
                }
                Head::Else => {
                    let mut inner_locals = locals.clone();
                    scan(body, slots, provided, &mut inner_locals, out);
                }
            }
            if let Some(o) = orelse {
                let mut inner_locals = locals.clone();
                scan(o, slots, provided, &mut inner_locals, out);
            }
        }

        ExprKind::Field(recv, _) | ExprKind::OptionalField(recv, _) => {
            if let ExprKind::Ident(name) = &recv.kind {
                record_access(name, SlotLevel::Value, e.span, slots, provided, locals, out);
            }
            scan(recv, slots, provided, locals, out);
        }

        ExprKind::Call(callee, args) => {
            match &callee.kind {
                ExprKind::Ident(name) if !slots.is_slot(name) && !locals.contains(name) => {
                    out.calls.push(CallSite {
                        callee: name.clone(),
                        provided: provided.clone(),
                        kind: CallKind::Direct,
                        span: e.span,
                    });
                }
                ExprKind::Field(recv, method) => {
                    if let ExprKind::Ident(name) = &recv.kind
                        && !locals.contains(name)
                        && let Some(trait_name) = slots.trait_of(name)
                    {
                        out.calls.push(CallSite {
                            callee: trait_method_key(trait_name, method),
                            provided: provided.clone(),
                            kind: CallKind::Method,
                            span: e.span,
                        });
                    }
                }
                ExprKind::Path(parts) => {
                    if let [owner, method] = parts.as_slice() {
                        let callee = if !locals.contains(owner) {
                            slots
                                .trait_of(owner)
                                .map(|trait_name| trait_method_key(trait_name, method))
                                .unwrap_or_else(|| type_method_key(owner, method))
                        } else {
                            type_method_key(owner, method)
                        };
                        out.calls.push(CallSite {
                            callee,
                            provided: provided.clone(),
                            kind: CallKind::Method,
                            span: e.span,
                        });
                    }
                }
                _ => {}
            }
            scan(callee, slots, provided, locals, out);
            for a in args {
                scan(a, slots, provided, locals, out);
            }
        }

        ExprKind::Int(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Nil => {}
        ExprKind::Ident(_) => {}
        ExprKind::Path(parts) => {
            if let Some(name) = parts.first() {
                record_access(name, SlotLevel::Type, e.span, slots, provided, locals, out);
            }
        }
        ExprKind::Array(items) => {
            for i in items {
                scan(i, slots, provided, locals, out);
            }
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, v) in fields {
                scan(v, slots, provided, locals, out);
            }
        }
        ExprKind::Let { name, value } => {
            scan(value, slots, provided, locals, out);
            locals.insert(name.clone());
        }
        ExprKind::Assign { target, value } => {
            scan(target, slots, provided, locals, out);
            scan(value, slots, provided, locals, out);
        }
        ExprKind::Unary(_, inner) => scan(inner, slots, provided, locals, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            scan(lhs, slots, provided, locals, out);
            scan(rhs, slots, provided, locals, out);
        }
        ExprKind::Return(Some(v)) => scan(v, slots, provided, locals, out),
        ExprKind::Return(None) => {}
        ExprKind::Assert(inner) => scan(inner, slots, provided, locals, out),
        ExprKind::Block(body) => scan_exprs(body, slots, provided, locals, out),

        // どの arm も実行されうるので、全 arm の要求と呼び出し辺を合流する。
        // payload の束縛は同名スロットをその arm の間だけ隠し、本体の `let` と
        // ともに他の arm へは漏らさない
        ExprKind::Match { subject, arms } => {
            scan(subject, slots, provided, locals, out);
            for arm in arms {
                let mut inner_locals = locals.clone();
                // `_` は名前を作らないので、外側のローカルとスロットがそのまま見える
                if let MatchPattern::Variant { bindings, .. } = &arm.pattern {
                    for binding in bindings {
                        if let PatternBinding::Bind(name) = binding {
                            inner_locals.insert(name.clone());
                        }
                    }
                }
                scan(&arm.body, slots, provided, &mut inner_locals, out);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record_access(
    name: &str,
    level: SlotLevel,
    span: Span,
    slots: &Slots,
    provided: &BTreeMap<String, SlotLevel>,
    locals: &BTreeSet<String>,
    out: &mut BodyFacts,
) {
    if !slots.is_slot(name) || locals.contains(name) {
        return;
    }
    if provided.get(name).is_some_and(|given| *given >= level) {
        return;
    }

    match out.escaping.get_mut(name) {
        Some(existing) if existing.level < level => *existing = SlotUse { level, span },
        Some(_) => {}
        None => {
            out.escaping
                .insert(name.to_string(), SlotUse { level, span });
        }
    }
}

fn trait_method_key(trait_name: &str, method: &str) -> String {
    format!("impl {trait_name}::{method}")
}

fn type_method_key(type_name: &str, method: &str) -> String {
    format!("impl {type_name}::{method}")
}

fn merge_facts(into: &mut BodyFacts, from: BodyFacts) {
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

// ---------------------------------------------------------------------------
// 手順5+6: 要求の伝播と到達経路
// ---------------------------------------------------------------------------

/// スロット名 → その要求がどこから来たかの経路。
///
/// 経路は呼び出しの連なりで、**末尾が実際に使っている関数**。
/// 空なら「この関数自身が使っている」。
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
    /// 経路の関数名だけ。1行表現と既存の一覧はこれで足りる。
    pub fn path_names(&self) -> Vec<&str> {
        self.path.iter().map(|hop| hop.name.as_str()).collect()
    }
}

pub type Reqs = BTreeMap<String, Requirement>;

#[derive(Debug)]
pub struct Analysis {
    pub slots: Slots,
    /// 関数名 → その関数が外へ要求するもの
    pub reqs: BTreeMap<String, Reqs>,
    /// 出力の並び順(宣言順)。BTreeMap の辞書順だと読みにくいため
    pub order: Vec<String>,
    diagnostics: Vec<Diag>,
}

/// テストの本体も関数と同じ扱いにする。名前がぶつからないよう印を付ける。
fn test_key(name: &str) -> String {
    format!("test \"{name}\"")
}

pub fn analyze(program: &Program) -> Analysis {
    let slots = collect_slots(program);
    let mut diagnostics = duplicate_slot_diagnostics(program);

    // 各関数・テスト・implメソッドの本体を1回だけ歩く。ここから先は AST を見ない
    let mut facts: BTreeMap<String, BodyFacts> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    for item in &program.items {
        match item {
            Item::Fn { sig, body, .. } => {
                let locals = sig.params.iter().map(|p| p.name.clone()).collect();
                facts.insert(
                    sig.name.clone(),
                    scan_body_with_locals(body, &slots, locals),
                );
                order.push(sig.name.clone());
            }
            Item::Test { name, body, .. } => {
                let key = test_key(name);
                facts.insert(key.clone(), scan_body(body, &slots));
                order.push(key);
            }
            Item::Impl {
                trait_name,
                type_name,
                methods,
                ..
            } => {
                for (sig, body) in methods {
                    let key = match trait_name {
                        Some(trait_name) => trait_method_key(trait_name, &sig.name),
                        None => type_method_key(type_name, &sig.name),
                    };
                    let mut locals: BTreeSet<String> =
                        sig.params.iter().map(|p| p.name.clone()).collect();
                    if sig.has_self {
                        locals.insert("self".to_string());
                    }
                    let method_facts = scan_body_with_locals(body, &slots, locals);
                    merge_facts(facts.entry(key).or_default(), method_facts.clone());
                    if trait_name.is_some() {
                        merge_facts(
                            facts
                                .entry(type_method_key(type_name, &sig.name))
                                .or_default(),
                            method_facts,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // v1 には extern / FFI の宣言構文がない。したがって、ここに定義がない直接
    // 呼び出しは外部関数とはみなさず typo として報告する。メソッドの存在確認は
    // レシーバの型が必要なので、型検査の段まで保留する。
    for (caller, body) in &facts {
        for site in &body.calls {
            if site.kind == CallKind::Direct && !facts.contains_key(&site.callee) {
                diagnostics.push(Diag::at(
                    site.span,
                    format!("{caller}: 関数 `{}` が定義されていません", site.callee),
                ));
            }
        }
    }
    diagnostics.sort();
    diagnostics.dedup();

    // 空から始めて、変化がなくなるまで回す。
    // 要求は増える一方(単調)なので必ず止まる。
    //
    // ponytail: 素朴な不動点反復。呼び出しグラフを Tarjan で SCC 縮約して
    // 逆位相順に舐めれば反復を減らせる(docs/adr/0003)。プログラムが
    // 大きくなって遅くなったら、そのときに入れ替える。
    let mut reqs: BTreeMap<String, Reqs> = BTreeMap::new();
    for name in facts.keys() {
        reqs.insert(name.clone(), Reqs::new());
    }

    loop {
        let mut changed = false;
        let mut updated: BTreeMap<String, Reqs> = BTreeMap::new();

        for (name, f) in &facts {
            let mut next = reqs[name].clone();

            // (1) この関数自身が使っていて、提供されていないもの。経路は空
            for (slot, used) in &f.escaping {
                changed |= merge_requirement(&mut next, slot, used.level, used.span, Vec::new());
            }

            // (2) 呼び出し先から上がってくるもの
            for site in &f.calls {
                // 定義されていない関数(FFI やまだ書いていないもの)は何も要求しない。
                // ponytail: 名前解決は未実装。未定義呼び出しをエラーにするのは別の段
                let Some(callee_reqs) = reqs.get(&site.callee) else {
                    continue;
                };

                for (slot, requirement) in callee_reqs {
                    // この呼び出し地点で提供されているなら、ここで止まる
                    if site
                        .provided
                        .get(slot)
                        .is_some_and(|given| *given >= requirement.level)
                    {
                        continue;
                    }

                    // このホップは「呼び出し元のどの呼び出しが要求を運んだか」なので、
                    // 名前は呼び先、位置は呼び出し地点になる
                    let mut path = vec![Hop {
                        name: site.callee.clone(),
                        span: site.span,
                    }];
                    path.extend(requirement.path.iter().cloned());
                    changed |= merge_requirement(
                        &mut next,
                        slot,
                        requirement.level,
                        requirement.span,
                        path,
                    );
                }
            }

            updated.insert(name.clone(), next);
        }

        reqs = updated;
        if !changed {
            break;
        }
    }

    Analysis {
        slots,
        reqs,
        order,
        diagnostics,
    }
}

fn duplicate_slot_diagnostics(program: &Program) -> Vec<Diag> {
    let mut declared: HashMap<&str, &str> = HashMap::new();
    let mut diagnostics = Vec::new();

    for item in &program.items {
        if let Item::Effect {
            slot, trait_name, ..
        } = item
            && let Some(previous_trait) = declared.insert(slot, trait_name)
        {
            diagnostics.push(Diag::at(
                item.span(),
                format!("effect `{slot}` が重複しています (`{previous_trait}` と `{trait_name}`)"),
            ));
        }
    }

    diagnostics
}

fn merge_requirement(
    reqs: &mut Reqs,
    slot: &str,
    level: SlotLevel,
    span: Span,
    path: Vec<Hop>,
) -> bool {
    match reqs.get_mut(slot) {
        Some(existing) if existing.level < level => {
            existing.level = level;
            existing.span = span;
            existing.path = path;
            true
        }
        Some(_) => false,
        None => {
            reqs.insert(slot.to_string(), Requirement { level, span, path });
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
    use crate::lex::{join, lex};
    use crate::parse;

    fn program(src: &str) -> Program {
        parse::parse(&join(lex(src).unwrap())).expect("パースできるはず")
    }

    /// 文言だけを見る検査のための取り出し。
    fn messages(diagnostics: &[Diag]) -> Vec<String> {
        diagnostics.iter().map(|d| d.msg.clone()).collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn facts_of(p: &Program, f: &str) -> BodyFacts {
        let slots = collect_slots(p);
        let (sig, body) = p
            .items
            .iter()
            .find_map(|i| match i {
                Item::Fn { sig, body, .. } if sig.name == f => Some((sig, body.as_slice())),
                _ => None,
            })
            .expect("その名前の関数がない");
        let locals = sig.params.iter().map(|p| p.name.clone()).collect();
        scan_body_with_locals(body, &slots, locals)
    }

    /// 提供されないまま漏れたスロット(手順2の検査対象)
    fn escaping(p: &Program, f: &str) -> BTreeSet<String> {
        facts_of(p, f).escaping.into_keys().collect()
    }

    /// 呼び出し辺の行き先(手順3の検査対象)。提供の有無はここでは見ない
    fn callees(p: &Program, f: &str) -> BTreeSet<String> {
        facts_of(p, f).calls.into_iter().map(|c| c.callee).collect()
    }

    // ---- 手順1 ----

    #[test]
    fn 手順1_スロット表を作る() {
        let p = program("effect db: Database\neffect clock: Clock\n");
        let slots = collect_slots(&p);

        assert_eq!(slots.trait_of("db"), Some("Database"));
        assert_eq!(slots.trait_of("clock"), Some("Clock"));
        assert_eq!(slots.trait_of("nope"), None);
        assert!(slots.is_slot("db"));
        assert!(!slots.is_slot("u"));
    }

    #[test]
    fn 手順1_正典のスロットは二つ() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let slots = collect_slots(&program(&src));
        assert_eq!(slots.names(), ["clock", "db"].into_iter().collect());
    }

    #[test]
    fn 重複したスロットを報告する() {
        let p = program("effect service: Clock\neffect service: Database\n");
        let errors = analyze(&p).errors();

        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].msg,
            "effect `service` が重複しています (`Clock` と `Database`)"
        );
        assert!(errors[0].span.is_some(), "重複した effect 宣言を指す");
    }

    #[test]
    fn 未定義の直接関数呼び出しを報告する() {
        let p = program("fn main() { stanp() }\n");
        let errors = analyze(&p).errors();

        assert_eq!(
            messages(&errors),
            vec!["main: 関数 `stanp` が定義されていません".to_string()]
        );
    }

    #[test]
    fn 後で定義された関数は呼び出せる() {
        let p = program("fn main() { stamp() }\nfn stamp() { 1 }\n");
        assert!(analyze(&p).errors().is_empty());
    }

    #[test]
    fn メソッドの存在確認は未定義関数検査に含めない() {
        let p = program("fn helper() { Missing::new() }\n");
        assert!(analyze(&p).errors().is_empty());
    }

    // ---- 手順2 ----

    #[test]
    fn 手順2_直接使っているスロットを集める() {
        let p = program(
            "effect db: Database\n\
             effect clock: Clock\n\
             fn stamp(u: User) {\n\
             \x20 u.promoted_at = clock.now()\n\
             \x20 db.save(u)\n\
             }\n",
        );
        assert_eq!(escaping(&p, "stamp"), set(&["clock", "db"]));
    }

    #[test]
    fn 手順2_ローカル変数のフィールドは要求ではない() {
        let p = program(
            "effect db: Database\n\
             fn f(u: User) {\n\
             \x20 u.rank\n\
             }\n",
        );
        assert!(escaping(&p, "f").is_empty());
    }

    #[test]
    fn 手順2_optional_fieldもレシーバの要求だけを集める() {
        let p = program(
            "effect db: Database\n\
             fn f() {\n\
             \x20 db.?state\n\
             }\n",
        );
        assert_eq!(escaping(&p, "f"), set(&["db"]));
        assert!(callees(&p, "f").is_empty());
    }

    // ---- 手順3 ----

    #[test]
    fn 手順3_呼んでいる関数の名前を集める() {
        let p = program(
            "fn stamp(u: User) {\n\
             \x20 1\n\
             }\n\
             fn promote(id: int -> bool) {\n\
             \x20 stamp(u)\n\
             \x20 audit(u)\n\
             \x20 true\n\
             }\n",
        );
        assert_eq!(callees(&p, "promote"), set(&["audit", "stamp"]));
    }

    #[test]
    fn 手順3_メソッド呼び出しもimplへの辺になる() {
        let p = program(
            "effect db: Database\n\
             fn f(id: int) {\n\
             \x20 db.save(id)\n\
             \x20 let x = Postgres::new(id)\n\
             \x20 stamp(x)\n\
             }\n",
        );
        assert_eq!(
            callees(&p, "f"),
            set(&["impl Database::save", "impl Postgres::new", "stamp"])
        );
    }

    #[test]
    fn 手順3_提供は呼び出しではない() {
        // `with db(pg) { ... }` は提供であって呼び出しではない。辺にしてはいけない
        let p = program(
            "effect db: Database\n\
             fn main() {\n\
             \x20 with db(pg) {\n\
             \x20   handle(id)\n\
             \x20 }\n\
             }\n",
        );
        assert_eq!(callees(&p, "main"), set(&["handle"]));
    }

    // ---- 手順4 ----

    #[test]
    fn 手順4_提供されていれば要求は消える() {
        let p = program(
            "effect db: Database\n\
             fn f() {\n\
             \x20 with db(pg) {\n\
             \x20   db.save(u)\n\
             \x20 }\n\
             }\n",
        );
        assert!(escaping(&p, "f").is_empty());
    }

    #[test]
    fn 手順4_提供されていないものは残る() {
        let p = program(
            "effect db: Database\n\
             effect clock: Clock\n\
             fn f() {\n\
             \x20 with db(pg) {\n\
             \x20   db.save(u)\n\
             \x20   clock.now()\n\
             \x20 }\n\
             }\n",
        );
        assert_eq!(escaping(&p, "f"), set(&["clock"]));
    }

    #[test]
    fn 手順4_提供はブロックの中だけ() {
        let p = program(
            "effect db: Database\n\
             fn f() {\n\
             \x20 with db(pg) {\n\
             \x20   db.save(u)\n\
             \x20 }\n\
             \x20 db.find(id)\n\
             }\n",
        );
        assert_eq!(escaping(&p, "f"), set(&["db"]));
    }

    #[test]
    fn 手順4_提供する値は提供の外で評価される() {
        // `db(make(clock.now()))` の clock.now() は、db が使えるようになる前に走る。
        // よって clock はこの提供では覆われない
        let p = program(
            "effect db: Database\n\
             effect clock: Clock\n\
             fn f() {\n\
             \x20 with db(make(clock.now())) {\n\
             \x20   db.save(u)\n\
             \x20 }\n\
             }\n",
        );
        assert_eq!(escaping(&p, "f"), set(&["clock"]));
    }

    #[test]
    fn 手順4_呼び出し地点の提供が記録される() {
        let p = program(
            "effect db: Database\n\
             fn f() {\n\
             \x20 handle(a)\n\
             \x20 with db(pg) {\n\
             \x20   handle(b)\n\
             \x20 }\n\
             }\n",
        );
        let facts = facts_of(&p, "f");

        assert_eq!(facts.calls.len(), 2);
        assert!(facts.calls[0].provided.is_empty());
        assert_eq!(facts.calls[1].provided.get("db"), Some(&SlotLevel::Value));
    }

    /// どの arm も実行されうるので、全 arm の直接使用と呼び出し辺が合流する。
    /// 対象自身の使用も落とさない(design.md 決定4)
    #[test]
    fn 全てのarmの要求と呼び出し辺が合流する() {
        let p = program(
            "effect db: Database\n\
             effect clock: Clock\n\
             fn f(r: Rank) {\n\
             \x20 match pick(r) {\n\
             \x20   Rank::Bronze: clock.now()\n\
             \x20   Rank::Gold {\n\
             \x20     stamp(r)\n\
             \x20   }\n\
             \x20 }\n\
             }\n\
             fn pick(r: Rank) { db.find(r) }\n\
             fn stamp(r: Rank) { 1 }\n",
        );
        assert_eq!(escaping(&p, "f"), set(&["clock"]));
        assert_eq!(callees(&p, "f"), set(&["impl Clock::now", "pick", "stamp"]));

        // 経路も既存の `if` と同じ形で伝わる
        let reqs = &analyze(&p).reqs["f"];
        assert_eq!(reqs["db"].path_names(), vec!["pick"]);
        assert!(reqs["clock"].path.is_empty());
    }

    #[test]
    fn armの中の提供はその本体だけを覆う() {
        let p = program(
            "effect db: Database\n\
             fn f(r: Rank) {\n\
             \x20 match r {\n\
             \x20   Rank::Bronze {\n\
             \x20     with db(pg) { db.save(u) }\n\
             \x20   }\n\
             \x20   Rank::Gold: db.find(id)\n\
             \x20 }\n\
             }\n",
        );
        assert_eq!(escaping(&p, "f"), set(&["db"]));
    }

    /// payload の名前は同名スロットをその arm の間だけ隠す。隣の arm と
    /// 後続の要求は残る(design.md 決定5)
    #[test]
    fn payload束縛はそのarmの中だけスロットを隠す() {
        let p = program(
            "effect db: Database\n\
             effect clock: Clock\n\
             fn f(l: Lookup) {\n\
             \x20 match l {\n\
             \x20   Lookup::Found(db): db.save(u)\n\
             \x20   Lookup::Missing(_): clock.now()\n\
             \x20   Lookup::Skipped: db.find(id)\n\
             \x20 }\n\
             }\n",
        );
        // 隠した arm は要求を作らないが、隠していない arm の分は残る
        assert_eq!(escaping(&p, "f"), set(&["db", "clock"]));
        assert_eq!(
            callees(&p, "f"),
            set(&["impl Clock::now", "impl Database::find"])
        );
    }

    #[test]
    fn payload束縛が全てのarmでスロットを隠せば要求は消える() {
        let p = program(
            "effect db: Database\n\
             fn f(l: Lookup) {\n\
             \x20 match l {\n\
             \x20   Lookup::Found(db): db.save(u)\n\
             \x20   Lookup::Missing(db): db.find(id)\n\
             \x20 }\n\
             }\n",
        );
        assert!(escaping(&p, "f").is_empty());
    }

    /// `_` は名前を作らないので、何も隠さない
    #[test]
    fn discardはスロットを隠さない() {
        let p = program(
            "effect db: Database\n\
             fn f(l: Lookup) {\n\
             \x20 match l {\n\
             \x20   Lookup::Found(_): db.save(u)\n\
             \x20 }\n\
             }\n",
        );
        assert_eq!(escaping(&p, "f"), set(&["db"]));
    }

    /// arm は束縛を導入しないが、本体の `let` は隣の arm へ漏らさない。
    /// 漏れるとスロットを隠してしまい、要求が消える
    #[test]
    fn armの束縛は隣のarmへ漏れない() {
        let p = program(
            "effect db: Database\n\
             fn f(r: Rank) {\n\
             \x20 match r {\n\
             \x20   Rank::Bronze {\n\
             \x20     let db = 1\n\
             \x20     db\n\
             \x20   }\n\
             \x20   Rank::Gold: db.find(id)\n\
             \x20 }\n\
             }\n",
        );
        assert_eq!(escaping(&p, "f"), set(&["db"]));
    }

    // ---- 手順5 + 6 ----

    #[test]
    fn 手順5_要求は呼び出しを遡って伝わる() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let a = analyze(&program(&src));

        // stamp が clock と db を使い、promote / handle は1文字も書いていないのに届く
        assert_eq!(a.reqs["stamp"].keys().count(), 2);
        assert_eq!(a.reqs["promote"].keys().count(), 2);
        assert_eq!(a.reqs["handle"].keys().count(), 2);

        // main と test は提供しているので要求ゼロ
        assert!(a.reqs["main"].is_empty());
    }

    #[test]
    fn 手順6_到達経路が付く() {
        let p = program(
            "effect clock: Clock\n\
             fn stamp(u: User) {\n\
             \x20 clock.now()\n\
             }\n\
             fn promote(id: int) {\n\
             \x20 stamp(id)\n\
             }\n\
             fn handle(id: int) {\n\
             \x20 promote(id)\n\
             }\n",
        );
        let a = analyze(&p);

        assert!(a.reqs["stamp"]["clock"].path_names().is_empty());
        assert_eq!(a.reqs["promote"]["clock"].path_names(), vec!["stamp"]);
        assert_eq!(
            a.reqs["handle"]["clock"].path_names(),
            vec!["promote", "stamp"]
        );
    }

    #[test]
    fn 手順6_提供忘れが経路付きで報告される() {
        let p = program(
            "effect clock: Clock\n\
             fn stamp(u: User) {\n\
             \x20 clock.now()\n\
             }\n\
             fn promote(id: int) {\n\
             \x20 stamp(id)\n\
             }\n\
             fn main() {\n\
             \x20 promote(1)\n\
             }\n",
        );
        let errors = analyze(&p).unsatisfied();

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
        let src = "effect clock: Clock\nfn main() { clock.now() }\n";
        let errors = analyze(&program(src)).unsatisfied();

        assert_eq!(errors.len(), 1);
        assert_eq!(slice(src, errors[0].span.unwrap()), "clock.now");
        assert!(errors[0].related.is_empty(), "{:?}", errors[0].related);
        assert_eq!(errors[0].help.as_deref(), Some("clock が要る ← main"));
    }

    /// 経路の各ホップは、その要求を運ぶ**呼び出し**を指す。使用地点は
    /// 一番下の関数の中にあり、ホップはそれを呼んだ側の位置になる。
    #[test]
    fn 手順6_経由した提供忘れはホップごとに呼び出しを指す() {
        let src = "effect clock: Clock\n                   fn stamp() { clock.now() }\n                   fn promote() { stamp() }\n                   fn main() { promote() }\n";
        let errors = analyze(&program(src)).unsatisfied();

        assert_eq!(errors.len(), 1);
        assert_eq!(slice(src, errors[0].span.unwrap()), "clock.now");
        let hops: Vec<&str> = errors[0]
            .related
            .iter()
            .map(|hop| slice(src, hop.span.unwrap()))
            .collect();
        assert_eq!(hops, vec!["stamp()", "promote()"]);
    }

    #[test]
    fn 手順6_正典には提供忘れがない() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let errors = analyze(&program(&src)).unsatisfied();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn 手順5_相互再帰でも止まる() {
        let p = program(
            "effect clock: Clock\n\
             fn ping(n: int) {\n\
             \x20 clock.now()\n\
             \x20 pong(n)\n\
             }\n\
             fn pong(n: int) {\n\
             \x20 ping(n)\n\
             }\n",
        );
        let a = analyze(&p);
        assert!(a.reqs["ping"].contains_key("clock"));
        assert!(a.reqs["pong"].contains_key("clock"));
    }

    #[test]
    fn 手順2_呼び出し先には潜らない() {
        // promote は db を直接使うが、stamp の中の clock は拾わない
        let p = program(
            "effect db: Database\n\
             effect clock: Clock\n\
             fn stamp(u: User) {\n\
             \x20 clock.now()\n\
             }\n\
             fn promote(id: int -> bool) {\n\
             \x20 let u = db.find(id)\n\
             \x20 stamp(u)\n\
             \x20 true\n\
             }\n",
        );
        assert_eq!(escaping(&p, "promote"), set(&["db"]));
    }

    #[test]
    fn 同名の引数はスロットを隠す() {
        let p = program(
            "effect db: Database\n\
             fn f(db: LocalDb) {\n\
             \x20 db.save(user)\n\
             }\n",
        );
        assert!(escaping(&p, "f").is_empty());
    }

    #[test]
    fn withの本体では提供したスロットが同名ローカルを隠す() {
        let p = program(
            "effect db: Database\n\
             fn f(db: LocalDb) {\n\
             \x20 with db(db) { db.save(user) }\n\
             }\n",
        );
        assert!(escaping(&p, "f").is_empty());
    }

    #[test]
    fn 型射影と値射影は異なる強さの要求になる() {
        let p = program(
            "effect db: Database\n\
             fn make() { db::new() }\n\
             fn use() { db.save(user) }\n",
        );
        let analysis = analyze(&p);
        assert_eq!(analysis.reqs["make"]["db"].level, SlotLevel::Type);
        assert_eq!(analysis.reqs["use"]["db"].level, SlotLevel::Value);
        assert!(analysis.render().contains("make / db (型)"));
    }

    #[test]
    fn 型提供は型要求だけを満たす() {
        let p = program(
            "effect db: Database\n\
             fn make() { db::new() }\n\
             fn use() { db.save(user) }\n\
             fn typed() { with db<Postgres> { make() } }\n\
             fn valued() { with db<Postgres> { use() } }\n",
        );
        let analysis = analyze(&p);
        assert!(analysis.reqs["typed"].is_empty());
        assert_eq!(analysis.reqs["valued"]["db"].level, SlotLevel::Value);
    }

    #[test]
    fn 実体提供は型要求と値要求の両方を満たす() {
        let p = program(
            "effect db: Database\n\
             fn make() { db::new() }\n\
             fn use() { db.save(user) }\n\
             fn f() {\n\
             \x20 with db(store) {\n\
             \x20   make()\n\
             \x20   use()\n\
             \x20 }\n\
             }\n",
        );
        assert!(analyze(&p).reqs["f"].is_empty());
    }

    #[test]
    fn 実行されないhead本体のletは後続のスロットを隠さない() {
        let p = program(
            "effect db: Database\n\
             fn f(local: LocalDb) {\n\
             \x20 if false { let db = local }\n\
             \x20 db.save(user)\n\
             }\n",
        );
        assert_eq!(analysis_level(&p, "f", "db"), Some(SlotLevel::Value));
    }

    #[test]
    fn impl本体の要求はスロット呼び出し元へ伝わる() {
        let p = program(
            "trait Database { fn save(self) }\n\
             trait Clock { fn now(self -> int) }\n\
             effect db: Database\n\
             effect clock: Clock\n\
             struct Store {}\n\
             impl Database for Store {\n\
             \x20 fn save(self) { clock.now() }\n\
             }\n\
             fn main() {\n\
             \x20 with db(Store) { db.save() }\n\
             }\n",
        );
        assert_eq!(analyze(&p).reqs["main"]["clock"].level, SlotLevel::Value);
    }

    #[test]
    fn impl本体の要求は型パス呼び出し元へ伝わる() {
        let p = program(
            "trait Clock { fn now(self -> int) }\n\
             effect clock: Clock\n\
             struct Store {}\n\
             impl Store {\n\
             \x20 fn new(-> Store) {\n\
             \x20   clock.now()\n\
             \x20   Store\n\
             \x20 }\n\
             }\n\
             fn main() { Store::new() }\n",
        );
        assert_eq!(analyze(&p).reqs["main"]["clock"].level, SlotLevel::Value);
    }

    fn analysis_level(program: &Program, function: &str, slot: &str) -> Option<SlotLevel> {
        analyze(program).reqs[function].get(slot).map(|r| r.level)
    }
}
