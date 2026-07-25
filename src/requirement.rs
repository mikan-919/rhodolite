//! 要求推論。「この関数はどのスロットを外から要求するか」を求める。
//!
//! 完成形のアルゴリズム(docs/adr/0003 より):
//!
//!   req(f) = (直接使用 ∪ ⋃ req(呼び出し先)) − そのスコープで提供されているもの
//!
//! 依存が呼ばれる側にしか向かわないので、Tarjan で強連結成分に縮約すれば
//! 逆位相順の1パスで済む(不動点反復は不要)。
//!
//! ただし一気に作らない。下の順で1段ずつ緑にしていく:
//!
//!   1. collect_slots  … スロット表を作る
//!   2. direct_uses    … 1関数の直接使用を集める(呼び出し先に潜らない)
//!   3. calls          … 呼び出し辺を集める
//!   4. 提供による打ち消し
//!   5. SCC 縮約 + 逆位相1パス
//!   6. 到達経路

use crate::ast::{Expr, ExprKind, Head, Item, Program};
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
        match x {
            Item::Effect {
                slot,
                trait_name,
                span: _,
            } => {
                // TODO: インサートでダブったらエラーにする
                map.insert(slot.clone(), trait_name.clone());
            }
            _ => (),
        }
    }
    Slots { map: map }
}

// ---------------------------------------------------------------------------
// 手順2: 直接使用
// ---------------------------------------------------------------------------

/// 1つの関数の本体を歩いて、**その関数が自分で直接使っている**スロットを集める。
///
/// - 呼び出し先の関数には潜らない(それは手順3〜5の仕事)
/// - 提供 (`db(pg): { ... }`) による打ち消しもまだ考えない(手順4)
///
/// 探すのはこの形:
///
/// ```text
/// db.save(u)  →  Call( Field( Ident("db"), "save" ), [u] )
///                      ~~~~~~~~~~~~~~~~~~~ ここの "db" が Slots にあれば要求
/// ```
///
/// ヒント: 下の `walk` で全部の部分式を訪問し、`ExprKind::Field(recv, _)` を見つけて
/// `recv` が `ExprKind::Ident(name)` かつ `slots.is_slot(name)` なら集める。
pub fn direct_uses(body: &[Expr], slots: &Slots) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for expr in body {
        walk(expr, &mut |e| {
            if let ExprKind::Field(recv, _) = &e.kind {
                if let ExprKind::Ident(name) = &recv.kind {
                    if slots.is_slot(name) {
                        result.insert(name.clone());
                    }
                }
            }
        });
    }
    result
}

// ---------------------------------------------------------------------------
// 手順3: 呼び出し辺
// ---------------------------------------------------------------------------

/// 1つの関数の本体を歩いて、**この関数が呼んでいる関数の名前**を集める。
/// これが呼び出しグラフの辺になる。
///
/// 集めるのはこの形だけ:
///
/// ```text
/// stamp(u)              →  Call( Ident("stamp"), [u] )        集める
/// db.save(u)            →  Call( Field(..), [u] )             集めない(スロット使用)
/// Postgres::new(url)    →  Call( Path([..]), [url] )          集めない(パス呼び出し)
/// ```
///
/// 定義されている関数かどうかの絞り込みはここではしない(グラフを組むときの仕事)。
pub fn calls(body: &[Expr], slots: &Slots) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for x in body {
        walk(x, &mut |x| {
            if let ExprKind::Call(callee, _) = &x.kind
                && let ExprKind::Ident(name) = &callee.kind
            {
                if slots.is_slot(name) {
                    return;
                }
                result.insert(name.clone());
            }
        })
    }
    result
    // todo!("手順3: Call(Ident(name), _) の name を集める")
}

// ---------------------------------------------------------------------------
// 手順4: 提供による打ち消し
// ---------------------------------------------------------------------------

/// 呼び出し1つ分の記録。
///
/// 「どの関数を呼んだか」だけでは足りない。`db(pg): { handle(id) }` のように
/// 提供の中で呼ばれていると、`handle` から来る db 要求はここで消えるため、
/// **その地点で何が提供されていたか**を一緒に覚えておく必要がある。
#[derive(Debug, Clone)]
pub struct CallSite {
    pub callee: String,
    pub provided: BTreeSet<String>,
}

