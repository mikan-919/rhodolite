//! struct の形の検査。型検査の最初の縦切り(docs/overview.md「次の一歩」1)。
//!
//! モジュール解決済みの `Program` を受け取り、宣言と生成のフィールド集合だけを
//! 突き合わせる。ここを通ったプログラムでは
//!
//!   - 全ての struct リテラルが宣言済み struct を指し、宣言フィールドを
//!     過不足なく一度ずつ持つ
//!   - ローカルに隠されていない bare struct 値はフィールド0個
//!
//! が成り立つ。フィールド**値**の型、引数、戻り値、演算、optional、配列は
//! まだ見ない(design.md の Non-Goals)。
//!
//! 走査は `requirement::scan` と同じ字句スコープ規則を持つが、運ぶ状態が
//! 違う(あちらは提供集合、こちらはローカル名だけ)ので別に書いている。

use crate::ast::{Expr, ExprKind, Head, Item, Program, Provision};
use std::collections::{BTreeMap, BTreeSet};

/// 宣言の索引。正準名でそのまま引く。
struct Decls {
    /// struct 名 → 宣言フィールド名
    structs: BTreeMap<String, BTreeSet<String>>,
    /// struct ではない宣言名(trait / effect / fn)。
    /// 「未知」と「struct ではない」を言い分けるためだけに持つ
    others: BTreeSet<String>,
}

/// 診断を全件返す。空なら struct の形は正しい。
pub fn check(program: &Program) -> Vec<String> {
    let mut out = Vec::new();
    let decls = collect(program, &mut out);

    for item in &program.items {
        match item {
            Item::Fn { sig, body, .. } => {
                let locals = sig.params.iter().map(|p| p.name.clone()).collect();
                check_body(body, &decls, locals, &sig.name, &mut out);
            }
            Item::Test { name, body, .. } => {
                let ctx = format!("test \"{name}\"");
                check_body(body, &decls, BTreeSet::new(), &ctx, &mut out);
            }
            Item::Impl {
                type_name, methods, ..
            } => {
                for (sig, body) in methods {
                    let mut locals: BTreeSet<String> =
                        sig.params.iter().map(|p| p.name.clone()).collect();
                    if sig.has_self {
                        locals.insert("self".to_string());
                    }
                    let ctx = format!("impl {type_name}::{}", sig.name);
                    check_body(body, &decls, locals, &ctx, &mut out);
                }
            }
            _ => {}
        }
    }

    out
}

fn collect(program: &Program, out: &mut Vec<String>) -> Decls {
    let mut structs = BTreeMap::new();
    let mut others = BTreeSet::new();

    for item in &program.items {
        match item {
            Item::Struct { name, fields, .. } => {
                let mut declared = BTreeSet::new();
                let mut duplicates = BTreeSet::new();
                for (field, _) in fields {
                    if !declared.insert(field.clone()) {
                        duplicates.insert(field.clone());
                    }
                }
                for field in &duplicates {
                    out.push(format!(
                        "struct `{name}`: フィールド `{field}` が重複して宣言されています"
                    ));
                }
                structs.insert(name.clone(), declared);
            }
            Item::Trait { name, .. } => {
                others.insert(name.clone());
            }
            Item::Effect { slot, .. } => {
                others.insert(slot.clone());
            }
            Item::Fn { sig, .. } => {
                others.insert(sig.name.clone());
            }
            Item::Impl { .. } | Item::Test { .. } => {}
        }
    }

    Decls { structs, others }
}

fn check_body(
    body: &[Expr],
    decls: &Decls,
    mut locals: BTreeSet<String>,
    ctx: &str,
    out: &mut Vec<String>,
) {
    check_exprs(body, decls, &mut locals, ctx, out);
}

fn check_exprs(
    body: &[Expr],
    decls: &Decls,
    locals: &mut BTreeSet<String>,
    ctx: &str,
    out: &mut Vec<String>,
) {
    for e in body {
        check_expr(e, decls, locals, ctx, out);
    }
}

