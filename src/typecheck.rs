//! struct の形と、分かる範囲の型の検査(docs/overview.md「次の一歩」1)。
//!
//! モジュール解決済みの `Program` を受け取り、宣言と使用を突き合わせる。
//! ここを通ったプログラムでは
//!
//!   - 全ての struct リテラルが宣言済み struct を指し、宣言フィールドを
//!     過不足なく一度ずつ持つ
//!   - ローカルに隠されていない bare struct 値はフィールド0個
//!   - 非 optional の enum 型フィールドには、**両辺の型が分かる限り**
//!     別の enum の値が入らない
//!   - トップレベル関数の直接呼び出しは宣言どおりの引数の個数を持ち、
//!     **型の分かる**引数は宣言された引数型と一致する
//!   - 戻り値型を宣言した関数の、**型の分かる**明示 `return` と最後の式は
//!     その型と一致する
//!
//! 型の同一性は名前と後置 `?` の一致だけ(nominal)。演算、`nil`、`??`、配列の
//! 要素、フィールドの読み、メソッドと関連関数の呼び出しはまだ見ない
//! (design.md の Non-Goals)。型の分からない式には診断を出さない。
//!
//! 走査は `requirement::scan` と同じ字句スコープ規則を持つが、運ぶ状態が
//! 違う(あちらは提供集合、こちらはローカル名と分かっている型)ので別に書いている。

use crate::ast::{Expr, ExprKind, Head, Item, Program, Provision, Type};
use std::collections::{BTreeMap, BTreeSet};

/// 宣言の索引。正準名でそのまま引く。
struct Decls {
    /// struct 名 → フィールド名 → 宣言された型
    structs: BTreeMap<String, BTreeMap<String, KnownType>>,
    /// enum 名
    enums: BTreeSet<String>,
    /// variant の正準名 → 所属 enum の正準名
    variants: BTreeMap<String, String>,
    /// トップレベル関数の正準名 → 署名。本体を見る前に全部集めるので、
    /// 前方参照と再帰も引ける(design.md 決定2)
    fns: BTreeMap<String, FnSig>,
    /// struct ではない宣言名(trait / effect / fn / enum / variant)。
    /// 「未知」と「struct ではない」を言い分けるためだけに持つ
    others: BTreeSet<String>,
}

/// 分かっている型。同一性は名前と後置 `?` の一致(nominal, design.md 決定1)。
#[derive(Clone, PartialEq, Eq)]
struct KnownType {
    name: String,
    optional: bool,
}

impl std::fmt::Display for KnownType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.name, if self.optional { "?" } else { "" })
    }
}

/// トップレベル関数の署名のうち、呼び出し側の検査に要る分だけ。
struct FnSig {
    params: Vec<KnownType>,
    ret: Option<KnownType>,
}

/// ローカル束縛。**型が分かっているものだけ**型を持つ。
///
/// 名前の集合ではなく表にしているのは、`u.rank = Gold` のレシーバ型を
/// 引くため。入口は型注釈付き引数・`self`・struct リテラル・enum variant・
/// 直接呼び出しの宣言戻り値と、それらを直接束縛・参照する式に限る。
type Locals = BTreeMap<String, Option<KnownType>>;

/// 型注釈をそのまま型の事実にする。`None` は「推論できない」の意味で使うので、
/// 注釈のある所は optional でも `Some` になる(design.md 決定1)。
fn known(ty: &Type) -> KnownType {
    KnownType {
        name: ty.name.clone(),
        optional: ty.optional,
    }
}

/// 後置 `?` の付かない型。variant / struct リテラル / self に使う。
fn plain(name: &str) -> KnownType {
    KnownType {
        name: name.to_string(),
        optional: false,
    }
}

/// 組み込みのスカラー型。ユーザー宣言はこの名前を名乗れない(design.md 決定1)。
const BUILTINS: [&str; 4] = ["bool", "int", "str", "unit"];