/// 関数の本体を1回歩いて分かること。
#[derive(Debug, Default)]
pub struct BodyFacts {
    /// 提供されないまま漏れた直接使用
    pub escaping: BTreeSet<String>,
    pub calls: Vec<CallSite>,
}

/// 本体を1回歩いて `BodyFacts` を作る。
pub fn scan_body(body: &[Expr], slots: &Slots) -> BodyFacts {
    let mut facts = BodyFacts::default();
    let nothing_provided = BTreeSet::new();
    for e in body {
        scan(e, slots, &nothing_provided, &mut facts);
    }
    facts
}

/// `walk` を使わず自前で再帰する。理由は `provided` を枝ごとに変えて下へ運ぶため。
///
/// **スコープの終わりを書く必要がない**ことに注目。`scan(body, &inner, ..)` から
/// 戻った時点で `inner` は消えていて、呼び出し元は元の `provided` を持ったまま。
/// 字句スコープを呼び出しスタックがそのまま表現している。
fn scan(e: &Expr, slots: &Slots, provided: &BTreeSet<String>, out: &mut BodyFacts) {
    match &e.kind {
        // --- ここが本題 ---
        ExprKind::Head { head, body, orelse } => {
            match head {
                Head::Ambient(binders) => {
                    // binders はブロックの「外」で評価される。
                    // `db(Postgres::new(url))` の `Postgres::new(url)` は
                    // db が使えるようになる前に走るので、provided は増やさない。
                    for b in binders {
                        scan(b, slots, provided, out);
                    }

                    // ブロックの中だけ、提供されたスロットが増える
                    let mut inner = provided.clone();
                    for b in binders {
                        if let Some(name) = provided_slot_name(b) {
                            inner.insert(name);
                        }
                    }
                    scan(body, slots, &inner, out);
                }
                Head::If(c) | Head::Elif(c) | Head::While(c) => {
                    scan(c, slots, provided, out);
                    scan(body, slots, provided, out);
                }
                Head::For { iter, .. } => {
                    scan(iter, slots, provided, out);
                    scan(body, slots, provided, out);
                }
                Head::Else => {
                    scan(body, slots, provided, out);
                }
            }
            if let Some(o) = orelse {
                scan(o, slots, provided, out);
            }
        }

        // --- スロットの使用 ---
        ExprKind::Field(recv, _) => {
            if let ExprKind::Ident(name) = &recv.kind {
                if slots.is_slot(name) && !provided.contains(name) {
                    out.escaping.insert(name.clone());
                }
            }
            scan(recv, slots, provided, out);
        }

        // --- 呼び出し辺 ---
        ExprKind::Call(callee, args) => {
            if let ExprKind::Ident(name) = &callee.kind {
                if !slots.is_slot(name) {
                    out.calls.push(CallSite {
                        callee: name.clone(),
                        provided: provided.clone(),
                    });
                }
            }
            scan(callee, slots, provided, out);
            for a in args {
                scan(a, slots, provided, out);
            }
        }

        // --- 以下は素通り。子に同じ provided を渡すだけ ---
        ExprKind::Int(_) | ExprKind::Str(_) | ExprKind::Bool(_) => {}
        ExprKind::Ident(_) | ExprKind::Path(_) => {}
        ExprKind::Array(items) => {
            for i in items {
                scan(i, slots, provided, out);
            }
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, v) in fields {
                scan(v, slots, provided, out);
            }
        }
        ExprKind::Let { value, .. } => scan(value, slots, provided, out),
        ExprKind::Assign { target, value } => {
            scan(target, slots, provided, out);
            scan(value, slots, provided, out);
        }
        ExprKind::Unary(_, inner) => scan(inner, slots, provided, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            scan(lhs, slots, provided, out);
            scan(rhs, slots, provided, out);
        }
        ExprKind::Return(Some(v)) => scan(v, slots, provided, out),
        ExprKind::Return(None) => {}
        ExprKind::Assert(inner) => scan(inner, slots, provided, out),
        ExprKind::Block(body) => {
            for e in body {
                scan(e, slots, provided, out);
            }
        }
    }
}

