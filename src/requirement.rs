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
// 手順2〜4: 直接使用・呼び出し辺・提供による打ち消し
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
///
/// 拾うのはこの2つの形だけ:
///
/// ```text
/// db.save(u)          →  Call( Field( Ident("db"), "save" ), [u] )
///                              ~~~~~~~~~~~~~~~~~~~ "db" が Slots にあれば escaping
/// stamp(u)            →  Call( Ident("stamp"), [u] )     calls の辺になる
/// db(pg): { ... }     →  Head( Ambient([Call(Ident("db"), [pg])]), .. )
///                        辺ではない。提供。ブロックの中だけ打ち消す
/// Postgres::new(url)  →  Call( Path([..]), [url] )       どちらでもない
/// ```
///
/// 呼び出し先には潜らない。定義されている関数かどうかの絞り込みもしない
/// (それは `analyze` の仕事)。
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

/// 汎用の木歩き(訪問関数を渡す形)ではなく自前で再帰する。理由は
/// `provided` を枝ごとに変えて下へ運ぶため — 訪問関数の形では状態を引き継げない。
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
/// 提供の形そのものは `Expr::as_provision`(ast.rs)が知っている。
fn provided_slot_name(binder: &Expr) -> Option<String> {
    binder.as_provision().map(|(slot, _)| slot.to_string())
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

    /// 提供されないまま漏れたスロット(手順2の検査対象)
    fn escaping(p: &Program, f: &str) -> BTreeSet<String> {
        let slots = collect_slots(p);
        scan_body(body_of(p, f), &slots).escaping
    }

    /// 呼び出し辺の行き先(手順3の検査対象)。提供の有無はここでは見ない
    fn callees(p: &Program, f: &str) -> BTreeSet<String> {
        let slots = collect_slots(p);
        scan_body(body_of(p, f), &slots)
            .calls
            .into_iter()
            .map(|c| c.callee)
            .collect()
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
        assert_eq!(callees(&p, "promote"), set(&["audit", "stamp"]));
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
        assert_eq!(callees(&p, "f"), set(&["stamp"]));
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
        assert_eq!(callees(&p, "main"), set(&["handle"]));
    }

    // ---- 手順4 ----

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
        assert_eq!(escaping(&p, "promote"), set(&["db"]));
    }
}
