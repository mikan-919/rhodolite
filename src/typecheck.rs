//! struct の形と、分かる範囲の enum 型の検査(docs/overview.md「次の一歩」1)。
//!
//! モジュール解決済みの `Program` を受け取り、宣言と生成のフィールド集合を
//! 突き合わせる。ここを通ったプログラムでは
//!
//!   - 全ての struct リテラルが宣言済み struct を指し、宣言フィールドを
//!     過不足なく一度ずつ持つ
//!   - ローカルに隠されていない bare struct 値はフィールド0個
//!   - 非 optional の enum 型フィールドには、**両辺の型が分かる限り**
//!     別の enum の値が入らない
//!
//! フィールド**値**の型は enum の1種類だけ見る。引数、戻り値、演算、optional、
//! 配列、呼び出し結果はまだ見ない(design.md の Non-Goals)。
//!
//! 走査は `requirement::scan` と同じ字句スコープ規則を持つが、運ぶ状態が
//! 違う(あちらは提供集合、こちらはローカル名と分かっている型)ので別に書いている。

use crate::ast::{Expr, ExprKind, Head, Item, Program, Provision, Type};
use std::collections::{BTreeMap, BTreeSet};

/// 宣言の索引。正準名でそのまま引く。
struct Decls {
    /// struct 名 → フィールド名 → 宣言された型
    structs: BTreeMap<String, BTreeMap<String, FieldType>>,
    /// enum 名
    enums: BTreeSet<String>,
    /// variant の正準名 → 所属 enum の正準名
    variants: BTreeMap<String, String>,
    /// struct ではない宣言名(trait / effect / fn / enum / variant)。
    /// 「未知」と「struct ではない」を言い分けるためだけに持つ
    others: BTreeSet<String>,
}

/// 宣言されたフィールドの型。後置 `?` は「今回は見ない」の印として残す。
struct FieldType {
    name: String,
    optional: bool,
}

/// ローカル束縛。**型が分かっているものだけ**型名を持つ。
///
/// 名前の集合ではなく表にしているのは、`u.rank = Gold` のレシーバ型を
/// 引くため。入口は型注釈付き引数・`self`・struct リテラル・enum variant と、
/// それらを直接束縛・参照する式に限る(design.md 決定4)。
type Locals = BTreeMap<String, Option<String>>;

/// 型注釈から「分かっている型」を取り出す。optional は今回の保証外なので落とす。
fn known(ty: &Type) -> Option<String> {
    if ty.optional {
        None
    } else {
        Some(ty.name.clone())
    }
}

