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

use crate::ast::{Expr, ExprKind, Item, Program};
use std::collections::{BTreeSet, HashMap};

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
pub fn collect_slots(_program: &Program) -> Slots {
    todo!("手順1: Item::Effect を集めて Slots を返す")
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
pub fn direct_uses(_body: &[Expr], _slots: &Slots) -> BTreeSet<String> {
    todo!("手順2: 本体を歩いて、直接使っているスロット名を集める")
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
        assert_eq!(direct_uses(body_of(&p, "stamp"), &slots), set(&["clock", "db"]));
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