/// 宣言名が組み込み型と衝突するなら、その綴り。正準名は修飾されているので
/// 末尾だけを見る(`main::int` も `int` の宣言)。
fn reserved(name: &str) -> Option<&str> {
    let short = name.rsplit_once("::").map_or(name, |(_, s)| s);
    BUILTINS.contains(&short).then_some(short)
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
                    .map(|p| (p.name.clone(), Some(known(&p.ty))))
                    .collect();
                let ret = sig.ret.as_ref().map(known);
                check_body(body, &decls, locals, &sig.name, ret.as_ref(), &mut out);
            }
            Item::Test { name, body, .. } => {
                let ctx = format!("test \"{name}\"");
                check_body(body, &decls, Locals::new(), &ctx, None, &mut out);
            }
            Item::Impl {
                type_name, methods, ..
            } => {
                for (sig, body) in methods {
                    let mut locals: Locals = sig
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), Some(known(&p.ty))))
                        .collect();
                    if sig.has_self {
                        locals.insert("self".to_string(), Some(plain(type_name)));
                    }
                    let ctx = format!("impl {type_name}::{}", sig.name);
                    let ret = sig.ret.as_ref().map(known);
                    check_body(body, &decls, locals, &ctx, ret.as_ref(), &mut out);
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
    let mut fns = BTreeMap::new();
    let mut others = BTreeSet::new();

    for item in &program.items {
        // 組み込み型はどの宣言種でも名乗れない。宣言名を出す口は module.rs の
        // 1本しかないので、そこを借りて全種を1箇所で見る
        for name in crate::module::item_names(item) {
            if let Some(short) = reserved(name) {
                out.push(format!("`{short}` は組み込み型の名前なので宣言できません"));
            }
        }
        match item {
            Item::Struct { name, fields, .. } => {
                let mut declared: BTreeMap<String, KnownType> = BTreeMap::new();
                let mut duplicates = BTreeSet::new();
                for (field, ty) in fields {
                    let previous = declared.insert(field.clone(), known(ty));
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
                fns.insert(
                    sig.name.clone(),
                    FnSig {
                        params: sig.params.iter().map(|p| known(&p.ty)).collect(),
                        ret: sig.ret.as_ref().map(known),
                    },
                );
            }
            Item::Impl { .. } | Item::Test { .. } => {}
        }
    }

    Decls {
        structs,
        enums,
        variants,
        fns,
        others,
    }
}

fn check_body(
    body: &[Expr],
    decls: &Decls,
    mut locals: Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Vec<String>,
) {
    check_exprs(body, decls, &mut locals, ctx, ret, out);
    // ブロックは値ベースなので最後の式も戻り値。明示 `return` は走査側が見る。
    // 最後の式を見る場所をここ一箇所にして、入れ子で二重に出るのを防ぐ
    if let Some(last) = body.last() {
        check_return(last, decls, &locals, ctx, ret, out);
    }
}

fn check_exprs(
    body: &[Expr],
    decls: &Decls,
    locals: &mut Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Vec<String>,
) {
    for e in body {
        check_expr(e, decls, locals, ctx, ret, out);
    }
}

