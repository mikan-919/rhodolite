//! struct の形と、分かる範囲の型の検査(docs/overview.md「次の一歩」)。
//!
//! モジュール解決済みの `Program` を受け取り、宣言と使用を突き合わせる。
//! ここを通ったプログラムでは
//!
//!   - 全ての struct リテラルが宣言済み struct を指し、宣言フィールドを
//!     過不足なく一度ずつ持つ
//!   - ローカルに隠されていない bare struct 値はフィールド0個
//!   - `int` / `bool` / `str` / `unit` を名乗る宣言が無い
//!   - 型の分かる非 optional のレシーバからのフィールドの読みは、宣言済み
//!     struct の宣言フィールドを指す
//!   - 算術と単項 `-` の**型の分かる**被演算子は `int`、`==` の**両辺の型が
//!     分かる**比較は同じ型、`if` / `elif` / `while` の条件と `assert` の
//!     **型の分かる**対象は `bool`
//!   - struct リテラルのフィールド値・フィールドへの代入・ローカルへの再代入は、
//!     **両辺の型が分かる限り**宛先の型と適合する
//!   - trait `impl` は宣言済み trait と struct を指し、契約のメソッドを
//!     過不足なく一度ずつ、宣言どおりのレシーバ・引数型・戻り値型で持つ
//!   - 直接呼び出し・メソッド・関連関数は、レシーバかスロットの分かる限り
//!     一意の宣言へ解決され、`.` と `::` は宣言された `self` の有無と一致する
//!   - 解決した呼び出しは宣言どおりの引数の個数を持ち、**型の分かる**引数は
//!     宣言された引数型と適合する
//!   - 戻り値型を宣言した関数の、**型の分かる**明示 `return` と最後の式は
//!     その型と適合する
//!   - 配列リテラルの要素は、期待要素型があればそれと、無ければ互いに適合する
//!   - `for` の反復対象は非 optional な配列で、ループ変数は要素型を持つ
//!   - `with` の提供は、**型の分かる限り**スロットの trait を実装した具体型
//!
//! 型の同一性は形と後置 `?` の一致だけ(nominal)。配列は要素型まで含めて
//! 一致しないと同じ型ではない。期待型のある宛先では
//! `T` を同名の `T?` へ注入できるが、式の推論型と等価比較は変えない。
//! `nil` は期待される `T?` の文脈でだけ適合し、`T? ?? T` は `T` を返す。
//! `S?.?field` は宣言 field の型に optional を付けて返す。
//!
//! 呼び出しの解決は評価器と同じ順序で、スロット経由なら宣言 trait の契約だけ、
//! 具体型なら inherent と trait 実装をまとめて名前で絞り一意を要求する。
//! レシーバの型が分からない呼び出しは保留する。`with slot<T>` は型名が
//! そのまま分かるので常に、`with slot(v)` は `v` の型が分かるときだけ契約と
//! 突き合わせ、分からない提供は eval 側の同じ判定が実行時に止める。
//! 型の分からない式には診断を出さず、後続の change が一つずつ潰していく。
//!
//! 走査は `requirement::scan` と同じ字句スコープ規則を持つが、運ぶ状態が
//! 違う(あちらは提供集合、こちらはローカル名と分かっている型)ので別に書いている。

use crate::ast::{
    BinOp, Expr, ExprKind, Head, Item, MatchArm, Program, Provision, Sig, Type, TypeKind, UnOp,
};
use crate::diag::Diag;
use crate::lex::Span;
use crate::module::short_name;
use crate::requirement::{Slots, collect_slots};
use std::collections::{BTreeMap, BTreeSet};

/// 宣言の索引。正準名でそのまま引く。
struct Decls {
    /// struct 名 → フィールド名 → 宣言された型
    structs: BTreeMap<String, BTreeMap<String, KnownType>>,
    /// variant の正準名 → 所属 enum の正準名
    variants: BTreeMap<String, String>,
    /// enum の正準名 → 宣言順の variant の正準名。網羅性の検査と、限定参照
    /// `Rank::Gold` の所属の照合に使う
    enums: BTreeMap<String, Vec<String>>,
    /// トップレベル関数の正準名 → 署名。本体を見る前に全部集めるので、
    /// 前方参照と再帰も引ける(design.md 決定2)
    fns: BTreeMap<String, FnSig>,
    /// trait 名 → メンバー名 → 契約署名。スロット経由の呼び出しは、実行時の
    /// 具体型が意図的に変わるので**契約だけ**を見る(design.md 決定4)
    traits: BTreeMap<String, BTreeMap<String, FnSig>>,
    /// 具体型名 → その型の全 `impl` メンバー(メンバー名, 署名)。trait 実装も
    /// inherent も混ぜて入れる。呼び出し地点で trait が分かるとは限らないので、
    /// 絞り込みは名前だけで行い、複数残れば曖昧とする(eval.rs `find_method`)
    impls: BTreeMap<String, Vec<(String, FnSig)>>,
    /// `impl Trait for Type` の (具体型名, trait 名)。`with` の提供が
    /// スロットの契約を満たすか見るためだけに持つ
    trait_impls: BTreeSet<(String, String)>,
    /// スロット名 → trait 名。`requirement` の表をそのまま借りる
    slots: Slots,
    /// struct ではない宣言名(trait / effect / fn / enum / variant)。
    /// 「未知」と「struct ではない」を言い分けるためだけに持つ
    others: BTreeSet<String>,
}

/// 分かっている型。同一性は形と後置 `?` の一致(nominal, design.md 決定1)。
/// 配列は要素型まで含めて一致しないと同じ型ではない(要素型は不変)。
#[derive(Clone, PartialEq, Eq)]
struct KnownType {
    kind: KnownKind,
    optional: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum KnownKind {
    Named(String),
    Array(Box<KnownType>),
}

impl KnownType {
    /// 名前の葉。配列なら `None`。struct 表を引く前に必ず通る
    fn name(&self) -> Option<&str> {
        match &self.kind {
            KnownKind::Named(name) => Some(name),
            KnownKind::Array(_) => None,
        }
    }

    /// 配列の要素型。後置 `?` の有無に関わらず取れる
    fn element(&self) -> Option<&KnownType> {
        match &self.kind {
            KnownKind::Array(element) => Some(element),
            KnownKind::Named(_) => None,
        }
    }
}

impl std::fmt::Display for KnownType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            KnownKind::Named(name) => write!(f, "{name}")?,
            KnownKind::Array(element) => write!(f, "[{element}]")?,
        }
        if self.optional {
            write!(f, "?")?;
        }
        Ok(())
    }
}

/// 署名のうち、呼び出し側の検査に要る分だけ。`self` は引数に数えないので
/// 別のフラグで持つ(design.md 決定1)。
#[derive(PartialEq, Eq)]
struct FnSig {
    has_self: bool,
    params: Vec<KnownType>,
    ret: Option<KnownType>,
}

/// 宣言された署名を検査用の形にする。引数名は実装側の局所名なので落とす。
fn signature(sig: &Sig) -> FnSig {
    FnSig {
        has_self: sig.has_self,
        params: sig.params.iter().map(|p| known(&p.ty)).collect(),
        ret: sig.ret.as_ref().map(known),
    }
}

/// ローカル束縛。**型が分かっているものだけ**型を持つ。
///
/// 名前の集合ではなく表にしているのは、`u.rank = Gold` のレシーバ型を
/// 引くため。入口は型注釈付き引数・`self`・struct リテラル・enum variant・
/// 呼び出しの宣言戻り値と、それらを直接束縛・参照する式に限る。
///
/// `with` が導入する ambient スロットは値ではないので言い分ける。同名の
/// ローカルはスロットを隠す(design.md 決定3)。
#[derive(Clone)]
enum Binding {
    Value(Option<KnownType>),
    Slot(String),
}

type Locals = BTreeMap<String, Binding>;

/// 型注釈をそのまま型の事実にする。`None` は「推論できない」の意味で使うので、
/// 注釈のある所は optional でも `Some` になる(design.md 決定1)。
fn known(ty: &Type) -> KnownType {
    let kind = match &ty.kind {
        TypeKind::Named(name) => KnownKind::Named(name.clone()),
        TypeKind::Array(element) => KnownKind::Array(Box::new(known(element))),
    };
    KnownType {
        kind,
        optional: ty.optional,
    }
}

/// 後置 `?` の付かない型。variant / struct リテラル / self に使う。
fn plain(name: &str) -> KnownType {
    KnownType {
        kind: KnownKind::Named(name.to_string()),
        optional: false,
    }
}

/// 後置 `?` の付かない配列型。
fn array_of(element: KnownType) -> KnownType {
    KnownType {
        kind: KnownKind::Array(Box::new(element)),
        optional: false,
    }
}

/// 組み込みのスカラー型。ユーザー宣言はこの名前を名乗れない(design.md 決定1)。
const BUILTINS: [&str; 4] = ["bool", "int", "str", "unit"];

/// 宣言名が組み込み型と衝突するなら、その綴り。正準名は修飾されているので
/// 末尾だけを見る(`main::int` も `int` の宣言)。
fn reserved(name: &str) -> Option<&str> {
    let short = short_name(name);
    BUILTINS.contains(&short).then_some(short)
}

/// 診断の受け皿。「いま検査している式か宣言」の span を持ち、押し込まれた文言に
/// それを刻む。42 箇所の `push` は文言だけを渡すままでよく、位置の管理は
/// 走査側の1本(`check_expr_at` が式ごとに差し替える)に閉じる。
struct Out {
    diagnostics: Vec<Diag>,
    /// 走査に入る前だけ `None`。以降は必ず何かを指している
    span: Option<Span>,
}

impl Out {
    fn push(&mut self, msg: String) {
        self.diagnostics.push(Diag::from_span(self.span, msg));
    }

    /// 検査中の式より内側の部分式について報告するとき。引数や戻り値のように、
    /// 責めるべき式が実引数として渡ってきている場合に使う。
    fn push_at(&mut self, span: Span, msg: String) {
        self.diagnostics.push(Diag::at(span, msg));
    }
}

/// 診断を全件返す。空なら struct の形と分かる enum 型は正しい。
pub fn check(program: &Program) -> Vec<Diag> {
    let mut out = Out {
        diagnostics: Vec::new(),
        span: None,
    };
    let decls = collect(program, &mut out);
    let out = &mut out;

    for item in &program.items {
        out.span = Some(item.span());
        match item {
            Item::Fn { sig, body, .. } => {
                let locals = sig
                    .params
                    .iter()
                    .map(|p| (p.name.clone(), Binding::Value(Some(known(&p.ty)))))
                    .collect();
                let ret = sig.ret.as_ref().map(known);
                check_body(body, &decls, locals, &sig.name, ret.as_ref(), out);
            }
            Item::Test { name, body, .. } => {
                let ctx = format!("test \"{name}\"");
                check_body(body, &decls, Locals::new(), &ctx, None, out);
            }
            Item::Impl {
                type_name, methods, ..
            } => {
                for (sig, body) in methods {
                    let mut locals: Locals = sig
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), Binding::Value(Some(known(&p.ty)))))
                        .collect();
                    if sig.has_self {
                        locals.insert("self".to_string(), Binding::Value(Some(plain(type_name))));
                    }
                    let ctx = format!("impl {type_name}::{}", sig.name);
                    let ret = sig.ret.as_ref().map(known);
                    out.span = Some(sig.span);
                    check_body(body, &decls, locals, &ctx, ret.as_ref(), out);
                }
            }
            _ => {}
        }
    }

    std::mem::take(&mut out.diagnostics)
}