/// 診断を全件返す。空なら struct の形と分かる enum 型は正しい。
pub fn check(program: &Program) -> Vec<String> {
    let mut out = Vec::new();
    let decls = collect(program, &mut out);

    for item in &program.items {
        match item {
            Item::Fn { sig, body, .. } => {
                let locals = sig
                    .params
                    .iter()
                    .map(|p| (p.name.clone(), known(&p.ty)))
                    .collect();
                check_body(body, &decls, locals, &sig.name, &mut out);
            }
            Item::Test { name, body, .. } => {
                let ctx = format!("test \"{name}\"");
                check_body(body, &decls, Locals::new(), &ctx, &mut out);
            }
            Item::Impl {
                type_name, methods, ..
            } => {
                for (sig, body) in methods {
                    let mut locals: Locals = sig
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), known(&p.ty)))
                        .collect();
                    if sig.has_self {
                        locals.insert("self".to_string(), Some(type_name.clone()));
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
    let mut enums = BTreeSet::new();
    let mut variants = BTreeMap::new();
    let mut others = BTreeSet::new();

    for item in &program.items {
        match item {
            Item::Struct { name, fields, .. } => {
                let mut declared: BTreeMap<String, FieldType> = BTreeMap::new();
                let mut duplicates = BTreeSet::new();
                for (field, ty) in fields {
                    let previous = declared.insert(
                        field.clone(),
                        FieldType {
                            name: ty.name.clone(),
                            optional: ty.optional,
                        },
                    );
                    if previous.is_some() {
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
            // variant の重複は宣言名前空間の衝突として module.rs が報告する。
            // ここは所属の索引を作るだけ
            Item::Enum {
                name, variants: vs, ..
            } => {
                enums.insert(name.clone());
                others.insert(name.clone());
                for variant in vs {
                    variants.insert(variant.clone(), name.clone());
                    others.insert(variant.clone());
                }
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

    Decls {
        structs,
        enums,
        variants,
        others,
    }
}

fn check_body(body: &[Expr], decls: &Decls, mut locals: Locals, ctx: &str, out: &mut Vec<String>) {
    check_exprs(body, decls, &mut locals, ctx, out);
}

fn check_exprs(
    body: &[Expr],
    decls: &Decls,
    locals: &mut Locals,
    ctx: &str,
    out: &mut Vec<String>,
) {
    for e in body {
        check_expr(e, decls, locals, ctx, out);
    }
}

/// `requirement::scan` と同じく、ブロックから戻れば内側の束縛は消える。
/// 枝へ入るときだけ `locals` を複製する。
fn check_expr(e: &Expr, decls: &Decls, locals: &mut Locals, ctx: &str, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::Ident(name) => check_bare(name, decls, locals, ctx, out),

        ExprKind::StructLit { name, fields } => {
            check_literal(name, fields, decls, ctx, out);
            for (field, v) in fields {
                check_expr(v, decls, locals, ctx, out);
                check_enum_field(name, field, v, decls, locals, ctx, out);
            }
        }

        ExprKind::Let { name, value } => {
            check_expr(value, decls, locals, ctx, out);
            let ty = infer(value, decls, locals);
            locals.insert(name.clone(), ty);
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
            if let ExprKind::Field(recv, field) = &target.kind {
                check_expr(recv, decls, locals, ctx, out);
                if let Some(type_name) = infer(recv, decls, locals) {
                    check_enum_field(&type_name, field, value, decls, locals, ctx, out);
                }
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
                        inner.insert(b.slot().to_string(), None);
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
                    // 配列の要素型は今回の保証外
                    inner.insert(var.clone(), None);
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

/// 式の型が分かるならその名前。分からないなら `None`。
///
/// 入口を増やすとその分だけ検査が効くが、保証範囲も広がる。今回は
/// design.md 決定4 の4つ(ローカル・variant・struct リテラル・それらの参照)だけ。
fn infer(e: &Expr, decls: &Decls, locals: &Locals) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(name) => match locals.get(name) {
            Some(known) => known.clone(),
            None => decls.variants.get(name).cloned(),
        },
        ExprKind::StructLit { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// 「宛先が非 optional の enum 型」かつ「値の型も enum と分かる」ときだけ照合する。
fn check_enum_field(
    type_name: &str,
    field: &str,
    value: &Expr,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Vec<String>,
) {
    let Some(declared) = decls.structs.get(type_name).and_then(|f| f.get(field)) else {
        return;
    };
    if declared.optional || !decls.enums.contains(&declared.name) {
        return;
    }
    let Some(actual) = infer(value, decls, locals) else {
        return;
    };
    if !decls.enums.contains(&actual) || actual == declared.name {
        return;
    }
    out.push(format!(
        "{ctx}: `{type_name}` のフィールド `{field}` は enum `{}` ですが、enum `{actual}` の値を与えています",
        declared.name
    ));
}

/// 裸の名前が値になれるのは、隠されていないフィールド0個の struct か
/// enum variant のときだけ。
fn check_bare(name: &str, decls: &Decls, locals: &Locals, ctx: &str, out: &mut Vec<String>) {
    if locals.contains_key(name) {
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
        .keys()
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
        .filter(|f| !declared.contains_key(*f))
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
    fn enumをstructとして生成すると報告する() {
        let e = only("enum Rank { Bronze Gold }\nfn main() { Rank { x = 1 } }\n");
        assert!(e.contains("`Rank` は struct ではありません"), "{e}");
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

    // ---- 4. 分かる enum 型 ----

    /// 検査環境の境界。何が「型の分かる入口」かをここで固定する
    const RANKS: &str = "enum Rank { Bronze Gold }\n\
                         enum Grade { Low High }\n\
                         struct User { rank: Rank }\n";

    #[test]
    fn 同じenumのvariantはstruct生成で受理される() {
        assert!(errors(&format!("{RANKS}fn main() {{ User {{ rank = Gold }} }}\n")).is_empty());
    }

    #[test]
    fn 別のenumのvariantをstruct生成で報告する() {
        let e = only(&format!("{RANKS}fn main() {{ User {{ rank = High }} }}\n"));
        assert!(e.contains("enum `Rank`"), "{e}");
        assert!(e.contains("enum `Grade`"), "{e}");
        assert!(e.contains("`rank`"), "{e}");
    }

    #[test]
    fn variantを束縛したローカルも型が分かる() {
        let e = only(&format!(
            "{RANKS}fn main() {{\n let g = High\n User {{ rank = g }}\n}}\n"
        ));
        assert!(e.contains("enum `Grade`"), "{e}");
    }

    #[test]
    fn 型の分かるレシーバへの代入で同じenumは受理される() {
        assert!(
            errors(&format!(
                "{RANKS}fn stamp(u: User) {{\n u.rank = Bronze\n}}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の分かるレシーバへの代入で別のenumを報告する() {
        let e = only(&format!("{RANKS}fn stamp(u: User) {{\n u.rank = Low\n}}\n"));
        assert!(e.contains("enum `Rank`"), "{e}");
        assert!(e.contains("enum `Grade`"), "{e}");
    }

    #[test]
    fn selfレシーバへの代入も検査する() {
        let e = only(&format!(
            "{RANKS}impl User {{\n fn demote(self) {{ self.rank = Low }}\n}}\n"
        ));
        assert!(e.contains("impl User::demote"), "{e}");
        assert!(e.contains("enum `Grade`"), "{e}");
    }

    #[test]
    fn structリテラルを束縛したローカルはレシーバ型が分かる() {
        let e = only(&format!(
            "{RANKS}fn main() {{\n let u = User {{ rank = Gold }}\n u.rank = Low\n}}\n"
        ));
        assert!(e.contains("enum `Grade`"), "{e}");
    }

    // ---- 5. 今回の保証外 ----

    #[test]
    fn 型の分からない値は診断しない() {
        // 引数の型注釈も struct リテラルも通っていない値。呼び出し結果も同じ
        assert!(
            errors(&format!(
                "{RANKS}fn pick(-> Grade) {{ Low }}\n\
                 fn main(n: Int) {{\n\
                 \x20 User {{ rank = n }}\n\
                 \x20 User {{ rank = pick() }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の分からないレシーバへの代入は診断しない() {
        assert!(errors(&format!("{RANKS}fn f(u: User?) {{ u.rank = Low }}\n")).is_empty());
    }

    #[test]
    fn 呼び出し結果への代入は診断しない() {
        assert!(
            errors(&format!(
                "{RANKS}fn get(-> User) {{ User {{ rank = Gold }} }}\n\
                 fn main() {{ get().rank = Low }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn optionalなenumフィールドは診断しない() {
        assert!(
            errors(
                "enum Rank { Bronze Gold }\n\
                 enum Grade { Low High }\n\
                 struct Draft { rank: Rank? }\n\
                 fn main() { Draft { rank = Low } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 配列の要素は診断しない() {
        assert!(
            errors(&format!(
                "{RANKS}fn main() {{\n for r in [Low] {{ User {{ rank = r }} }}\n}}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn enum型でないフィールドは診断しない() {
        assert!(
            errors(
                "enum Grade { Low High }\n\
                 struct User { id: UserId }\n\
                 fn main() { User { id = Low } }\n",
            )
            .is_empty()
        );
    }

    // ---- 正典 ----

    #[test]
    fn 正典プログラムは診断を出さない() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let errors = errors(&src);
        assert!(errors.is_empty(), "{errors:?}");
    }
}