/// `requirement::scan` と同じく、ブロックから戻れば内側の束縛は消える。
/// 枝へ入るときだけ `locals` を複製する。
fn check_expr(
    e: &Expr,
    decls: &Decls,
    locals: &mut BTreeSet<String>,
    ctx: &str,
    out: &mut Vec<String>,
) {
    match &e.kind {
        ExprKind::Ident(name) => check_bare(name, decls, locals, ctx, out),

        ExprKind::StructLit { name, fields } => {
            check_literal(name, fields, decls, ctx, out);
            for (_, v) in fields {
                check_expr(v, decls, locals, ctx, out);
            }
        }

        ExprKind::Let { name, value } => {
            check_expr(value, decls, locals, ctx, out);
            locals.insert(name.clone());
        }

        ExprKind::Call(callee, args) => {
            // 呼び出し先の名前は値として読まれない。評価器も `Ident` / `Path` の
            // callee は関数表・型表から引くだけで、値へ落とさない
            if !matches!(callee.kind, ExprKind::Ident(_) | ExprKind::Path(_)) {
                check_expr(callee, decls, locals, ctx, out);
            }
            for a in args {
                check_expr(a, decls, locals, ctx, out);
            }
        }

        ExprKind::Field(recv, _) => check_expr(recv, decls, locals, ctx, out),

        ExprKind::Assign { target, value } => {
            check_expr(value, decls, locals, ctx, out);
            // 代入先の裸の名前は書き込み先であって値の読みではない
            if let ExprKind::Field(recv, _) = &target.kind {
                check_expr(recv, decls, locals, ctx, out);
            }
        }

        ExprKind::Head { head, body, orelse } => {
            match head {
                Head::Ambient(binders) => {
                    // 提供値は外側で評価される
                    for b in binders {
                        if let Provision::Value { value, .. } = b {
                            check_expr(value, decls, locals, ctx, out);
                        }
                    }
                    let mut inner = locals.clone();
                    for b in binders {
                        inner.insert(b.slot().to_string());
                    }
                    check_expr(body, decls, &mut inner, ctx, out);
                }
                Head::If(c) | Head::Elif(c) | Head::While(c) => {
                    check_expr(c, decls, locals, ctx, out);
                    check_expr(body, decls, &mut locals.clone(), ctx, out);
                }
                Head::For { var, iter } => {
                    check_expr(iter, decls, locals, ctx, out);
                    let mut inner = locals.clone();
                    inner.insert(var.clone());
                    check_expr(body, decls, &mut inner, ctx, out);
                }
                Head::Else => check_expr(body, decls, &mut locals.clone(), ctx, out),
            }
            if let Some(o) = orelse {
                check_expr(o, decls, &mut locals.clone(), ctx, out);
            }
        }

        ExprKind::Array(items) => {
            for i in items {
                check_expr(i, decls, locals, ctx, out);
            }
        }
        ExprKind::Block(body) => check_exprs(body, decls, locals, ctx, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            check_expr(lhs, decls, locals, ctx, out);
            check_expr(rhs, decls, locals, ctx, out);
        }
        ExprKind::Unary(_, inner) | ExprKind::Assert(inner) | ExprKind::Return(Some(inner)) => {
            check_expr(inner, decls, locals, ctx, out)
        }

        ExprKind::Int(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Nil
        | ExprKind::Path(_)
        | ExprKind::Return(None) => {}
    }
}

/// 裸の名前が値になれるのは、隠されていないフィールド0個の struct のときだけ。
fn check_bare(
    name: &str,
    decls: &Decls,
    locals: &BTreeSet<String>,
    ctx: &str,
    out: &mut Vec<String>,
) {
    if locals.contains(name) {
        return;
    }
    let Some(declared) = decls.structs.get(name) else {
        return;
    };
    if declared.is_empty() {
        return;
    }
    out.push(format!(
        "{ctx}: `{name}` はフィールドを {} 個持ちます。struct リテラルで生成してください",
        declared.len()
    ));
}

fn check_literal(
    name: &str,
    fields: &[(String, Expr)],
    decls: &Decls,
    ctx: &str,
    out: &mut Vec<String>,
) {
    let Some(declared) = decls.structs.get(name) else {
        if decls.others.contains(name) {
            out.push(format!("{ctx}: `{name}` は struct ではありません"));
        } else {
            out.push(format!("{ctx}: struct `{name}` は宣言されていません"));
        }
        return;
    };

    let mut given: BTreeSet<&str> = BTreeSet::new();
    let mut duplicates: BTreeSet<&str> = BTreeSet::new();
    for (field, _) in fields {
        if !given.insert(field) {
            duplicates.insert(field);
        }
    }
    for field in &duplicates {
        out.push(format!(
            "{ctx}: struct `{name}` のフィールド `{field}` を二度指定しています"
        ));
    }

    let missing: Vec<&str> = declared
        .iter()
        .map(String::as_str)
        .filter(|f| !given.contains(f))
        .collect();
    if !missing.is_empty() {
        out.push(format!(
            "{ctx}: struct `{name}` の生成にフィールド {} がありません",
            quoted(&missing)
        ));
    }

    let extra: Vec<&str> = given
        .iter()
        .copied()
        .filter(|f| !declared.contains(*f))
        .collect();
    if !extra.is_empty() {
        out.push(format!(
            "{ctx}: struct `{name}` に宣言されていないフィールド {} を与えています",
            quoted(&extra)
        ));
    }
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::{join, lex};
    use crate::parse;

    fn errors(src: &str) -> Vec<String> {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        check(&program)
    }

    fn only(src: &str) -> String {
        let errors = errors(src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        errors.into_iter().next().unwrap()
    }

    // ---- 1. 宣言 ----

    #[test]
    fn 重複した宣言フィールドを報告する() {
        let e = only("struct User { rank: Rank\nrank: Rank }\n");
        assert!(e.contains("struct `User`"), "{e}");
        assert!(e.contains("`rank`"), "{e}");
    }

    #[test]
    fn 相異なる宣言フィールドは診断を出さない() {
        assert!(errors("struct User { id: UserId\nrank: Rank }\n").is_empty());
    }

    // ---- 2. リテラル ----

    #[test]
    fn 宣言どおりのリテラルは通る() {
        assert!(
            errors(
                "struct User { id: UserId\nrank: Rank }\n\
                 fn main() { User { id = 1, rank = 2 } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 宣言と順序が違っても通る() {
        assert!(
            errors(
                "struct User { id: UserId\nrank: Rank }\n\
                 fn main() { User { rank = 2, id = 1 } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 未知のstructを報告する() {
        let e = only("fn main() { Missing { x = 1 } }\n");
        assert!(e.contains("struct `Missing` は宣言されていません"), "{e}");
        assert!(e.starts_with("main: "), "{e}");
    }

    #[test]
    fn structでない宣言をstructとして使うと報告する() {
        let e = only("trait Database { fn find(self) }\nfn main() { Database { x = 1 } }\n");
        assert!(e.contains("`Database` は struct ではありません"), "{e}");
    }

    #[test]
    fn 不足フィールドを報告する() {
        let e = only(
            "struct User { id: UserId\nrank: Rank }\n\
             fn main() { User { id = 1 } }\n",
        );
        assert!(e.contains("`rank`"), "{e}");
        assert!(!e.contains("`id`"), "{e}");
    }

    #[test]
    fn 余分なフィールドを報告する() {
        let e = only(
            "struct User { id: UserId }\n\
             fn main() { User { id = 1, nope = 2 } }\n",
        );
        assert!(e.contains("`nope`"), "{e}");
    }

    #[test]
    fn 重複したリテラルフィールドを報告する() {
        let errors = errors(
            "struct User { id: UserId }\n\
             fn main() { User { id = 1, id = 2 } }\n",
        );
        assert!(errors.iter().any(|e| e.contains("二度指定")), "{errors:?}");
    }

    #[test]
    fn implとtestの本体も検査する() {
        let errors = errors(
            "struct User { id: UserId }\n\
             struct Store {}\n\
             impl Store {\n\
             \x20 fn make(-> User) { User {} }\n\
             }\n\
             test \"t\" { User {} }\n",
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].starts_with("impl Store::make: "), "{errors:?}");
        assert!(errors[1].starts_with("test \"t\": "), "{errors:?}");
    }

    // ---- 3. bare struct と字句スコープ ----

    #[test]
    fn フィールド0個のstructは名前だけで値になれる() {
        assert!(errors("struct Gold {}\nfn main() { Gold }\n").is_empty());
    }

    #[test]
    fn フィールドを持つstructの裸の名前を報告する() {
        let e = only("struct User { id: UserId }\nfn main() { User }\n");
        assert!(e.contains("`User` はフィールドを 1 個持ちます"), "{e}");
    }

    #[test]
    fn 同名の引数はstruct名を隠す() {
        assert!(errors("struct User { id: UserId }\nfn f(User: Int) { User }\n").is_empty());
    }

    #[test]
    fn letはstruct名を隠す() {
        assert!(
            errors("struct User { id: UserId }\nfn f(n: Int) { let User = n\nUser }\n").is_empty()
        );
    }

    #[test]
    fn forの束縛はstruct名を隠す() {
        assert!(
            errors("struct User { id: UserId }\nfn f(xs: Users) { for User in xs { User } }\n")
                .is_empty()
        );
    }

    #[test]
    fn selfはローカルとして扱う() {
        // `self` は予約語なので struct 名とは衝突しえない。ここで固定するのは
        // 「メソッド本体のレシーバが裸の名前として診断されない」ことだけ
        assert!(
            errors(
                "struct User { id: UserId }\n\
                 struct Store { users: Users }\n\
                 impl Store { fn first(self -> User) { self.users } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 実行されない枝のletは後続のstruct名を隠さない() {
        let e = only(
            "struct User { id: UserId }\n\
             fn f(n: Int) {\n\
             \x20 if false { let User = n }\n\
             \x20 User\n\
             }\n",
        );
        assert!(e.contains("`User` はフィールドを 1 個持ちます"), "{e}");
    }

    #[test]
    fn 呼び出し先の名前は裸のstructとして読まない() {
        // `User(1)` は関数呼び出しであって struct 値の読みではない。
        // 未定義関数の検査は requirement の仕事
        assert!(errors("struct User { id: UserId }\nfn main() { User(1) }\n").is_empty());
    }

    // ---- 正典 ----

    #[test]
    fn 正典プログラムは診断を出さない() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let errors = errors(&src);
        assert!(errors.is_empty(), "{errors:?}");
    }
}