fn collect(program: &Program, out: &mut Out) -> Decls {
    let mut structs = BTreeMap::new();
    let mut variants = BTreeMap::new();
    let mut enums = BTreeMap::new();
    let mut fns = BTreeMap::new();
    let mut traits: BTreeMap<String, BTreeMap<String, FnSig>> = BTreeMap::new();
    let mut impls: BTreeMap<String, Vec<(String, FnSig)>> = BTreeMap::new();
    let mut trait_impls = BTreeSet::new();
    let mut others = BTreeSet::new();

    for item in &program.items {
        out.span = Some(item.span());
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
                others.insert(name.clone());
                for variant in vs {
                    variants.insert(variant.clone(), name.clone());
                    others.insert(variant.clone());
                }
                enums.insert(name.clone(), vs.clone());
            }
            // trait のメンバー名の重複は宣言の誤りだが、契約としては一意に保つ。
            // 実装側の過不足は `check_impl` が契約と突き合わせて報告する
            Item::Trait { name, methods, .. } => {
                others.insert(name.clone());
                traits.insert(
                    name.clone(),
                    methods
                        .iter()
                        .map(|sig| (sig.name.clone(), signature(sig)))
                        .collect(),
                );
            }
            Item::Effect { slot, .. } => {
                others.insert(slot.clone());
            }
            Item::Fn { sig, .. } => {
                others.insert(sig.name.clone());
                fns.insert(sig.name.clone(), signature(sig));
            }
            Item::Impl {
                trait_name,
                type_name,
                methods,
                ..
            } => {
                if let Some(trait_name) = trait_name {
                    trait_impls.insert((type_name.clone(), trait_name.clone()));
                }
                let entry = impls.entry(type_name.clone()).or_default();
                for (sig, _) in methods {
                    entry.push((sig.name.clone(), signature(sig)));
                }
            }
            Item::Test { .. } => {}
        }
    }

    let decls = Decls {
        structs,
        variants,
        enums,
        fns,
        traits,
        impls,
        trait_impls,
        slots: collect_slots(program),
        others,
    };

    // 契約の検査は索引が揃ってから。前方参照の trait も引ける(design.md 決定2)
    for item in &program.items {
        if let Item::Impl {
            trait_name: Some(trait_name),
            type_name,
            methods,
            ..
        } = item
        {
            out.span = Some(item.span());
            check_impl(trait_name, type_name, methods, &decls, out);
        }
    }

    decls
}

/// trait `impl` を契約と突き合わせる。呼び出しの到達性に依らせないため、
/// 宣言の時点で見る(design.md 決定2)。
fn check_impl(
    trait_name: &str,
    type_name: &str,
    methods: &[(Sig, Vec<Expr>)],
    decls: &Decls,
    out: &mut Out,
) {
    let ctx = format!("impl {trait_name} for {type_name}");
    let Some(contract) = decls.traits.get(trait_name) else {
        out.push(format!("{ctx}: `{trait_name}` は trait ではありません"));
        return;
    };
    if !decls.structs.contains_key(type_name) {
        out.push(format!("{ctx}: `{type_name}` は struct ではありません"));
        return;
    }

    let mut given: BTreeSet<&str> = BTreeSet::new();
    for (sig, _) in methods {
        if !given.insert(&sig.name) {
            out.push(format!("{ctx}: `{}` を二度実装しています", sig.name));
            continue;
        }
        let Some(declared) = contract.get(&sig.name) else {
            out.push(format!(
                "{ctx}: `{}` は `{trait_name}` に宣言されていません",
                sig.name
            ));
            continue;
        };
        // 引数名は実装側の局所名なので同一性に入れない(design.md 決定2)
        let actual = signature(sig);
        if actual.has_self != declared.has_self {
            out.push(format!(
                "{ctx}: `{}` のレシーバの形が `{trait_name}` の宣言と違います",
                sig.name
            ));
        }
        if actual.params != declared.params {
            out.push(format!(
                "{ctx}: `{}` の引数は {} ですが、{} を宣言しています",
                sig.name,
                params(&declared.params),
                params(&actual.params)
            ));
        }
        if actual.ret != declared.ret {
            out.push(format!(
                "{ctx}: `{}` の戻り値は {} ですが、{} を宣言しています",
                sig.name,
                returned(&declared.ret),
                returned(&actual.ret)
            ));
        }
    }

    let missing: Vec<&str> = contract
        .keys()
        .map(String::as_str)
        .filter(|m| !given.contains(m))
        .collect();
    if !missing.is_empty() {
        out.push(format!(
            "{ctx}: `{trait_name}` のメソッド {} を実装していません",
            quoted(&missing)
        ));
    }
}

/// 診断に出す引数型の並び。
fn params(types: &[KnownType]) -> String {
    let shown: Vec<String> = types.iter().map(|t| format!("`{t}`")).collect();
    format!("({})", shown.join(", "))
}

/// 診断に出す戻り値型。
fn returned(ty: &Option<KnownType>) -> String {
    ty.as_ref().map_or("無し".to_string(), |t| format!("`{t}`"))
}

fn check_body(
    body: &[Expr],
    decls: &Decls,
    mut locals: Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Out,
) {
    // ブロックは値ベースなので最後の式も戻り値。明示 `return` は走査側が見る。
    // 最後の式を見る場所をここ一箇所にして、入れ子で二重に出るのを防ぐ
    let Some((last, init)) = body.split_last() else {
        return;
    };
    check_exprs(init, decls, &mut locals, ctx, ret, out);
    check_expr_at(last, ret, decls, &mut locals, ctx, ret, out);
    check_return(last, decls, &locals, ctx, ret, out);
}

fn check_exprs(
    body: &[Expr],
    decls: &Decls,
    locals: &mut Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Out,
) {
    for e in body {
        check_expr(e, decls, locals, ctx, ret, out);
    }
}

/// 期待型の無い位置の式。
fn check_expr(
    e: &Expr,
    decls: &Decls,
    locals: &mut Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Out,
) {
    check_expr_at(e, None, decls, locals, ctx, ret, out);
}

/// `requirement::scan` と同じく、ブロックから戻れば内側の束縛は消える。
/// 枝へ入るときだけ `locals` を複製する。
///
/// `expected` は宛先の宣言型。配列リテラルだけが要素へ配るために使い、
/// それ以外の式は自分の型を推論するだけなので見ない。
/// 検査中の式を診断の位置にする。部分木から戻ったら外側の式へ戻す
/// (戻さないと、子を見た後の親の診断が子の位置を指してしまう)。
#[allow(clippy::too_many_arguments)]
fn check_expr_at(
    e: &Expr,
    expected: Option<&KnownType>,
    decls: &Decls,
    locals: &mut Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Out,
) {
    let outer = out.span.replace(e.span);
    check_expr_kind(e, expected, decls, locals, ctx, ret, out);
    out.span = outer;
}