/// `requirement::scan` と同じく、ブロックから戻れば内側の束縛は消える。
/// 枝へ入るときだけ `locals` を複製する。
fn check_expr(
    e: &Expr,
    decls: &Decls,
    locals: &mut Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Vec<String>,
) {
    match &e.kind {
        ExprKind::Ident(name) => check_bare(name, decls, locals, ctx, out),

        ExprKind::StructLit { name, fields } => {
            check_literal(name, fields, decls, ctx, out);
            for (field, v) in fields {
                check_expr(v, decls, locals, ctx, ret, out);
                check_enum_field(name, field, v, decls, locals, ctx, out);
            }
        }

        ExprKind::Let { name, value } => {
            check_expr(value, decls, locals, ctx, ret, out);
            let ty = infer(value, decls, locals);
            locals.insert(name.clone(), ty);
        }

        ExprKind::Call(callee, args) => {
            // 呼び出し先の名前は値として読まれない。評価器も `Ident` / `Path` の
            // callee は関数表・型表から引くだけで、値へ落とさない
            if !matches!(callee.kind, ExprKind::Ident(_) | ExprKind::Path(_)) {
                check_expr(callee, decls, locals, ctx, ret, out);
            }
            for a in args {
                check_expr(a, decls, locals, ctx, ret, out);
            }
            // メソッド (`Field`) と関連関数 (`Path`) は候補の絞り込みが要るので
            // 今回は見ない(design.md 決定3)
            if let ExprKind::Ident(name) = &callee.kind
                && let Some(sig) = decls.fns.get(name)
            {
                check_call(name, sig, args, decls, locals, ctx, out);
            }
        }

        ExprKind::Field(recv, _) => check_expr(recv, decls, locals, ctx, ret, out),

        ExprKind::Assign { target, value } => {
            check_expr(value, decls, locals, ctx, ret, out);
            // 代入先の裸の名前は書き込み先であって値の読みではない
            if let ExprKind::Field(recv, field) = &target.kind {
                check_expr(recv, decls, locals, ctx, ret, out);
                // optional の中身を取り出す規則はまだ無いので、レシーバは
                // 非 optional と分かるときだけ見る
                if let Some(ty) = infer(recv, decls, locals)
                    && !ty.optional
                {
                    check_enum_field(&ty.name, field, value, decls, locals, ctx, out);
                }
            }
        }

        ExprKind::Head { head, body, orelse } => {
            match head {
                Head::Ambient(binders) => {
                    // 提供値は外側で評価される
                    for b in binders {
                        if let Provision::Value { value, .. } = b {
                            check_expr(value, decls, locals, ctx, ret, out);
                        }
                    }
                    let mut inner = locals.clone();
                    for b in binders {
                        inner.insert(b.slot().to_string(), None);
                    }
                    check_expr(body, decls, &mut inner, ctx, ret, out);
                }
                Head::If(c) | Head::Elif(c) | Head::While(c) => {
                    check_expr(c, decls, locals, ctx, ret, out);
                    check_expr(body, decls, &mut locals.clone(), ctx, ret, out);
                }
                Head::For { var, iter } => {
                    check_expr(iter, decls, locals, ctx, ret, out);
                    let mut inner = locals.clone();
                    // 配列の要素型は今回の保証外
                    inner.insert(var.clone(), None);
                    check_expr(body, decls, &mut inner, ctx, ret, out);
                }
                Head::Else => check_expr(body, decls, &mut locals.clone(), ctx, ret, out),
            }
            if let Some(o) = orelse {
                check_expr(o, decls, &mut locals.clone(), ctx, ret, out);
            }
        }

        ExprKind::Array(items) => {
            for i in items {
                check_expr(i, decls, locals, ctx, ret, out);
            }
        }
        ExprKind::Block(body) => check_exprs(body, decls, locals, ctx, ret, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            check_expr(lhs, decls, locals, ctx, ret, out);
            check_expr(rhs, decls, locals, ctx, ret, out);
        }
        ExprKind::Return(Some(inner)) => {
            check_expr(inner, decls, locals, ctx, ret, out);
            check_return(inner, decls, locals, ctx, ret, out);
        }
        ExprKind::Unary(_, inner) | ExprKind::Assert(inner) => {
            check_expr(inner, decls, locals, ctx, ret, out)
        }

        ExprKind::Int(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Nil
        | ExprKind::Path(_)
        | ExprKind::Return(None) => {}
    }
}

/// 直接呼び出しの検査。引数の個数は常に、型は**分かるときだけ**見る。
fn check_call(
    name: &str,
    sig: &FnSig,
    args: &[Expr],
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Vec<String>,
) {
    if args.len() != sig.params.len() {
        out.push(format!(
            "{ctx}: `{name}` は引数を {} 個取りますが、{} 個渡しています",
            sig.params.len(),
            args.len()
        ));
        return;
    }
    for (i, (arg, expected)) in args.iter().zip(&sig.params).enumerate() {
        let Some(actual) = infer(arg, decls, locals) else {
            continue;
        };
        if actual != *expected {
            out.push(format!(
                "{ctx}: `{name}` の第 {} 引数は `{expected}` ですが、`{actual}` を渡しています",
                i + 1
            ));
        }
    }
}

/// 宣言された戻り値型と、**型の分かる**戻り値だけを照合する。
/// 全ての経路が値を返すかは見ない(design.md 決定4)。
fn check_return(
    value: &Expr,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Vec<String>,
) {
    let Some(expected) = ret else {
        return;
    };
    let Some(actual) = infer(value, decls, locals) else {
        return;
    };
    if actual != *expected {
        out.push(format!(
            "{ctx}: 戻り値は `{expected}` ですが、`{actual}` を返しています"
        ));
    }
}

/// 式の型が分かるならそれ。分からないなら `None`。
///
/// 入口を増やすとその分だけ検査が効くが、保証範囲も広がる。今回の入口は
/// ローカル・variant・struct リテラル・**宣言戻り値のある直接呼び出し**と、
/// それらを直接束縛・参照する式だけ。
fn infer(e: &Expr, decls: &Decls, locals: &Locals) -> Option<KnownType> {
    match &e.kind {
        ExprKind::Int(_) => Some(plain("int")),
        ExprKind::Bool(_) => Some(plain("bool")),
        ExprKind::Str(_) => Some(plain("str")),
        ExprKind::Ident(name) => match locals.get(name) {
            Some(known) => known.clone(),
            None => decls.variants.get(name).map(|e| plain(e)),
        },
        ExprKind::StructLit { name, .. } => Some(plain(name)),
        ExprKind::Call(callee, _) => match &callee.kind {
            ExprKind::Ident(name) => decls.fns.get(name)?.ret.clone(),
            _ => None,
        },
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
    if actual.optional || !decls.enums.contains(&actual.name) || actual.name == declared.name {
        return;
    }
    out.push(format!(
        "{ctx}: `{type_name}` のフィールド `{field}` は enum `{}` ですが、enum `{}` の値を与えています",
        declared.name, actual.name
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
        assert!(errors("struct User { id: int\nrank: Rank }\n").is_empty());
    }

    // ---- 2. リテラル ----

    #[test]
    fn 宣言どおりのリテラルは通る() {
        assert!(
            errors(
                "struct User { id: int\nrank: Rank }\n\
                 fn main() { User { id = 1, rank = 2 } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 宣言と順序が違っても通る() {
        assert!(
            errors(
                "struct User { id: int\nrank: Rank }\n\
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
            "struct User { id: int\nrank: Rank }\n\
             fn main() { User { id = 1 } }\n",
        );
        assert!(e.contains("`rank`"), "{e}");
        assert!(!e.contains("`id`"), "{e}");
    }

    #[test]
    fn 余分なフィールドを報告する() {
        let e = only(
            "struct User { id: int }\n\
             fn main() { User { id = 1, nope = 2 } }\n",
        );
        assert!(e.contains("`nope`"), "{e}");
    }

    #[test]
    fn 重複したリテラルフィールドを報告する() {
        let errors = errors(
            "struct User { id: int }\n\
             fn main() { User { id = 1, id = 2 } }\n",
        );
        assert!(errors.iter().any(|e| e.contains("二度指定")), "{errors:?}");
    }

    #[test]
    fn implとtestの本体も検査する() {
        let errors = errors(
            "struct User { id: int }\n\
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
        let e = only("struct User { id: int }\nfn main() { User }\n");
        assert!(e.contains("`User` はフィールドを 1 個持ちます"), "{e}");
    }

    #[test]
    fn 同名の引数はstruct名を隠す() {
        assert!(errors("struct User { id: int }\nfn f(User: int) { User }\n").is_empty());
    }

    #[test]
    fn letはstruct名を隠す() {
        assert!(
            errors("struct User { id: int }\nfn f(n: int) { let User = n\nUser }\n").is_empty()
        );
    }

    #[test]
    fn forの束縛はstruct名を隠す() {
        assert!(
            errors("struct User { id: int }\nfn f(xs: Users) { for User in xs { User } }\n")
                .is_empty()
        );
    }

    #[test]
    fn selfはローカルとして扱う() {
        // `self` は予約語なので struct 名とは衝突しえない。ここで固定するのは
        // 「メソッド本体のレシーバが裸の名前として診断されない」ことだけ
        assert!(
            errors(
                "struct User { id: int }\n\
                 struct Store { users: Users }\n\
                 impl Store { fn first(self -> User) { self.users } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 実行されない枝のletは後続のstruct名を隠さない() {
        let e = only(
            "struct User { id: int }\n\
             fn f(n: int) {\n\
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
        assert!(errors("struct User { id: int }\nfn main() { User(1) }\n").is_empty());
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
        // 引数の型注釈も struct リテラルも通っていない値
        assert!(
            errors(&format!(
                "{RANKS}fn main(n: int) {{ User {{ rank = n }} }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の分からないレシーバへの代入は診断しない() {
        assert!(errors(&format!("{RANKS}fn f(u: User?) {{ u.rank = Low }}\n")).is_empty());
    }

    #[test]
    fn メソッド呼び出しの結果は診断しない() {
        // レシーバ付きの呼び出しは候補の絞り込みが要るので型が分からない
        assert!(
            errors(&format!(
                "{RANKS}struct Store {{ id: int }}\n\
                 impl Store {{ fn get(self -> User) {{ User {{ rank = Gold }} }} }}\n\
                 fn main(s: Store) {{ s.get().rank = Low }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 関連関数呼び出しの結果は診断しない() {
        assert!(
            errors(&format!(
                "{RANKS}struct Store {{}}\n\
                 impl Store {{ fn make(-> User) {{ User {{ rank = Gold }} }} }}\n\
                 fn main() {{ Store::make().rank = Low }}\n"
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
                 struct User { id: int }\n\
                 fn main() { User { id = Low } }\n",
            )
            .is_empty()
        );
    }

    // ---- 6. 直接呼び出しの引数 ----

    #[test]
    fn 宣言どおりの引数の個数は診断を出さない() {
        assert!(errors("fn f(a: int, b: int) { a }\nfn main(n: int) { f(n, n) }\n").is_empty());
    }

    #[test]
    fn 引数が足りない呼び出しを報告する() {
        let e = only("fn f(a: int, b: int) { a }\nfn main(n: int) { f(n) }\n");
        assert!(e.starts_with("main: "), "{e}");
        assert!(e.contains("`f`"), "{e}");
        assert!(e.contains("2 個取ります"), "{e}");
        assert!(e.contains("1 個渡しています"), "{e}");
    }

    #[test]
    fn 引数が多すぎる呼び出しを報告する() {
        let e = only("fn f(a: int) { a }\nfn main(n: int) { f(n, n) }\n");
        assert!(e.contains("1 個取ります"), "{e}");
        assert!(e.contains("2 個渡しています"), "{e}");
    }

    #[test]
    fn 前方参照と再帰の呼び出しも検査する() {
        let errors = errors(
            "fn main(n: int) { later(n, n) }\n\
             fn later(a: int) { later(a, a) }\n",
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].starts_with("main: "), "{errors:?}");
        assert!(errors[1].starts_with("later: "), "{errors:?}");
    }

    #[test]
    fn 型の合う引数は診断を出さない() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank) {{ r }}\nfn main() {{ f(Gold) }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の違う引数を位置付きで報告する() {
        let e = only(&format!(
            "{RANKS}fn f(a: Rank, b: Rank) {{ a }}\nfn main() {{ f(Gold, High) }}\n"
        ));
        assert!(e.contains("`f` の第 2 引数"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn optionalの有無が違う引数を報告する() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank?) {{ r }}\nfn main(r: Rank) {{ f(r) }}\n"
        ));
        assert!(e.contains("`Rank?`"), "{e}");
    }

    #[test]
    fn 型の分からない引数は診断しない() {
        // 演算・配列・nil・メソッド結果はまだ推論の外
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank) {{ r }}\nfn main(n: int) {{ f(n + 1) }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn メソッドと関連関数の呼び出しは引数を検査しない() {
        assert!(
            errors(
                "struct Store {}\n\
                 impl Store {\n\
                 \x20 fn make(a: int -> Store) { Store {} }\n\
                 \x20 fn take(self, a: int) { a }\n\
                 }\n\
                 fn main(s: Store) {\n\
                 \x20 s.take()\n\
                 \x20 Store::make()\n\
                 }\n",
            )
            .is_empty()
        );
    }

    // ---- 7. 直接呼び出しの戻り値型 ----

    #[test]
    fn 呼び出し結果を束縛したローカルは型が分かる() {
        let e = only(&format!(
            "{RANKS}fn pick(-> Grade) {{ Low }}\n\
             fn main() {{\n let g = pick()\n User {{ rank = g }}\n}}\n"
        ));
        assert!(e.contains("enum `Grade`"), "{e}");
    }

    #[test]
    fn 呼び出し結果はそのままフィールド検査に届く() {
        let e = only(&format!(
            "{RANKS}fn pick(-> Grade) {{ Low }}\nfn main() {{ User {{ rank = pick() }} }}\n"
        ));
        assert!(e.contains("enum `Rank`"), "{e}");
        assert!(e.contains("enum `Grade`"), "{e}");
    }

    #[test]
    fn 呼び出し結果への代入もレシーバ型が分かる() {
        let e = only(&format!(
            "{RANKS}fn get(-> User) {{ User {{ rank = Gold }} }}\n\
             fn main() {{ get().rank = Low }}\n"
        ));
        assert!(e.contains("enum `Grade`"), "{e}");
    }

    #[test]
    fn 戻り値型の無い関数の結果は分からないまま() {
        assert!(
            errors(&format!(
                "{RANKS}fn pick() {{ Low }}\nfn main() {{ User {{ rank = pick() }} }}\n"
            ))
            .is_empty()
        );
    }

    // ---- 8. 宣言された戻り値の検査 ----

    #[test]
    fn 型の合う最後の式は診断を出さない() {
        assert!(errors(&format!("{RANKS}fn pick(-> Grade) {{ Low }}\n")).is_empty());
    }

    #[test]
    fn 型の違う最後の式を報告する() {
        let e = only(&format!("{RANKS}fn pick(-> Grade) {{ Gold }}\n"));
        assert!(e.starts_with("pick: "), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
    }

    #[test]
    fn 入れ子の明示returnの型違いを報告する() {
        let e = only(&format!(
            "{RANKS}fn pick(b: bool -> Grade) {{\n\
             \x20 if b {{ return Gold }}\n\
             \x20 Low\n\
             }}\n"
        ));
        assert!(e.contains("`Grade`"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
    }

    #[test]
    fn implメソッドの戻り値も検査する() {
        let e = only(&format!(
            "{RANKS}impl User {{ fn grade(self -> Grade) {{ Gold }} }}\n"
        ));
        assert!(e.starts_with("impl User::grade: "), "{e}");
    }

    #[test]
    fn 最後の式が明示returnでも二重に報告しない() {
        let e = only(&format!("{RANKS}fn pick(-> Grade) {{ return Gold }}\n"));
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 戻り値型を宣言しない関数は診断しない() {
        assert!(errors(&format!("{RANKS}fn pick() {{ Gold }}\n")).is_empty());
    }

    #[test]
    fn 値の無いreturnは診断しない() {
        assert!(
            errors(&format!(
                "{RANKS}fn pick(b: bool -> Grade) {{\n if b {{ return }}\n Low\n}}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の分からない戻り値は診断しない() {
        assert!(errors(&format!("{RANKS}fn pick(n: int -> Grade) {{ n + 1 }}\n")).is_empty());
    }

    // ---- 9. 組み込みのスカラー型 ----

    #[test]
    fn リテラルは組み込み型を持つ() {
        assert!(
            errors(
                "fn f(a: int, b: bool, c: str) { a }\n\
                 fn main() { f(1, true, \"x\") }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 型の違うリテラル引数を報告する() {
        let e = only("fn f(a: int) { a }\nfn main() { f(\"x\") }\n");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`str`"), "{e}");
    }

    #[test]
    fn 型の違うリテラルの戻り値を報告する() {
        let e = only("fn f(-> int) { true }\n");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`bool`"), "{e}");
    }

    #[test]
    fn 組み込み型名を名乗る宣言を報告する() {
        // 宣言種ごとに1本ずつ。どれか一つでも抜けるとリテラルの意味が変わる
        for src in [
            "struct int {}\n",
            "enum bool { Yes No }\n",
            "enum E { str Other }\n",
            "trait unit { fn f(self) }\n",
            "effect int: Clock\n",
            "fn bool() { 1 }\n",
        ] {
            let e = only(src);
            assert!(e.contains("組み込み型の名前"), "{src}: {e}");
        }
    }

    // ---- 正典 ----

    #[test]
    fn 正典プログラムは診断を出さない() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let errors = errors(&src);
        assert!(errors.is_empty(), "{errors:?}");
    }
}