/// `db(pg)` という提供から、スロット名 `db` を取り出す。
fn provided_slot_name(binder: &Expr) -> Option<String> {
    if let ExprKind::Call(callee, _) = &binder.kind {
        if let ExprKind::Ident(name) = &callee.kind {
            return Some(name.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 木歩きの道具(ボイラープレート)
// ---------------------------------------------------------------------------

/// 式とそのすべての部分式を前順(自分 → 子)で訪問する。
///
/// 注意: これは**手順2と3までの道具**。手順4で「いま提供されているスロットの集合」を
/// 持ち回る必要が出ると、この形では足りなくなる(訪問中に状態を引き継げないため)。
/// そのときは `walk` を使うのをやめて、自分で再帰関数を書くことになる。
pub fn walk(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Int(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Ident(_) => {}
        ExprKind::Path(_) => {}
        ExprKind::Field(recv, _) => walk(recv, f),
        ExprKind::Call(callee, args) => {
            walk(callee, f);
            for a in args {
                walk(a, f);
            }
        }
        ExprKind::Array(items) => {
            for i in items {
                walk(i, f);
            }
        }
        ExprKind::StructLit { fields, .. } => {
            for (_, v) in fields {
                walk(v, f);
            }
        }
        ExprKind::Let { value, .. } => walk(value, f),
        ExprKind::Assign { target, value } => {
            walk(target, f);
            walk(value, f);
        }
        ExprKind::Unary(_, inner) => walk(inner, f),
        ExprKind::Binary { lhs, rhs, .. } => {
            walk(lhs, f);
            walk(rhs, f);
        }
        ExprKind::Return(Some(v)) => walk(v, f),
        ExprKind::Return(None) => {}
        ExprKind::Assert(inner) => walk(inner, f),
        ExprKind::Block(body) => {
            for e in body {
                walk(e, f);
            }
        }
        ExprKind::Head { head, body, orelse } => {
            walk_head(head, f);
            walk(body, f);
            if let Some(o) = orelse {
                walk(o, f);
            }
        }
    }
}

fn walk_head(h: &crate::ast::Head, f: &mut impl FnMut(&Expr)) {
    use crate::ast::Head::*;
    match h {
        If(c) | Elif(c) | While(c) => walk(c, f),
        For { iter, .. } => walk(iter, f),
        Else => {}
        Ambient(binders) => {
            for b in binders {
                walk(b, f);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 手順5+6: 要求の伝播と到達経路
// ---------------------------------------------------------------------------

/// スロット名 → その要求がどこから来たかの経路。
///
/// 経路は呼び出しの連なりで、**末尾が実際に使っている関数**。
/// 空なら「この関数自身が使っている」。
/// `{"clock": ["handle", "promote", "stamp"]}` は
/// 「clock が要る ← handle ← promote ← stamp」と読む。
pub type Reqs = BTreeMap<String, Vec<String>>;

#[derive(Debug)]
pub struct Analysis {
    pub slots: Slots,
    /// 関数名 → その関数が外へ要求するもの
    pub reqs: BTreeMap<String, Reqs>,
    /// 出力の並び順(宣言順)。BTreeMap の辞書順だと読みにくいため
    pub order: Vec<String>,
}

/// テストの本体も関数と同じ扱いにする。名前がぶつからないよう印を付ける。
fn test_key(name: &str) -> String {
    format!("test \"{name}\"")
}

pub fn analyze(program: &Program) -> Analysis {
    let slots = collect_slots(program);

    // 各関数の本体を1回だけ歩く。ここから先は AST を見ない
    let mut facts: BTreeMap<String, BodyFacts> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    for item in &program.items {
        match item {
            Item::Fn { sig, body, .. } => {
                facts.insert(sig.name.clone(), scan_body(body, &slots));
                order.push(sig.name.clone());
            }
            Item::Test { name, body, .. } => {
                let key = test_key(name);
                facts.insert(key.clone(), scan_body(body, &slots));
                order.push(key);
            }
            _ => {}
        }
    }

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
            for slot in &f.escaping {
                if !next.contains_key(slot) {
                    next.insert(slot.clone(), Vec::new());
                    changed = true;
                }
            }

            // (2) 呼び出し先から上がってくるもの
            for site in &f.calls {
                // 定義されていない関数(FFI やまだ書いていないもの)は何も要求しない。
                // ponytail: 名前解決は未実装。未定義呼び出しをエラーにするのは別の段
                let Some(callee_reqs) = reqs.get(&site.callee) else {
                    continue;
                };

                for (slot, path_below) in callee_reqs {
                    // この呼び出し地点で提供されているなら、ここで止まる
                    if site.provided.contains(slot) {
                        continue;
                    }
                    // 最初に見つかった経路を採る。回を追うごとに深くなるので、
                    // 最初に見つかるものが最短になる
                    if next.contains_key(slot) {
                        continue;
                    }

                    let mut path = Vec::new();
                    path.push(site.callee.clone());
                    for step in path_below {
                        path.push(step.clone());
                    }
                    next.insert(slot.clone(), path);
                    changed = true;
                }
            }

            updated.insert(name.clone(), next);
        }

        reqs = updated;
        if !changed {
            break;
        }
    }

    Analysis { slots, reqs, order }
}

impl Analysis {
    /// 推論結果の一覧。IDE がゴーストテキストで見せるものの、テキスト版。
    pub fn render(&self) -> String {
        let mut out = String::new();
        for name in &self.order {
            let reqs = &self.reqs[name];
            if reqs.is_empty() {
                out.push_str(&format!("  {name} / (要求なし)\n"));
            } else {
                let names: Vec<&str> = reqs.keys().map(|s| s.as_str()).collect();
                out.push_str(&format!("  {name} / {}\n", names.join(", ")));
            }
        }
        out
    }

    /// エントリ(`main` とテスト)に要求が残っていたら、それは提供忘れ。
    ///
    /// 到達経路を添えて返す。原因は数階層下にあるので、経路がないと直せない。
    pub fn unsatisfied(&self) -> Vec<String> {
        let mut errors = Vec::new();

        for name in &self.order {
            let is_entry = name == "main" || name.starts_with("test \"");
            if !is_entry {
                continue;
            }

            for (slot, path) in &self.reqs[name] {
                let mut line = format!("{name}: `{slot}` が提供されていません\n");
                line.push_str(&format!("  {slot} が要る"));
                for step in path {
                    line.push_str(&format!(" ← {step}"));
                }
                errors.push(line);
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

    fn body_of<'a>(p: &'a Program, name: &str) -> &'a [Expr] {
        p.items
            .iter()
            .find_map(|i| match i {
                Item::Fn { sig, body, .. } if sig.name == name => Some(body.as_slice()),
                _ => None,
            })
            .expect("その名前の関数がない")
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
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
        let slots = collect_slots(&p);
        assert_eq!(
            direct_uses(body_of(&p, "stamp"), &slots),
            set(&["clock", "db"])
        );
    }

    #[test]
    fn 手順2_ローカル変数のフィールドは要求ではない() {
        let p = program(
            "effect db: Database\n\
             fn f(u: User) {\n\
             \x20 u.rank\n\
             }\n",
        );
        let slots = collect_slots(&p);
        assert!(direct_uses(body_of(&p, "f"), &slots).is_empty());
    }

    // ---- 手順3 ----

    #[test]
    fn 手順3_呼んでいる関数の名前を集める() {
        let p = program(
            "fn stamp(u: User) {\n\
             \x20 1\n\
             }\n\
             fn promote(id: UserId -> bool) {\n\
             \x20 stamp(u)\n\
             \x20 audit(u)\n\
             \x20 true\n\
             }\n",
        );
        let slots = collect_slots(&p);
        assert_eq!(
            calls(body_of(&p, "promote"), &slots),
            set(&["audit", "stamp"])
        );
    }

    #[test]
    fn 手順3_スロット使用とパス呼び出しは辺ではない() {
        let p = program(
            "effect db: Database\n\
             fn f(id: UserId) {\n\
             \x20 db.save(id)\n\
             \x20 let x = Postgres::new(id)\n\
             \x20 stamp(x)\n\
             }\n",
        );
        // db.save は Field 越し、Postgres::new は Path。辺になるのは stamp だけ
        let slots = collect_slots(&p);
        assert_eq!(calls(body_of(&p, "f"), &slots), set(&["stamp"]));
    }

    #[test]
    fn 手順3_提供は呼び出しではない() {
        // `db(pg): { ... }` の `db(pg)` は Call(Ident("db"), _) と同じ形をしているが、
        // これは提供であって呼び出しではない。辺にしてはいけない
        let p = program(
            "effect db: Database\n\
             fn main() {\n\
             \x20 db(pg): {\n\
             \x20   handle(id)\n\
             \x20 }\n\
             }\n",
        );
        let slots = collect_slots(&p);
        assert_eq!(calls(body_of(&p, "main"), &slots), set(&["handle"]));
    }

    // ---- 手順4 ----

    fn escaping(p: &Program, f: &str) -> BTreeSet<String> {
        let slots = collect_slots(p);
        scan_body(body_of(p, f), &slots).escaping
    }

    #[test]
    fn 手順4_提供されていれば要求は消える() {
        let p = program(
            "effect db: Database\n\
             fn f() {\n\
             \x20 db(pg): {\n\
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
             \x20 db(pg): {\n\
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
             \x20 db(pg): {\n\
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
             \x20 db(make(clock.now())): {\n\
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
             \x20 db(pg): {\n\
             \x20   handle(b)\n\
             \x20 }\n\
             }\n",
        );
        let slots = collect_slots(&p);
        let facts = scan_body(body_of(&p, "f"), &slots);

        assert_eq!(facts.calls.len(), 2);
        assert!(facts.calls[0].provided.is_empty());
        assert_eq!(facts.calls[1].provided, set(&["db"]));
    }

    /// mikan が手書きした `walk` 版と、`provided` を運ぶ版が
    /// 提供のないコードでは一致すること
    #[test]
    fn 手順4_提供がなければ手順2と一致する() {
        let src = "effect db: Database\n\
                   effect clock: Clock\n\
                   fn f(u: User) {\n\
                   \x20 clock.now()\n\
                   \x20 db.save(u)\n\
                   }\n";
        let p = program(src);
        let slots = collect_slots(&p);
        assert_eq!(
            direct_uses(body_of(&p, "f"), &slots),
            scan_body(body_of(&p, "f"), &slots).escaping
        );
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
             fn promote(id: UserId) {\n\
             \x20 stamp(id)\n\
             }\n\
             fn handle(id: UserId) {\n\
             \x20 promote(id)\n\
             }\n",
        );
        let a = analyze(&p);

        assert_eq!(a.reqs["stamp"]["clock"], Vec::<String>::new());
        assert_eq!(a.reqs["promote"]["clock"], vec!["stamp".to_string()]);
        assert_eq!(
            a.reqs["handle"]["clock"],
            vec!["promote".to_string(), "stamp".to_string()]
        );
    }

    #[test]
    fn 手順6_提供忘れが経路付きで報告される() {
        let p = program(
            "effect clock: Clock\n\
             fn stamp(u: User) {\n\
             \x20 clock.now()\n\
             }\n\
             fn promote(id: UserId) {\n\
             \x20 stamp(id)\n\
             }\n\
             fn main() {\n\
             \x20 promote(1)\n\
             }\n",
        );
        let errors = analyze(&p).unsatisfied();

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("clock が要る ← promote ← stamp"), "{}", errors[0]);
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
             fn ping(n: Int) {\n\
             \x20 clock.now()\n\
             \x20 pong(n)\n\
             }\n\
             fn pong(n: Int) {\n\
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
             fn promote(id: UserId -> bool) {\n\
             \x20 let u = db.find(id)\n\
             \x20 stamp(u)\n\
             \x20 true\n\
             }\n",
        );
        let slots = collect_slots(&p);
        assert_eq!(direct_uses(body_of(&p, "promote"), &slots), set(&["db"]));
    }
}