#[allow(clippy::too_many_arguments)]
fn check_expr_kind(
    e: &Expr,
    expected: Option<&KnownType>,
    decls: &Decls,
    locals: &mut Locals,
    ctx: &str,
    ret: Option<&KnownType>,
    out: &mut Out,
) {
    match &e.kind {
        ExprKind::Ident(name) => check_bare(name, decls, locals, ctx, out),

        ExprKind::StructLit { name, fields } => {
            check_literal(name, fields, decls, ctx, out);
            for (field, v) in fields {
                let declared = declared_field(name, field, decls);
                check_expr_at(v, declared.as_ref(), decls, locals, ctx, ret, out);
                check_field_value(name, field, v, decls, locals, ctx, out);
            }
        }

        ExprKind::Let { name, value } => {
            check_expr(value, decls, locals, ctx, ret, out);
            let ty = infer(value, decls, locals);
            locals.insert(name.clone(), Binding::Value(ty));
        }

        ExprKind::Call(callee, args) => {
            match &callee.kind {
                // 呼び出し先の名前は値として読まれない。評価器も `Ident` / `Path` の
                // callee は関数表・型表から引くだけで、値へ落とさない
                ExprKind::Ident(_) | ExprKind::Path(_) => {}
                // `db.save(u)` の `save` はメソッド名であってフィールドの読みでは
                // ない。値になるのはレシーバだけ
                ExprKind::Field(recv, _) => check_expr(recv, decls, locals, ctx, ret, out),
                _ => check_expr(callee, decls, locals, ctx, ret, out),
            }
            // 直接呼び出し・メソッド・関連関数を1本の解決に通す。以降の個数・
            // 引数・戻り値の扱いは呼び出しの形に依らない(design.md 決定4)
            let sig = match resolve_call(callee, decls, locals) {
                Ok(sig) => sig,
                Err(message) => {
                    out.push(format!("{ctx}: {message}"));
                    None
                }
            };
            // 個数が合わなければ引数と宣言の対応が取れないので期待型は配らない
            let params = sig.map(|s| &s.params).filter(|p| p.len() == args.len());
            for (i, a) in args.iter().enumerate() {
                let expected = params.and_then(|p| p.get(i)).cloned();
                check_expr_at(a, expected.as_ref(), decls, locals, ctx, ret, out);
            }
            if let Some(sig) = sig {
                check_call(&member(callee), sig, args, decls, locals, ctx, out);
            }
        }

        ExprKind::Field(recv, field) => {
            check_expr(recv, decls, locals, ctx, ret, out);
            check_field_read(recv, field, decls, locals, ctx, out);
        }
        ExprKind::OptionalField(recv, field) => {
            check_expr(recv, decls, locals, ctx, ret, out);
            check_optional_field_read(recv, field, decls, locals, ctx, out);
        }

        ExprKind::Assign { target, value } => {
            match &target.kind {
                // 代入先の裸の名前は書き込み先であって値の読みではない。
                // 束縛の型は宣言時に決まるので、後の代入では変えない(design.md 決定4)
                ExprKind::Ident(name) => {
                    let expected = match locals.get(name) {
                        Some(Binding::Value(ty)) => ty.clone(),
                        _ => None,
                    };
                    check_expr_at(value, expected.as_ref(), decls, locals, ctx, ret, out);
                    if let Some(expected) = &expected {
                        require(
                            value,
                            expected,
                            &format!("`{name}` への代入"),
                            decls,
                            locals,
                            ctx,
                            out,
                        );
                    }
                }
                ExprKind::Field(recv, field) => {
                    check_expr(recv, decls, locals, ctx, ret, out);
                    // optional の中身を取り出す規則はまだ無いので、レシーバは
                    // 非 optional と分かるときだけ見る
                    let owner = infer(recv, decls, locals)
                        .filter(|ty| !ty.optional)
                        .and_then(|ty| Some(ty.name()?.to_string()));
                    let declared = owner.as_ref().and_then(|o| declared_field(o, field, decls));
                    check_expr_at(value, declared.as_ref(), decls, locals, ctx, ret, out);
                    if let Some(owner) = &owner {
                        check_field_value(owner, field, value, decls, locals, ctx, out);
                    }
                }
                _ => check_expr(value, decls, locals, ctx, ret, out),
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
                        check_provision(b, decls, locals, ctx, out);
                    }
                    // 本体では内側の束縛が勝つ。スロットでない名前は
                    // requirement 側が報告するので、ここでは型不明の値にする
                    let mut inner = locals.clone();
                    for b in binders {
                        let slot = b.slot();
                        let binding = match decls.slots.trait_of(slot) {
                            Some(trait_name) => Binding::Slot(trait_name.to_string()),
                            None => Binding::Value(None),
                        };
                        inner.insert(slot.to_string(), binding);
                    }
                    check_expr(body, decls, &mut inner, ctx, ret, out);
                }
                Head::If(c) | Head::Elif(c) | Head::While(c) => {
                    check_expr(c, decls, locals, ctx, ret, out);
                    require(c, &plain("bool"), "条件", decls, locals, ctx, out);
                    check_expr(body, decls, &mut locals.clone(), ctx, ret, out);
                }
                Head::For { var, iter } => {
                    check_expr(iter, decls, locals, ctx, ret, out);
                    let mut inner = locals.clone();
                    let element = iterated(iter, decls, locals, ctx, out);
                    inner.insert(var.clone(), Binding::Value(element));
                    check_expr(body, decls, &mut inner, ctx, ret, out);
                }
                Head::Else => check_expr(body, decls, &mut locals.clone(), ctx, ret, out),
            }
            if let Some(o) = orelse {
                check_expr(o, decls, &mut locals.clone(), ctx, ret, out);
            }
        }

        // 期待要素型があればそれ、無ければ最初に型の分かる要素を基準にして、
        // 残りの要素を既存の適合規則で照合する。基準が無い(空配列・全要素が
        // 型不明)なら何も言わない(design.md 決定3)
        ExprKind::Array(items) => {
            let element = match expected.and_then(KnownType::element) {
                Some(element) => Some(element.clone()),
                None => items.iter().find_map(|i| infer(i, decls, locals)),
            };
            for (n, item) in items.iter().enumerate() {
                check_expr_at(item, element.as_ref(), decls, locals, ctx, ret, out);
                if let Some(element) = &element {
                    let what = format!("配列の第 {} 要素", n + 1);
                    require(item, element, &what, decls, locals, ctx, out);
                }
            }
        }
        ExprKind::Block(body) => check_exprs(body, decls, locals, ctx, ret, out),

        // 対象は既知の非 optional な enum で、arm はその全 variant を一度ずつ。
        // 結果型は期待型、無ければ最初に型の分かる arm を基準にする
        // (design.md 決定3・5)。arm は束縛を導入しないが、本体の `let` を
        // 他の arm や後続へ漏らさないよう `locals` を複製して入る
        ExprKind::Match { subject, arms } => {
            check_expr(subject, decls, locals, ctx, ret, out);
            let matched = matched_enum(subject, decls, locals, ctx, out);
            check_arms(arms, matched.as_deref(), decls, ctx, out);

            let result = match expected {
                Some(expected) => Some(expected.clone()),
                None => arms.iter().find_map(|arm| infer(&arm.body, decls, locals)),
            };
            for arm in arms {
                check_expr_at(
                    &arm.body,
                    result.as_ref(),
                    decls,
                    &mut locals.clone(),
                    ctx,
                    ret,
                    out,
                );
                if let Some(result) = &result {
                    let what = format!("arm `{}::{}` の値", arm.enum_name, arm.variant);
                    require(&arm.body, result, &what, decls, locals, ctx, out);
                }
            }
        }
        ExprKind::Binary { op, lhs, rhs } => {
            check_expr(lhs, decls, locals, ctx, ret, out);
            check_expr(rhs, decls, locals, ctx, ret, out);
            match op {
                // 評価器の整数演算をそのまま静的にする。暗黙変換も文字列連結も無い
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                    let int = plain("int");
                    let sym = symbol(*op);
                    require(
                        lhs,
                        &int,
                        &format!("`{sym}` の左辺"),
                        decls,
                        locals,
                        ctx,
                        out,
                    );
                    require(
                        rhs,
                        &int,
                        &format!("`{sym}` の右辺"),
                        decls,
                        locals,
                        ctx,
                        out,
                    );
                }
                // `nil` は相手側の optional 性を文脈にする。それ以外は両方の型が
                // 分かるときだけ比べる。結果はどちらにせよ `bool`
                BinOp::Eq => check_equality(lhs, rhs, decls, locals, ctx, out),
                BinOp::Coalesce => check_coalesce(lhs, rhs, decls, locals, ctx, out),
            }
        }
        ExprKind::Return(Some(inner)) => {
            check_expr_at(inner, ret, decls, locals, ctx, ret, out);
            check_return(inner, decls, locals, ctx, ret, out);
        }
        ExprKind::Unary(UnOp::Neg, inner) => {
            check_expr(inner, decls, locals, ctx, ret, out);
            require(
                inner,
                &plain("int"),
                "単項 `-` の被演算子",
                decls,
                locals,
                ctx,
                out,
            );
        }
        ExprKind::Assert(inner) => {
            check_expr(inner, decls, locals, ctx, ret, out);
            require(
                inner,
                &plain("bool"),
                "`assert` の対象",
                decls,
                locals,
                ctx,
                out,
            );
        }

        // 宣言済み enum を修飾した path は variant 値。それ以外の path は
        // 関連関数や ambient の型射影なので、従来どおり何も言わない
        ExprKind::Path(parts) => {
            if let [enum_name, variant] = parts.as_slice()
                && let Some(declared) = decls.enums.get(enum_name)
                && !declared.iter().any(|v| short_name(v) == variant)
            {
                out.push(format!(
                    "{ctx}: `{enum_name}::{variant}` は `{enum_name}` の variant ではありません"
                ));
            }
        }

        ExprKind::Int(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Nil
        | ExprKind::Return(None) => {}
    }
}

/// `match` の対象の enum。既知の非 optional な enum のときだけ返す。
///
/// 型不明の対象を実行時へ委ねると arm の所属・網羅性・結果型の基準を決められない
/// ので、ここで診断する(design.md 決定3)。
fn matched_enum(
    subject: &Expr,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
) -> Option<String> {
    let Some(ty) = infer(subject, decls, locals) else {
        out.push(format!("{ctx}: `match` の対象の型が決まりません"));
        return None;
    };
    match ty.name() {
        Some(name) if !ty.optional && decls.enums.contains_key(name) => Some(name.to_string()),
        _ => {
            out.push(format!(
                "{ctx}: `match` の対象は非 optional な enum である必要がありますが、`{ty}` です"
            ));
            None
        }
    }
}

/// arm の集合が対象 enum の宣言 variant と完全に一致するか見る。
/// 対象の enum が分からないときは、arm 自身の整合だけを見る。
fn check_arms(arms: &[MatchArm], matched: Option<&str>, decls: &Decls, ctx: &str, out: &mut Out) {
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    // 呼び出し時点の span は `match` 式全体。arm を指す診断の間だけ差し替える
    let whole = out.span;
    for arm in arms {
        out.span = Some(arm.span);
        let arm_name = format!("{}::{}", arm.enum_name, arm.variant);
        let Some(declared) = decls.enums.get(&arm.enum_name) else {
            out.push(format!("{ctx}: `{}` は enum ではありません", arm.enum_name));
            continue;
        };
        if !declared.iter().any(|v| short_name(v) == arm.variant) {
            out.push(format!(
                "{ctx}: `{arm_name}` は `{}` の variant ではありません",
                arm.enum_name
            ));
            continue;
        }
        let Some(matched) = matched else { continue };
        if arm.enum_name != matched {
            out.push(format!(
                "{ctx}: arm `{arm_name}` は `{matched}` の variant ではありません"
            ));
            continue;
        }
        if !covered.insert(arm.variant.as_str()) {
            out.push(format!("{ctx}: arm `{arm_name}` が重複しています"));
        }
    }

    out.span = whole;

    // 欠落は「無いもの」なので指すべき arm が無い。式全体が唯一正しい位置
    let Some(matched) = matched else { return };
    let missing: Vec<&str> = decls.enums[matched]
        .iter()
        .map(|variant| short_name(variant))
        .filter(|variant| !covered.contains(variant))
        .collect();
    if !missing.is_empty() {
        out.push(format!(
            "{ctx}: `{matched}` の variant {} を扱っていません",
            quoted(&missing)
        ));
    }
}

/// 期待型のある位置で値を照合し、不一致なら診断用の実型名を返す。
///
/// `nil` は自分だけでは nominal 型を持たず、文脈が optional のときだけ適合する。
/// 同名の非 optional 値は optional の期待型へ一方向に注入できる。式の推論型は
/// 変えない。それ以外の型不明式は後続 change のために従来どおり保留する。
fn mismatch(value: &Expr, expected: &KnownType, decls: &Decls, locals: &Locals) -> Option<String> {
    if matches!(value.kind, ExprKind::Nil) {
        return (!expected.optional).then(|| "nil".to_string());
    }
    // 期待型のある位置に直接書かれた配列リテラルは生成の境界。要素は
    // `check_expr` が期待要素型で照合済みなので、ここでは配列全体を
    // 適合として扱う(design.md 決定4)
    if matches!(value.kind, ExprKind::Array(_)) && expected.element().is_some() {
        return None;
    }
    // 期待型のある位置の match も同じ。期待型は各 arm へそのまま配られ、
    // `check_expr` が arm ごとに照合済みなので、式全体では二度言わない
    if matches!(value.kind, ExprKind::Match { .. }) {
        return None;
    }
    let actual = infer(value, decls, locals)?;
    let compatible = actual == *expected
        || (actual.kind == expected.kind && !actual.optional && expected.optional);
    (!compatible).then(|| actual.to_string())
}

/// 宣言済み struct の宣言フィールドの型。期待型を配るためだけに引く。
fn declared_field(type_name: &str, field: &str, decls: &Decls) -> Option<KnownType> {
    decls.structs.get(type_name)?.get(field).cloned()
}

/// `for x in xs` のループ変数の型。反復対象は非 optional な配列でなければ
/// ならない。型が分からないときは従来どおり何も言わず、変数も不明にする
/// (design.md 決定5)。
fn iterated(
    iter: &Expr,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
) -> Option<KnownType> {
    let ty = infer(iter, decls, locals)?;
    let Some(element) = ty.element() else {
        out.push(format!(
            "{ctx}: `for` の反復対象は配列である必要がありますが、`{ty}` です"
        ));
        return None;
    };
    if ty.optional {
        out.push(format!(
            "{ctx}: optional な配列 `{ty}` はそのまま反復できません。先に `??` で展開してください"
        ));
        return None;
    }
    Some(element.clone())
}

/// 呼び出し先の署名を選ぶ(design.md 決定4)。
///
/// `Ok(None)` はレシーバの型が分からないので保留、`Err` は候補の無さ・曖昧さ・
/// レシーバの形の不一致で、呼び出し側が文脈を付けて報告する。選び方は評価器と
/// 同じ順序で、スロット経由は契約だけ、具体型は inherent と trait 実装をまとめて
/// 名前で絞り、一意になってから `.` と `::` の別を見る。
fn resolve_call<'d>(
    callee: &Expr,
    decls: &'d Decls,
    locals: &Locals,
) -> Result<Option<&'d FnSig>, String> {
    match &callee.kind {
        ExprKind::Ident(name) => Ok(decls.fns.get(name)),
        ExprKind::Field(recv, name) => {
            if let ExprKind::Ident(recv_name) = &recv.kind
                && let Some(trait_name) = slot_trait(recv_name, decls, locals)
            {
                return from_trait(&trait_name, name, true, decls);
            }
            let Some(ty) = infer(recv, decls, locals) else {
                return Ok(None);
            };
            // optional の中身を取り出す規則はまだ無く、配列にメソッドも無い
            match ty.name().filter(|_| !ty.optional) {
                Some(type_name) => from_type(type_name, name, true, decls),
                None => Ok(None),
            }
        }
        ExprKind::Path(parts) => {
            let [first, name] = parts.as_slice() else {
                return Ok(None);
            };
            match slot_trait(first, decls, locals) {
                Some(trait_name) => from_trait(&trait_name, name, false, decls),
                None => from_type(first, name, false, decls),
            }
        }
        _ => Ok(None),
    }
}

/// 名前がこの位置で ambient スロットなら、その宣言 trait 名。最も内側の束縛が
/// 勝つので、同名のローカルはスロットを隠す(eval.rs `name_is_slot` と同じ規則)。
fn slot_trait(name: &str, decls: &Decls, locals: &Locals) -> Option<String> {
    match locals.get(name) {
        Some(Binding::Slot(trait_name)) => Some(trait_name.clone()),
        Some(Binding::Value(_)) => None,
        None => decls.slots.trait_of(name).map(str::to_string),
    }
}

/// スロット経由の選択。実行時の具体型は意図的に変わるので契約だけを見る。
fn from_trait<'d>(
    trait_name: &str,
    name: &str,
    dot: bool,
    decls: &'d Decls,
) -> Result<Option<&'d FnSig>, String> {
    let Some(contract) = decls.traits.get(trait_name) else {
        return Ok(None);
    };
    let Some(sig) = contract.get(name) else {
        return Err(format!("`{trait_name}` に `{name}` はありません"));
    };
    receiver_form(sig, trait_name, name, dot)
}

/// 具体型からの選択。inherent と trait 実装を混ぜて名前で絞り、一意を要求する。
fn from_type<'d>(
    type_name: &str,
    name: &str,
    dot: bool,
    decls: &'d Decls,
) -> Result<Option<&'d FnSig>, String> {
    let mut named = decls
        .impls
        .get(type_name)
        .into_iter()
        .flatten()
        .filter(|(member, _)| member == name);
    let Some((_, first)) = named.next() else {
        // 宣言を知らない型のメンバーは今回の推論の外
        if !decls.structs.contains_key(type_name) {
            return Ok(None);
        }
        return Err(format!("`{type_name}` に `{name}` はありません"));
    };
    // 候補が2つ以上あるのに trait が分からない。スロット経由なら契約で絞れるので、
    // ここに来るのは具体型から引いたときだけ
    if named.next().is_some() {
        return Err(format!(
            "`{type_name}` の `{name}` がどの trait のものか決まりません"
        ));
    }
    receiver_form(first, type_name, name, dot)
}

/// 呼び出しの構文と宣言されたレシーバの形を突き合わせる。
fn receiver_form<'d>(
    sig: &'d FnSig,
    owner: &str,
    name: &str,
    dot: bool,
) -> Result<Option<&'d FnSig>, String> {
    match (sig.has_self, dot) {
        (false, true) => Err(format!(
            "`{owner}::{name}` は self を取りません。`{owner}::{name}()` で呼びます"
        )),
        (true, false) => Err(format!(
            "`{owner}::{name}` はレシーバが必要です。値から `.{name}()` で呼びます"
        )),
        _ => Ok(Some(sig)),
    }
}

/// 診断に出す呼び出し先の綴り。
fn member(callee: &Expr) -> String {
    match &callee.kind {
        ExprKind::Ident(name) | ExprKind::Field(_, name) => name.clone(),
        ExprKind::Path(parts) => parts.join("::"),
        _ => String::new(),
    }
}

/// `with` の提供がスロットの契約を満たすか見る。`with db<Postgres>` は型名が
/// そのまま分かるので常に、`with db(v)` は `v` の型が分かるときだけ見る
/// (design.md 決定3)。スロットでない名前は requirement / eval 側が報告する。
///
/// eval.rs にも同じ判定がある。あちらは型の分からない提供を実行時に止める網。
fn check_provision(b: &Provision, decls: &Decls, locals: &Locals, ctx: &str, out: &mut Out) {
    let Some(want) = decls.slots.trait_of(b.slot()) else {
        return;
    };
    let ty = match b {
        Provision::Type { type_name, .. } => plain(type_name),
        Provision::Value { value, .. } => match infer(value, decls, locals) {
            Some(ty) => ty,
            None => return,
        },
    };
    // 提供できるのは trait を実装した具体型そのものだけ。配列と optional は
    // 名前を持たないので、この時点で落ちる
    let implemented = !ty.optional
        && ty.name().is_some_and(|name| {
            decls
                .trait_impls
                .contains(&(name.to_string(), want.to_string()))
        });
    if !implemented {
        out.push(format!(
            "{ctx}: `{ty}` は `{want}` を実装していないので `{}` に提供できません",
            b.slot()
        ));
    }
}

/// 解決できた呼び出しの検査。引数の個数は常に、型は文脈と照合できるときだけ見る。
fn check_call(
    name: &str,
    sig: &FnSig,
    args: &[Expr],
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
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
        let Some(actual) = mismatch(arg, expected, decls, locals) else {
            continue;
        };
        out.push_at(
            arg.span,
            format!(
                "{ctx}: `{name}` の第 {} 引数は `{expected}` ですが、`{actual}` を渡しています",
                i + 1
            ),
        );
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
    out: &mut Out,
) {
    let Some(expected) = ret else {
        return;
    };
    let Some(actual) = mismatch(value, expected, decls, locals) else {
        return;
    };
    out.push_at(
        value.span,
        format!("{ctx}: 戻り値は `{expected}` ですが、`{actual}` を返しています"),
    );
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
            Some(Binding::Value(known)) => known.clone(),
            // スロットは値ではない。読みの診断は requirement / eval の側にある
            Some(Binding::Slot(_)) => None,
            None => decls.variants.get(name).map(|e| plain(e)),
        },
        // `Rank::Gold` — 宣言済み enum の variant を限定した path は、裸の
        // `Gold` と同じ enum 値。ローカルは限定参照を隠さない(design.md 決定2)
        ExprKind::Path(parts) => {
            let [enum_name, variant] = parts.as_slice() else {
                return None;
            };
            decls
                .enums
                .get(enum_name)?
                .iter()
                .any(|v| short_name(v) == variant)
                .then(|| plain(enum_name))
        }
        ExprKind::StructLit { name, .. } => Some(plain(name)),
        // 素直な再帰なので `user.profile.name` の連鎖もそのまま辿れる。
        // 診断は `check_field_read` の側にあるので、ここは事実を引くだけ
        ExprKind::Field(recv, field) => {
            let ty = infer(recv, decls, locals)?;
            if ty.optional {
                return None;
            }
            decls.structs.get(ty.name()?)?.get(field).cloned()
        }
        ExprKind::OptionalField(recv, field) => {
            let ty = infer(recv, decls, locals)?;
            if !ty.optional {
                return None;
            }
            let mut field_ty = decls.structs.get(ty.name()?)?.get(field).cloned()?;
            // optional は1 bit。宣言型が既に T? でも結果は T? のまま。
            field_ty.optional = true;
            Some(field_ty)
        }
        // 解決できた呼び出しは宣言戻り値を持つ。診断は `check_expr` の側にある
        ExprKind::Call(callee, _) => resolve_call(callee, decls, locals)
            .ok()
            .flatten()?
            .ret
            .clone(),
        // 全要素の型が分かって一致するときだけ配列型になる。空配列と、
        // 型の分からない要素や矛盾する要素を含む配列は不明のまま
        // (design.md 決定3)。矛盾の診断は `check_expr` の側にある
        ExprKind::Array(items) => {
            let mut element: Option<KnownType> = None;
            for item in items {
                let ty = infer(item, decls, locals)?;
                match &element {
                    Some(first) if *first != ty => return None,
                    Some(_) => {}
                    None => element = Some(ty),
                }
            }
            Some(array_of(element?))
        }
        // 期待型の無い `match` は最初に型の分かる arm を結果型にする。
        // 残りの arm との照合は `check_expr` の側にある(design.md 決定5)
        ExprKind::Match { arms, .. } => arms.iter().find_map(|arm| infer(&arm.body, decls, locals)),
        // 演算子の結果型は被演算子に依らず決まる。被演算子の診断は
        // `check_expr` の側にある(design.md 決定3)
        ExprKind::Unary(UnOp::Neg, _) => Some(plain("int")),
        ExprKind::Binary { op, .. } => match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => Some(plain("int")),
            BinOp::Eq => Some(plain("bool")),
            BinOp::Coalesce => infer_coalesce(e, decls, locals),
        },
        _ => None,
    }
}

/// 使用位置が要求する型と照合する。**型が分かるときだけ**見る(design.md 決定3)。
fn require(
    e: &Expr,
    expected: &KnownType,
    what: &str,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
) {
    let Some(actual) = mismatch(e, expected, decls, locals) else {
        return;
    };
    out.push_at(
        e.span,
        format!("{ctx}: {what}は `{expected}` ですが、`{actual}` です"),
    );
}

/// `T? ?? T` の被演算子を検査する。右辺の直接 `return` は値を産まずに
/// 枝を終えるので、中身の型との照合は不要。
fn check_coalesce(
    lhs: &Expr,
    rhs: &Expr,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
) {
    if matches!(lhs.kind, ExprKind::Nil) {
        if let Some(actual) = infer(rhs, decls, locals)
            && actual.optional
        {
            out.push(format!(
                "{ctx}: `??` の右辺には非 optional の値が必要ですが、`{actual}` です"
            ));
        }
        return;
    }

    let Some(left) = infer(lhs, decls, locals) else {
        return;
    };
    if !left.optional {
        out.push(format!(
            "{ctx}: `??` の左辺は optional である必要がありますが、`{left}` です"
        ));
        return;
    }
    if matches!(rhs.kind, ExprKind::Return(_)) {
        return;
    }

    let expected = KnownType {
        kind: left.kind,
        optional: false,
    };
    require(rhs, &expected, "`??` の右辺", decls, locals, ctx, out);
}

/// 診断を出さずに `??` の継続経路の型を得る。
fn infer_coalesce(e: &Expr, decls: &Decls, locals: &Locals) -> Option<KnownType> {
    let ExprKind::Binary {
        op: BinOp::Coalesce,
        lhs,
        rhs,
    } = &e.kind
    else {
        return None;
    };

    if matches!(lhs.kind, ExprKind::Nil) {
        let right = infer(rhs, decls, locals)?;
        return (!right.optional).then_some(right);
    }

    let mut left = infer(lhs, decls, locals)?;
    if !left.optional {
        return None;
    }
    left.optional = false;
    Some(left)
}

/// 等価比較の互換性。`nil` は反対側の optional 性を文脈にする。
fn check_equality(
    lhs: &Expr,
    rhs: &Expr,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
) {
    let pair = match (&lhs.kind, &rhs.kind) {
        (ExprKind::Nil, ExprKind::Nil) => return,
        (ExprKind::Nil, _) => infer(rhs, decls, locals)
            .filter(|ty| !ty.optional)
            .map(|ty| ("nil".to_string(), ty.to_string())),
        (_, ExprKind::Nil) => infer(lhs, decls, locals)
            .filter(|ty| !ty.optional)
            .map(|ty| (ty.to_string(), "nil".to_string())),
        _ => match (infer(lhs, decls, locals), infer(rhs, decls, locals)) {
            (Some(l), Some(r)) if l != r => Some((l.to_string(), r.to_string())),
            _ => None,
        },
    };
    if let Some((left, right)) = pair {
        out.push(format!(
            "{ctx}: `==` の両辺は同じ型である必要がありますが、`{left}` と `{right}` です"
        ));
    }
}

fn symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Coalesce => "??",
    }
}

/// 型の分かる非 optional のレシーバは、宣言済み struct の宣言フィールドしか読めない。
/// optional のレシーバには明示的な `.?` を要求する。
fn check_field_read(
    recv: &Expr,
    field: &str,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
) {
    let Some(ty) = infer(recv, decls, locals) else {
        return;
    };
    if ty.optional {
        out.push(format!(
            "{ctx}: optional 型 `{ty}` から `{field}` を読むには `.?{field}` を使うか、先に `??` で展開してください"
        ));
        return;
    }
    let Some(declared) = ty.name().and_then(|name| decls.structs.get(name)) else {
        out.push(format!(
            "{ctx}: `{ty}` は struct ではないので `{field}` を読めません"
        ));
        return;
    };
    if !declared.contains_key(field) {
        out.push(format!("{ctx}: `{ty}` にフィールド `{field}` はありません"));
    }
}

/// `S?.?field` は S の宣言フィールドを読み、結果へ optional bit を立てる。
fn check_optional_field_read(
    recv: &Expr,
    field: &str,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
) {
    let Some(ty) = infer(recv, decls, locals) else {
        return;
    };
    if !ty.optional {
        out.push(format!(
            "{ctx}: `.?{field}` のレシーバは optional である必要がありますが、`{ty}` です"
        ));
        return;
    }
    let Some(declared) = ty.name().and_then(|name| decls.structs.get(name)) else {
        out.push(format!(
            "{ctx}: `{ty}` の中身は struct ではないので `.?{field}` を読めません"
        ));
        return;
    };
    if !declared.contains_key(field) {
        out.push(format!(
            "{ctx}: `{}` にフィールド `{field}` はありません",
            ty.name().unwrap_or_default()
        ));
    }
}

/// 宛先の宣言型と値の型が**両方分かる**ときだけ照合する。enum の取り違えも
/// スカラーの取り違えも optional の有無も、この1本が吸収する(design.md 決定4)。
fn check_field_value(
    type_name: &str,
    field: &str,
    value: &Expr,
    decls: &Decls,
    locals: &Locals,
    ctx: &str,
    out: &mut Out,
) {
    let Some(declared) = decls.structs.get(type_name).and_then(|f| f.get(field)) else {
        return;
    };
    let Some(actual) = mismatch(value, declared, decls, locals) else {
        return;
    };
    out.push_at(
        value.span,
        format!(
            "{ctx}: `{type_name}` のフィールド `{field}` は `{declared}` ですが、`{actual}` を与えています"
        ),
    );
}

/// 裸の名前が値になれるのは、隠されていないフィールド0個の struct か
/// enum variant のときだけ。
fn check_bare(name: &str, decls: &Decls, locals: &Locals, ctx: &str, out: &mut Out) {
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

fn check_literal(name: &str, fields: &[(String, Expr)], decls: &Decls, ctx: &str, out: &mut Out) {
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
        diagnostics(src).into_iter().map(|d| d.msg).collect()
    }

    fn diagnostics(src: &str) -> Vec<Diag> {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        check(&program)
    }

    fn only(src: &str) -> String {
        let errors = errors(src);
        assert_eq!(errors.len(), 1, "{errors:?}");
        errors.into_iter().next().unwrap()
    }

    /// 診断が指す範囲をソースから切り出す。span が無ければ失敗させる。
    fn spanned(src: &str, expected_msg_part: &str) -> String {
        let diagnostics = diagnostics(src);
        let found = diagnostics
            .iter()
            .find(|d| d.msg.contains(expected_msg_part))
            .unwrap_or_else(|| panic!("`{expected_msg_part}` を含む診断がない: {diagnostics:?}"));
        let span = found.span.expect("実行前の診断は位置を持つ");
        src[span.start as usize..span.end as usize].to_string()
    }

    // ---- 診断の位置 ----

    /// 4つの形(式・宣言・arm・match 式)それぞれで、指すものを固定する。
    #[test]
    fn 式の診断はその式を指す() {
        assert_eq!(
            spanned(
                "struct Card { n: int }
fn main() { Card { n = \"x\" } }
",
                "フィールド `n`"
            ),
            "\"x\""
        );
    }

    #[test]
    fn 宣言の診断はその宣言を指す() {
        assert_eq!(
            spanned(
                "struct User { rank: Rank
rank: Rank }
",
                "struct `User`"
            ),
            "struct User { rank: Rank\nrank: Rank }"
        );
    }

    #[test]
    fn armの診断はそのarmを指す() {
        let src = "enum Rank { Bronze Gold }
                   fn main(r: Rank) {
                   \x20 match r {\n\x20   Rank::Bronze: 1\n\x20   Rank::Bronze: 2\n\x20 }\n                   }\n";
        assert_eq!(spanned(src, "が重複しています"), "Rank::Bronze: 2");
    }

    #[test]
    fn 欠落variantの診断はmatch式を指す() {
        let src = "enum Rank { Bronze Gold }\n                   fn main(r: Rank) { match r { Rank::Bronze: 1 } }\n";
        assert_eq!(
            spanned(src, "を扱っていません"),
            "match r { Rank::Bronze: 1 }"
        );
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
                "struct User { id: int\nrank: int }\n\
                 fn main() { User { id = 1, rank = 2 } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 宣言と順序が違っても通る() {
        assert!(
            errors(
                "struct User { id: int\nrank: int }\n\
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
            "struct User { id: int\nrank: int }\n\
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
            errors("struct User { id: int }\nfn f(xs: [User]) { for User in xs { User } }\n")
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
                 impl Store { fn first(self -> Users) { self.users } }\n",
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
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
        assert!(e.contains("`rank`"), "{e}");
    }

    #[test]
    fn variantを束縛したローカルも型が分かる() {
        let e = only(&format!(
            "{RANKS}fn main() {{\n let g = High\n User {{ rank = g }}\n}}\n"
        ));
        assert!(e.contains("`Grade`"), "{e}");
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
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn selfレシーバへの代入も検査する() {
        let e = only(&format!(
            "{RANKS}impl User {{\n fn demote(self) {{ self.rank = Low }}\n}}\n"
        ));
        assert!(e.contains("impl User::demote"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn structリテラルを束縛したローカルはレシーバ型が分かる() {
        let e = only(&format!(
            "{RANKS}fn main() {{\n let u = User {{ rank = Gold }}\n u.rank = Low\n}}\n"
        ));
        assert!(e.contains("`Grade`"), "{e}");
    }

    // ---- 4b. 限定した variant ----

    #[test]
    fn 限定したvariantは裸の参照と同じ値になる() {
        assert!(
            errors(&format!(
                "{RANKS}fn take(r: Rank) {{ r }}\n\
                 fn f(-> bool) {{\n\
                 \x20 take(Rank::Gold)\n\
                 \x20 User {{ rank = Rank::Bronze }}\n\
                 \x20 let g = Rank::Gold\n\
                 \x20 g == Gold\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 限定したvariantはローカルに隠されない() {
        // 裸の `Gold` はローカルを指すが、`Rank::Gold` は宣言を通り続ける
        let e = only(&format!(
            "{RANKS}fn f(Gold: int -> bool) {{ Rank::Gold == Gold }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`int`"), "{e}");
    }

    #[test]
    fn 別のenumの限定variantを報告する() {
        let e = only(&format!("{RANKS}fn f(-> Rank) {{ Grade::Low }}\n"));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 宣言に無い限定variantを報告する() {
        let e = only(&format!("{RANKS}fn f() {{ Rank::Silver }}\n"));
        assert!(e.contains("`Rank::Silver`"), "{e}");
        assert!(e.contains("variant ではありません"), "{e}");
    }

    #[test]
    fn enumでない修飾は限定variantにならない() {
        // 関連関数の path はこれまでどおり値にならず、診断も増えない
        assert!(
            errors(&format!(
                "{RANKS}struct Store {{}}\n\
                 impl Store {{ fn make(-> User) {{ User {{ rank = Gold }} }} }}\n\
                 fn f(-> User) {{ Store::make() }}\n"
            ))
            .is_empty()
        );
        assert!(errors(&format!("{RANKS}fn f() {{ User::nope }}\n")).is_empty());
    }

    // ---- 4c. match ----

    #[test]
    fn 網羅的なmatchは診断を出さない() {
        assert!(
            errors(&format!(
                "{RANKS}fn label(r: Rank -> str) {{\n\
                 \x20 match r {{\n\
                 \x20   Rank::Bronze: \"bronze\"\n\
                 \x20   Rank::Gold: \"gold\"\n\
                 \x20 }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn matchの対象は既知の非optionalなenumに限る() {
        for (subject, expected) in [
            ("n: int", "`int` です"),
            ("r: Rank?", "`Rank?` です"),
            ("u: User", "`User` です"),
            ("xs: [Rank]", "`[Rank]` です"),
        ] {
            let e = only(&format!(
                "{RANKS}fn f({subject}) {{ match {} {{ Rank::Bronze: 1\nRank::Gold: 2 }} }}\n",
                subject.split(':').next().unwrap()
            ));
            assert!(e.contains("非 optional な enum"), "{subject}: {e}");
            assert!(e.contains(expected), "{subject}: {e}");
        }

        let e = only(&format!(
            "{RANKS}fn unknown() {{ nil }}\n\
             fn f() {{ match unknown() {{ Rank::Bronze: 1\nRank::Gold: 2 }} }}\n"
        ));
        assert!(e.contains("`match` の対象の型が決まりません"), "{e}");
    }

    #[test]
    fn 欠けているarmを列挙して報告する() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank -> int) {{ match r {{ Rank::Gold: 1 }} }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Bronze`"), "{e}");
        assert!(!e.contains("`Gold`"), "{e}");
    }

    #[test]
    fn 重複したarmを報告する() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank -> int) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Bronze: 1\n\
             \x20   Rank::Gold: 2\n\
             \x20   Rank::Gold: 3\n\
             \x20 }}\n\
             }}\n"
        ));
        assert!(e.contains("arm `Rank::Gold` が重複"), "{e}");
    }

    #[test]
    fn 別のenumのarmと宣言に無いarmを報告する() {
        let errors = errors(&format!(
            "{RANKS}fn f(r: Rank -> int) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Bronze: 1\n\
             \x20   Rank::Gold: 2\n\
             \x20   Grade::Low: 3\n\
             \x20   Rank::Silver: 4\n\
             \x20   User::Nope: 5\n\
             \x20 }}\n\
             }}\n"
        ));
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(
            errors[0].contains("arm `Grade::Low` は `Rank` の variant ではありません"),
            "{errors:?}"
        );
        assert!(
            errors[1].contains("`Rank::Silver` は `Rank` の variant ではありません"),
            "{errors:?}"
        );
        assert!(
            errors[2].contains("`User` は enum ではありません"),
            "{errors:?}"
        );
    }

    #[test]
    fn 空のenumはarmゼロで網羅的() {
        assert!(
            errors("enum Never {}\nfn f(n: Never -> int) { match n { } }\n")
                .iter()
                .all(|e| !e.contains("variant")),
            "空の enum に扱い漏れは無い"
        );
    }

    #[test]
    fn 期待型のあるmatchは全armをその型で照合する() {
        assert!(
            errors(&format!(
                "{RANKS}fn take(s: str) {{ s }}\n\
                 fn f(r: Rank) {{ take(match r {{ Rank::Bronze: \"b\"\nRank::Gold: \"g\" }}) }}\n"
            ))
            .is_empty()
        );

        let e = only(&format!(
            "{RANKS}fn take(s: str) {{ s }}\n\
             fn f(r: Rank) {{ take(match r {{ Rank::Bronze: \"b\"\nRank::Gold: 1 }}) }}\n"
        ));
        assert!(e.contains("arm `Rank::Gold` の値"), "{e}");
        assert!(e.contains("`str`"), "{e}");
        assert!(e.contains("`int`"), "{e}");
    }

    #[test]
    fn 期待型が無いmatchは最初に型の分かるarmを基準にする() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank) {{\n\
             \x20 let label = match r {{ Rank::Bronze: \"b\"\nRank::Gold: 1 }}\n\
             }}\n"
        ));
        assert!(e.contains("arm `Rank::Gold` の値"), "{e}");
        assert!(e.contains("`str`"), "{e}");
    }

    #[test]
    fn matchの結果型は後続の検査へ届く() {
        // 束縛を経由しても推論した結果型が残る
        let e = only(&format!(
            "{RANKS}fn take(n: int) {{ n }}\n\
             fn f(r: Rank) {{\n\
             \x20 let label = match r {{ Rank::Bronze: \"b\"\nRank::Gold: \"g\" }}\n\
             \x20 take(label)\n\
             }}\n"
        ));
        assert!(e.contains("`take` の第 1 引数"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`str`"), "{e}");
    }

    #[test]
    fn 宣言戻り値は各armへ配られ二度は報告しない() {
        let errors = errors(&format!(
            "{RANKS}fn f(r: Rank -> str) {{ match r {{ Rank::Bronze: 1\nRank::Gold: 2 }} }}\n"
        ));
        assert_eq!(errors.len(), 2, "arm ごとに一度ずつ: {errors:?}");
        assert!(errors[0].contains("arm `Rank::Bronze` の値"), "{errors:?}");
        assert!(errors[1].contains("arm `Rank::Gold` の値"), "{errors:?}");
        assert!(
            errors
                .iter()
                .all(|e| e.contains("`str`") && e.contains("`int`")),
            "{errors:?}"
        );
    }

    #[test]
    fn 非optionalなarmはoptionalな期待型へ注入できる() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank -> Rank?) {{ match r {{ Rank::Bronze: Gold\nRank::Gold: nil }} }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の分からないarmだけのmatchは結果型を持たない() {
        // ブロック形の arm と `return` は推論の外。存在しない型を作らない
        assert!(
            errors(&format!(
                "{RANKS}fn take(n: int) {{ n }}\n\
                 fn f(r: Rank -> int) {{\n\
                 \x20 take(match r {{\n\
                 \x20   Rank::Bronze {{ \"b\" }}\n\
                 \x20   Rank::Gold: return 0\n\
                 \x20 }})\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn armの束縛は他のarmと後続へ漏れない() {
        let errors = errors(&format!(
            "{RANKS}fn f(r: Rank) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Bronze {{ let u = User {{ rank = Gold }}\nu }}\n\
             \x20   Rank::Gold {{ u.rank = Low }}\n\
             \x20 }}\n\
             \x20 u.rank = Low\n\
             }}\n"
        ));
        assert!(
            errors.is_empty(),
            "隣の arm と後続では `u` の型が分からないので照合しない: {errors:?}"
        );
    }

    // ---- 5. 今回の保証外 ----

    #[test]
    fn 型の分からないレシーバへの代入は診断しない() {
        assert!(errors(&format!("{RANKS}fn f(u: User?) {{ u.rank = Low }}\n")).is_empty());
    }

    #[test]
    fn メソッド呼び出しの結果はレシーバ型として届く() {
        let e = only(&format!(
            "{RANKS}struct Store {{ id: int }}\n\
             impl Store {{ fn get(self -> User) {{ User {{ rank = Gold }} }} }}\n\
             fn main(s: Store) {{ s.get().rank = Low }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 関連関数呼び出しの結果もレシーバ型として届く() {
        let e = only(&format!(
            "{RANKS}struct Store {{}}\n\
             impl Store {{ fn make(-> User) {{ User {{ rank = Gold }} }} }}\n\
             fn main() {{ Store::make().rank = Low }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
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
    fn 非optional引数は同名のoptional引数型へ注入できる() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank?) {{ r }}\nfn main(r: Rank) {{ f(r) }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn optional引数は非optional引数型へ注入できない() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank) {{ r }}\nfn main(r: Rank?) {{ f(r) }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Rank?`"), "{e}");
    }

    #[test]
    fn nil引数は期待するoptional性と照合する() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank?) {{ r }}\nfn main() {{ f(nil) }}\n"
            ))
            .is_empty()
        );
        let e = only(&format!(
            "{RANKS}fn f(r: Rank) {{ r }}\nfn main() {{ f(nil) }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`nil`"), "{e}");
    }

    // ---- 7. 直接呼び出しの戻り値型 ----

    #[test]
    fn 呼び出し結果を束縛したローカルは型が分かる() {
        let e = only(&format!(
            "{RANKS}fn pick(-> Grade) {{ Low }}\n\
             fn main() {{\n let g = pick()\n User {{ rank = g }}\n}}\n"
        ));
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 呼び出し結果はそのままフィールド検査に届く() {
        let e = only(&format!(
            "{RANKS}fn pick(-> Grade) {{ Low }}\nfn main() {{ User {{ rank = pick() }} }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 呼び出し結果への代入もレシーバ型が分かる() {
        let e = only(&format!(
            "{RANKS}fn get(-> User) {{ User {{ rank = Gold }} }}\n\
             fn main() {{ get().rank = Low }}\n"
        ));
        assert!(e.contains("`Grade`"), "{e}");
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
    fn nil戻り値は期待するoptional性と照合する() {
        assert!(errors(&format!("{RANKS}fn pick(-> Rank?) {{ nil }}\n")).is_empty());
        let e = only(&format!("{RANKS}fn pick(-> Grade) {{ nil }}\n"));
        assert!(e.contains("`Grade`"), "{e}");
        assert!(e.contains("`nil`"), "{e}");
    }

    #[test]
    fn 非optional戻り値は同名のoptional戻り値型へ注入できる() {
        assert!(
            errors(&format!(
                "{RANKS}fn final_value(-> Rank?) {{ Gold }}\n\
                 fn explicit_value(-> Rank?) {{ return Bronze }}\n"
            ))
            .is_empty()
        );
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

    // ---- 10. 普通のフィールドの読み ----

    /// フィールドの読みの検査環境。連鎖が辿れることを見たいので2段にしてある
    const NESTED: &str = "enum Rank { Bronze Gold }\n\
                          struct Profile { rank: Rank }\n\
                          struct User { profile: Profile\nage: int }\n";

    #[test]
    fn 宣言フィールドの読みは宣言型を持つ() {
        assert!(errors(&format!("{NESTED}fn f(u: User -> int) {{ u.age }}\n")).is_empty());
    }

    #[test]
    fn フィールドの連鎖も宣言型を辿る() {
        assert!(
            errors(&format!(
                "{NESTED}fn f(u: User -> Rank) {{ u.profile.rank }}\n"
            ))
            .is_empty()
        );
        let e = only(&format!(
            "{NESTED}fn f(u: User -> int) {{ u.profile.rank }}\n"
        ));
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
    }

    #[test]
    fn 宣言に無いフィールドの読みを報告する() {
        let e = only(&format!("{NESTED}fn f(u: User) {{ u.nope }}\n"));
        assert!(e.contains("`User`"), "{e}");
        assert!(e.contains("`nope`"), "{e}");
    }

    #[test]
    fn structでない型からのフィールドの読みを報告する() {
        let e = only(&format!("{NESTED}fn f(n: int) {{ n.age }}\n"));
        assert!(e.contains("`int` は struct ではない"), "{e}");
    }

    #[test]
    fn 型の分からないレシーバのフィールドは診断しない() {
        // 型の分からない反復対象の束縛と、戻り値型の無い呼び出し結果は推論の外
        assert!(
            errors(&format!(
                "{NESTED}fn unknown() {{ nil }}\n\
                 fn f() {{\n\
                 \x20 for x in unknown() {{ x.nope }}\n\
                 \x20 unknown().?nope\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    // ---- 11. optional field access ----

    const OPTIONAL_FIELDS: &str = "struct Profile { name: str\nalias: str? }\n\
                                   struct User { profile: Profile\nmanager: User? }\n";

    #[test]
    fn optional_fieldは結果にoptionalを付けて既存optionalを平坦化する() {
        assert!(
            errors(&format!(
                "{OPTIONAL_FIELDS}fn name(u: User? -> str?) {{ u.?profile.?name }}\n\
                 fn manager(u: User? -> User?) {{ u.?manager }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn optional_fieldの不正なレシーバとフィールドを報告する() {
        let e = only(&format!("{OPTIONAL_FIELDS}fn f(u: User?) {{ u.?nope }}\n"));
        assert!(e.contains("`User`"), "{e}");
        assert!(e.contains("`nope`"), "{e}");

        let e = only("fn f(n: int?) { n.?value }\n");
        assert!(e.contains("`int?`"), "{e}");
        assert!(e.contains("struct ではない"), "{e}");

        let e = only(&format!(
            "{OPTIONAL_FIELDS}fn f(u: User) {{ u.?profile }}\n"
        ));
        assert!(e.contains("optional である必要"), "{e}");
        assert!(e.contains("`User`"), "{e}");
    }

    #[test]
    fn 普通のfieldはoptionalレシーバを暗黙に展開しない() {
        let e = only(&format!(
            "{OPTIONAL_FIELDS}fn f(u: User?) {{ u.profile }}\n"
        ));
        assert!(e.contains("`.?profile`"), "{e}");

        let e = only(&format!(
            "{OPTIONAL_FIELDS}fn f(u: User?) {{ u.?profile.name }}\n"
        ));
        assert!(e.contains("`.?name`"), "{e}");
    }

    #[test]
    fn optional_fieldの結果はfallbackと既存照合へ届く() {
        assert!(
            errors(&format!(
                "{OPTIONAL_FIELDS}fn take(s: str?) {{ s }}\n\
                 fn valid(u: User?, s: str? -> str) {{\n\
                 \x20 s = u.?profile.?name\n\
                 \x20 take(u.?profile.?name)\n\
                 \x20 assert u.?profile.?name == s\n\
                 \x20 u.?profile.?name ?? \"unknown\"\n\
                 }}\n"
            ))
            .is_empty()
        );

        let errors = errors(&format!(
            "{OPTIONAL_FIELDS}fn take(n: int?) {{ n }}\n\
             fn invalid(u: User?, n: int? -> int?) {{\n\
             \x20 n = u.?profile.?name\n\
             \x20 take(u.?profile.?name)\n\
             \x20 assert u.?profile.?name == n\n\
             \x20 u.?profile.?name\n\
             }}\n"
        ));
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(
            errors
                .iter()
                .all(|e| e.contains("`str?`") && e.contains("`int?`")),
            "{errors:?}"
        );
    }

    #[test]
    fn メソッド名はフィールドの読みとして診断しない() {
        assert!(
            errors(&format!(
                "{NESTED}impl User {{ fn shout(self -> int) {{ self.age }} }}\n\
                 fn f(u: User -> int) {{ u.shout() }}\n"
            ))
            .is_empty()
        );
    }

    // ---- 11. 演算子と bool の文脈 ----

    #[test]
    fn 整数の演算と符号反転は診断を出さない() {
        assert!(
            errors(
                "fn f(a: int, b: int -> int) { a + b * -a / (a - b) }\n\
                 fn main() { f(1, 2) }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 整数でない演算の被演算子を報告する() {
        for (src, actual) in [
            ("fn f(b: bool -> int) { b + 1 }\n", "bool"),
            ("fn f(s: str -> int) { 1 + s }\n", "str"),
            // `+` は文字列連結ではない
            ("fn f(s: str -> int) { s + s }\n", "str"),
            ("fn f(b: bool -> int) { -b }\n", "bool"),
        ] {
            let errors = errors(src);
            assert!(
                errors
                    .iter()
                    .any(|e| e.contains(&format!("`{actual}` です"))),
                "{src}: {errors:?}"
            );
        }
    }

    #[test]
    fn 演算の結果は整数として届く() {
        let e = only("fn f(r: str) { r }\nfn main(n: int) { f(n + 1) }\n");
        assert!(e.contains("`str`"), "{e}");
        assert!(e.contains("`int`"), "{e}");
    }

    #[test]
    fn 同じ型どうしの比較は診断を出さない() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(a: Rank, b: Rank -> bool) {{ a == b }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の違う比較を両辺付きで報告する() {
        let e = only(&format!(
            "{RANKS}fn f(a: Rank, b: Grade -> bool) {{ a == b }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 等価比較では非optionalからoptionalへ注入しない() {
        let e = only(&format!(
            "{RANKS}fn f(a: Rank, b: Rank? -> bool) {{ a == b }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Rank?`"), "{e}");
    }

    #[test]
    fn nilとの比較は相手のoptional性を使う() {
        for expr in ["a == nil", "nil == a"] {
            assert!(
                errors(&format!("{RANKS}fn f(a: Rank? -> bool) {{ {expr} }}\n")).is_empty(),
                "{expr}"
            );
            let e = only(&format!("{RANKS}fn f(a: Rank -> bool) {{ {expr} }}\n"));
            assert!(e.contains("`Rank`"), "{expr}: {e}");
            assert!(e.contains("`nil`"), "{expr}: {e}");
        }
        assert!(errors("fn f(-> bool) { nil == nil }\n").is_empty());
    }

    // ---- 12. optional fallback ----

    #[test]
    fn optionalと同じ中身のfallbackは中身の型になる() {
        assert!(
            errors(&format!(
                "{RANKS}fn take(r: Rank) {{ r }}\n\
                 fn f(o: Rank? -> Rank) {{\n\
                 \x20 let r = o ?? Gold\n\
                 \x20 r = o ?? Bronze\n\
                 \x20 take(o ?? Gold)\n\
                 \x20 assert (o ?? Gold) == Bronze\n\
                 \x20 o ?? Gold\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn fallbackの結果型は後続の照合へ届く() {
        let errors = errors(&format!(
            "{RANKS}fn take(g: Grade) {{ g }}\n\
             fn f(o: Rank?, g: Grade -> Grade) {{\n\
             \x20 let x = g\n\
             \x20 x = o ?? Gold\n\
             \x20 take(o ?? Gold)\n\
             \x20 assert (o ?? Gold) == g\n\
             \x20 o ?? Gold\n\
             }}\n"
        ));
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(
            errors
                .iter()
                .all(|e| e.contains("`Rank`") && e.contains("`Grade`")),
            "{errors:?}"
        );
    }

    #[test]
    fn 非optionalの左辺にはfallbackを使えない() {
        let e = only(&format!("{RANKS}fn f(r: Rank -> Rank) {{ r ?? Gold }}\n"));
        assert!(e.contains("`??` の左辺"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
    }

    #[test]
    fn fallback値はoptionalの中身と同じ非optional型を要求する() {
        for (rhs, actual) in [("Low", "Grade"), ("other", "Rank?"), ("nil", "nil")] {
            let e = only(&format!(
                "{RANKS}fn f(o: Rank?, other: Rank? -> Rank) {{ o ?? {rhs} }}\n"
            ));
            assert!(e.contains("`??` の右辺"), "{rhs}: {e}");
            assert!(e.contains("`Rank`"), "{rhs}: {e}");
            assert!(e.contains(&format!("`{actual}`")), "{rhs}: {e}");
        }
    }

    #[test]
    fn nilのfallbackは右辺から中身の型を得る() {
        assert!(
            errors(&format!(
                "{RANKS}fn take(r: Rank) {{ r }}\nfn f() {{ take(nil ?? Gold) }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の分からない左辺のfallbackは保留する() {
        assert!(
            errors(&format!(
                "{RANKS}fn unknown() {{ nil }}\n\
                 fn f(-> Grade) {{ unknown() ?? Gold }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn returnはfallbackの枝を終了できる() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(o: Rank? -> Rank) {{ o ?? return Bronze }}\n"
            ))
            .is_empty()
        );
        let e = only(&format!(
            "{RANKS}fn f(o: Rank? -> Rank) {{ o ?? return Low }}\n"
        ));
        assert!(e.contains("戻り値"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn boolの条件と表明は診断を出さない() {
        assert!(
            errors(
                "fn f(b: bool) {\n\
                 \x20 if b { 1 } else { 2 }\n\
                 \x20 while b { 1 }\n\
                 \x20 assert b\n\
                 }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn boolでない条件と表明を報告する() {
        for src in [
            "fn f(n: int) { if n { 1 } }\n",
            "fn f(n: int) { if false { 1 } elif n { 2 } }\n",
            "fn f(n: int) { while n { 1 } }\n",
            "fn f(n: int) { assert n }\n",
        ] {
            let e = only(src);
            assert!(e.contains("`bool`"), "{src}: {e}");
            assert!(e.contains("`int`"), "{src}: {e}");
        }
    }

    #[test]
    fn 型の分からない条件と表明は診断しない() {
        assert!(
            errors("fn unknown() { nil }\nfn f() { for x in unknown() { if x { assert x } } }\n")
                .is_empty()
        );
    }

    // ---- 12. 分かる代入の照合 ----

    /// 代入の検査環境。スカラー・struct・enum・optional を1つずつ持たせてある
    const FIELDS: &str = "enum Rank { Bronze Gold }\n\
                          enum Grade { Low High }\n\
                          struct Tag {}\n\
                          struct Card { n: int\ntag: Tag\nrank: Rank\nnote: Rank? }\n";

    #[test]
    fn 型の合うstruct生成のフィールド値は診断を出さない() {
        assert!(
            errors(&format!(
                "{FIELDS}fn f(o: Rank? -> Card) {{\n\
                 Card {{ n = 1, tag = Tag {{}}, rank = Gold, note = nil }}\n\
                 Card {{ n = 1, tag = Tag {{}}, rank = Gold, note = o }}\n\
                 Card {{ n = 1, tag = Tag {{}}, rank = Gold, note = Gold }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の違うstruct生成のフィールド値を報告する() {
        for (fields, actual) in [
            ("n = \"x\", tag = Tag {}, rank = Gold, note = nil", "str"),
            ("n = 1, tag = 1, rank = Gold, note = nil", "int"),
            ("n = 1, tag = Tag {}, rank = Low, note = nil", "Grade"),
            ("n = 1, tag = Tag {}, rank = nil, note = nil", "nil"),
        ] {
            let e = only(&format!(
                "{FIELDS}fn f(-> Card) {{ Card {{ {fields} }} }}\n"
            ));
            assert!(e.contains(&format!("`{actual}` を与えています")), "{e}");
        }
    }

    #[test]
    fn 型の合うフィールド代入は診断を出さない() {
        assert!(
            errors(&format!(
                "{FIELDS}fn f(c: Card) {{ c.n = 2\nc.rank = Bronze\nc.note = nil\nc.note = Gold }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の違うフィールド代入を報告する() {
        let e = only(&format!("{FIELDS}fn f(c: Card) {{ c.n = Low }}\n"));
        assert!(e.contains("`n`"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 型の合う再代入は診断を出さない() {
        assert!(
            errors(&format!(
                "{FIELDS}fn f(n: int, note: Rank?) {{\n\
                 let r = Gold\n r = Bronze\n n = 2\n note = nil\n note = Gold\n}}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の違う再代入を報告する() {
        let e = only(&format!("{FIELDS}fn f(n: int) {{ n = Low }}\n"));
        assert!(e.contains("`n` への代入"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");

        let e = only(&format!("{FIELDS}fn f() {{\n let r = Gold\n r = Low\n}}\n"));
        assert!(e.contains("`r` への代入"), "{e}");
    }

    #[test]
    fn 型の分からない初期化子の束縛は後の代入で型を得ない() {
        // 代入から遡って型を決めるには分岐の合流と到達性が要る(design.md 決定4)
        assert!(
            errors(&format!(
                "{FIELDS}fn f() {{\n let r = nil\n r = Gold\n r = Low\n}}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn nilを非optionalへ代入すると報告する() {
        let errors = errors(&format!(
            "{FIELDS}fn f(c: Card, n: int) {{\n c.n = nil\n n = nil\n}}\n"
        ));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().all(|e| e.contains("`nil`")), "{errors:?}");
    }

    // ---- 13. 配列リテラルの型付け ----

    #[test]
    fn 同じ型の要素からなる配列は型が分かる() {
        assert!(
            errors(
                "fn take(xs: [int]) { xs }\n\
                 fn main() { take([1, 2, 3]) }\n",
            )
            .is_empty()
        );
        // 推論した配列型は束縛を越えて既存の照合へ届く
        let e = only("fn take(xs: [str]) { xs }\nfn main() { let xs = [1, 2]\ntake(xs) }\n");
        assert!(e.contains("`[str]`"), "{e}");
        assert!(e.contains("`[int]`"), "{e}");
    }

    #[test]
    fn 型の食い違う要素を位置付きで報告する() {
        let e = only("fn main() { let xs = [1, true, 2]\nxs }\n");
        assert!(e.contains("配列の第 2 要素"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`bool`"), "{e}");
    }

    #[test]
    fn 型の分からない要素を含む配列は型を持たない() {
        // 空配列も、`nil` しか無い配列も、後の使用から遡って型を得ない
        assert!(
            errors(
                "fn take(n: int) { n }\n\
                 fn main() {\n\
                 \x20 let empty = []\n\
                 \x20 let nils = [nil, nil]\n\
                 \x20 take(empty)\n\
                 \x20 take(nils)\n\
                 }\n",
            )
            .is_empty()
        );
    }

    /// 配列の検査環境。要素が enum・optional・入れ子のフィールドを1つずつ持つ
    const ARRAYS: &str = "enum Rank { Bronze Gold }\n\
                          enum Grade { Low High }\n\
                          struct User { rank: Rank }\n\
                          struct Store { users: [User]\nnotes: [Rank?]\ngrid: [[int]] }\n";

    #[test]
    fn 期待する配列型のある位置では要素をその型と照合する() {
        assert!(
            errors(&format!(
                "{ARRAYS}fn f(u: User -> Store) {{\n\
                 \x20 Store {{ users = [], notes = [Gold, nil], grid = [[1], []] }}\n\
                 \x20 Store {{ users = [u], notes = [], grid = [] }}\n\
                 }}\n"
            ))
            .is_empty(),
            "空配列はどの期待配列型にも収まり、要素へは既存の nil と optional 注入が効く"
        );
    }

    #[test]
    fn 期待する要素型と食い違う要素を報告する() {
        let e = only(&format!(
            "{ARRAYS}fn f(-> Store) {{ Store {{ users = [], notes = [Low], grid = [] }} }}\n"
        ));
        assert!(e.contains("配列の第 1 要素"), "{e}");
        assert!(e.contains("`Rank?`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");

        // 入れ子でも診断は一番内側の要素に一度だけ出る
        let e = only(&format!(
            "{ARRAYS}fn f(-> Store) {{ Store {{ users = [], notes = [], grid = [[1], [true]] }} }}\n"
        ));
        assert!(e.contains("配列の第 1 要素"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`bool`"), "{e}");
    }

    #[test]
    fn 配列の値は要素型について不変() {
        assert!(
            errors(&format!(
                "{ARRAYS}fn take(xs: [User]?) {{ xs }}\nfn main(xs: [User]) {{ take(xs) }}\n"
            ))
            .is_empty(),
            "外側の optional への注入は既存の規則どおり通る"
        );

        let e = only(&format!(
            "{ARRAYS}fn take(xs: [User?]) {{ xs }}\nfn main(xs: [User]) {{ take(xs) }}\n"
        ));
        assert!(e.contains("`[User?]`"), "{e}");
        assert!(e.contains("`[User]`"), "{e}");
    }

    // ---- 14. for の要素型 ----

    #[test]
    fn ループ変数は配列の要素型を得る() {
        assert!(
            errors(&format!(
                "{ARRAYS}fn f(s: Store) {{ for u in s.users {{ u.rank = Gold }} }}\n"
            ))
            .is_empty()
        );

        let e = only(&format!(
            "{ARRAYS}fn f(s: Store) {{ for u in s.users {{ u.rank = Low }} }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");

        let e = only(&format!(
            "{ARRAYS}fn f(s: Store) {{ for u in s.users {{ u.nope }} }}\n"
        ));
        assert!(e.contains("`User`"), "{e}");
        assert!(e.contains("`nope`"), "{e}");
    }

    #[test]
    fn 配列でない反復対象を報告する() {
        let e = only(&format!("{ARRAYS}fn f(n: int) {{ for x in n {{ x }} }}\n"));
        assert!(e.contains("`for` の反復対象は配列"), "{e}");
        assert!(e.contains("`int`"), "{e}");
    }

    #[test]
    fn optionalな配列の反復を報告する() {
        let e = only(&format!(
            "{ARRAYS}fn f(xs: [User]?) {{ for u in xs {{ u }} }}\n"
        ));
        assert!(e.contains("optional な配列 `[User]?`"), "{e}");
    }

    // ---- 15. trait 実装の契約 ----

    /// 契約の検査環境。self を取るものと取らないものを1つずつ持たせてある
    const CONTRACT: &str = "struct User { id: int }\n\
                            trait Database {\n\
                            \x20 fn find(self, id: int -> User?)\n\
                            \x20 fn empty(-> User?)\n\
                            }\n\
                            struct Store {}\n";

    #[test]
    fn 宣言どおりのtrait実装は診断を出さない() {
        assert!(
            errors(&format!(
                "{CONTRACT}impl Database for Store {{\n\
                 \x20 fn find(self, other: int -> User?) {{ nil }}\n\
                 \x20 fn empty(-> User?) {{ nil }}\n\
                 }}\n"
            ))
            .is_empty(),
            "引数名は実装側の局所名なので契約の同一性に入らない"
        );
    }

    #[test]
    fn 実装し忘れたtraitメソッドを報告する() {
        let e = only(&format!(
            "{CONTRACT}impl Database for Store {{ fn find(self, id: int -> User?) {{ nil }} }}\n"
        ));
        assert!(e.starts_with("impl Database for Store: "), "{e}");
        assert!(e.contains("`empty`"), "{e}");
        assert!(!e.contains("`find`"), "{e}");
    }

    #[test]
    fn 宣言に無い実装メソッドと二度の実装を報告する() {
        let e = only(&format!(
            "{CONTRACT}impl Database for Store {{\n\
             \x20 fn find(self, id: int -> User?) {{ nil }}\n\
             \x20 fn empty(-> User?) {{ nil }}\n\
             \x20 fn extra(self) {{ 1 }}\n\
             }}\n"
        ));
        assert!(e.contains("`extra`"), "{e}");
        assert!(e.contains("`Database` に宣言されていません"), "{e}");

        let e = only(&format!(
            "{CONTRACT}impl Database for Store {{\n\
             \x20 fn find(self, id: int -> User?) {{ nil }}\n\
             \x20 fn find(self, id: int -> User?) {{ nil }}\n\
             \x20 fn empty(-> User?) {{ nil }}\n\
             }}\n"
        ));
        assert!(e.contains("`find` を二度実装しています"), "{e}");
    }

    #[test]
    fn 契約と食い違う実装署名を報告する() {
        for (method, expected) in [
            ("fn find(id: int -> User?) { nil }", "レシーバの形"),
            ("fn find(self, id: str -> User?) { nil }", "引数は (`int`)"),
            ("fn find(self -> User?) { nil }", "引数は (`int`)"),
            ("fn find(self, id: int -> User) { nil }", "戻り値は `User?`"),
            ("fn find(self, id: int) { nil }", "戻り値は `User?`"),
        ] {
            // 本体の検査は宣言した署名に対して従来どおり走るので、契約の
            // 診断が出ていることだけを見る
            let errors = errors(&format!(
                "{CONTRACT}impl Database for Store {{\n {method}\n fn empty(-> User?) {{ nil }}\n}}\n"
            ));
            assert!(
                errors.iter().any(|e| e.contains(expected)),
                "{method}: {errors:?}"
            );
        }
    }

    #[test]
    fn traitでない実装対象とstructでない実装先を報告する() {
        let e = only(&format!("{CONTRACT}impl User for Store {{}}\n"));
        assert!(e.contains("`User` は trait ではありません"), "{e}");

        let e = only(&format!("{CONTRACT}impl Database for User {{}}\n"));
        assert!(
            e.contains("`Database` のメソッド"),
            "struct なら中身の検査へ進む: {e}"
        );

        let e = only(&format!(
            "{CONTRACT}enum Rank {{ Bronze }}\nimpl Database for Rank {{}}\n"
        ));
        assert!(e.contains("`Rank` は struct ではありません"), "{e}");
    }

    // ---- 16. 呼び出しの解決 ----

    /// 解決の検査環境。同名メソッドを持つ2つの trait と、スロットを1つ持つ
    const CALLS: &str = "struct User { id: int }\n\
                         trait Database { fn find(self, id: int -> User?) }\n\
                         trait Cache { fn find(self, id: int -> User?) }\n\
                         effect db: Database\n\
                         struct Store {}\n\
                         impl Store {\n\
                         \x20 fn new(-> Store) { Store {} }\n\
                         \x20 fn count(self -> int) { 1 }\n\
                         }\n\
                         impl Database for Store { fn find(self, id: int -> User?) { nil } }\n";

    #[test]
    fn 一意な候補は具体型から解決される() {
        assert!(
            errors(&format!(
                "{CALLS}fn f(s: Store -> int) {{\n\
                 \x20 let ignored = s.find(1)\n\
                 \x20 let made = Store::new()\n\
                 \x20 made.count()\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 同名のtraitメソッドが複数あると曖昧として報告する() {
        let e = only(&format!(
            "{CALLS}impl Cache for Store {{ fn find(self, id: int -> User?) {{ nil }} }}\n\
             fn f(s: Store) {{ s.find(1) }}\n"
        ));
        assert!(e.contains("どの trait のものか決まりません"), "{e}");
        assert!(e.contains("`find`"), "{e}");
    }

    #[test]
    fn スロット経由の呼び出しは宣言traitだけを見る() {
        // 具体型に同名の別 trait 実装があっても、スロットは契約で一意に決まる
        assert!(
            errors(&format!(
                "{CALLS}impl Cache for Store {{ fn find(self, id: int -> User?) {{ nil }} }}\n\
                 fn f(-> User?) {{ db.find(1) }}\n"
            ))
            .is_empty()
        );

        let e = only(&format!("{CALLS}fn f() {{ db.save(1) }}\n"));
        assert!(e.contains("`Database` に `save` はありません"), "{e}");
    }

    #[test]
    fn 同名のローカルはスロットを隠す() {
        // `db` がローカルなら契約ではなく具体型から引く
        let e = only(&format!("{CALLS}fn f(db: Store) {{ db.nope(1) }}\n"));
        assert!(e.contains("`Store` に `nope` はありません"), "{e}");
    }

    #[test]
    fn withの提供値は外側で本体は内側で検査する() {
        // 提供値の `store` はまだローカル、本体の `db` はスロット
        assert!(
            errors(&format!(
                "{CALLS}fn f(store: Store) {{ with db(store) {{ db.find(1) }} }}\n"
            ))
            .is_empty()
        );

        let e = only(&format!(
            "{CALLS}fn f(db: Store) {{ with db(db) {{ db.save(1) }} }}\n"
        ));
        assert!(e.contains("`Database` に `save` はありません"), "{e}");
    }

    #[test]
    fn 型の分からないレシーバの呼び出しは保留する() {
        assert!(
            errors(&format!(
                "{CALLS}fn unknown() {{ nil }}\n\
                 fn f() {{\n\
                 \x20 let u = unknown()\n\
                 \x20 u.nope(1)\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 呼び出し構文とレシーバの形の食い違いを報告する() {
        let e = only(&format!("{CALLS}fn f(s: Store -> Store) {{ s.new() }}\n"));
        assert!(e.contains("self を取りません"), "{e}");

        let e = only(&format!(
            "{CALLS}fn f(s: Store -> int) {{ Store::count() }}\n"
        ));
        assert!(e.contains("レシーバが必要です"), "{e}");
    }

    // ---- 17. 解決した呼び出しの署名 ----

    #[test]
    fn 解決した呼び出しの引数の個数と型を検査する() {
        let e = only(&format!("{CALLS}fn f(s: Store) {{ s.find() }}\n"));
        assert!(e.contains("`find` は引数を 1 個取ります"), "{e}");

        let e = only(&format!("{CALLS}fn f(s: Store) {{ s.find(\"x\") }}\n"));
        assert!(e.contains("`find` の第 1 引数"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`str`"), "{e}");

        let e = only(&format!("{CALLS}fn f() {{ Store::new(1) }}\n"));
        assert!(e.contains("`Store::new` は引数を 0 個取ります"), "{e}");
    }

    #[test]
    fn 解決した呼び出しの引数にも既存のoptional規則が効く() {
        assert!(
            errors(&format!(
                "{CALLS}impl Store {{ fn take(self, u: User?) {{ u }} }}\n\
                 fn f(s: Store, u: User) {{\n\
                 \x20 s.take(u)\n\
                 \x20 s.take(nil)\n\
                 }}\n"
            ))
            .is_empty(),
            "`T` から `T?` への注入は宛先の規則どおり通る"
        );

        let e = only(&format!(
            "{CALLS}impl Store {{ fn take(self, u: User) {{ u }} }}\n\
             fn f(s: Store, u: User?) {{ s.take(u) }}\n"
        ));
        assert!(e.contains("`User?`"), "{e}");
    }

    #[test]
    fn 解決した呼び出しの戻り値は後続の検査へ届く() {
        // optional fallback・フィールドの読み・宣言戻り値の照合まで一続き
        assert!(
            errors(&format!(
                "{CALLS}fn f(s: Store -> int) {{ (s.find(1) ?? return 0).id }}\n"
            ))
            .is_empty()
        );

        let e = only(&format!(
            "{CALLS}fn f(s: Store -> str) {{ (s.find(1) ?? return \"x\").id }}\n"
        ));
        assert!(e.contains("`str`"), "{e}");
        assert!(e.contains("`int`"), "{e}");

        let e = only(&format!("{CALLS}fn f(s: Store -> int) {{ s.find(1) }}\n"));
        assert!(e.contains("`User?`"), "{e}");
    }

    #[test]
    fn 戻り値型の無いメソッドの結果は分からないまま() {
        assert!(
            errors(&format!(
                "{CALLS}impl Store {{ fn nothing(self) {{ 1 }} }}\n\
                 fn f(s: Store -> int) {{ s.nothing() }}\n"
            ))
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

    /// 正典の `for u in self.users` が静的に検査されていること。
    /// `users: [User]` を宣言した効果は、ループ本体の誤りが実行前に出ることで見える
    #[test]
    fn 正典のループ本体は要素型で検査される() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let broken = src.replace("if u.id == id", "if u.nope == id");
        assert_ne!(broken, src, "正典のループ本体が変わったらここも直す");

        let e = only(&broken);
        assert!(e.contains("`User`"), "{e}");
        assert!(e.contains("`nope`"), "{e}");
    }
}
