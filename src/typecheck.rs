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
//!   - 全ての関数・trait メンバー・実装メソッドは実効戻り値型を持つ。注釈が
//!     あればそれ、無ければ `unit`。**型の分かる**明示 `return` と最後の式は
//!     その型と適合する(注釈を省略して `unit` 以外を返すのはエラー)
//!   - 配列リテラルの要素は、期待要素型があればそれと、無ければ互いに適合する
//!   - `for` の反復対象は非 optional な配列で、ループ変数は要素型を持つ
//!   - `with` の提供は、**型の分かる限り**スロットの trait を実装した具体型
//!   - 限定 variant の呼び出しは宣言 payload と同じ個数の引数を持ち、
//!     **型の分かる**引数は対応する payload 型と適合する。payload を持つ
//!     variant は呼び出さない限り値にならない
//!   - `match` の arm は宣言 payload と同じ個数の pattern 要素を持ち、
//!     一つの pattern が同じ名前を二度束縛しない
//!   - `match` の catch-all `_` は一度だけ、最後の arm として現れる。
//!     あれば残りの variant を全部受けるので網羅的になる
//!
//! 型の同一性は形と後置 `?` の一致だけ(nominal)。配列は要素型まで含めて
//! 一致しないと同じ型ではない。期待型のある宛先では
//! `T` を同名の `T?` へ注入できるが、式の推論型と等価比較は変えない。
//! `nil` は期待される `T?` の文脈でだけ適合し、`T? ?? T` は `T` を返す。
//! `S?.?field` は宣言 field の型に optional を付けて返す。
//!
//! 呼び出しの解決は評価器と同じ順序で、宣言済み enum の限定 variant を
//! constructor として最初に見てから、スロット経由なら宣言 trait の契約だけ、
//! 具体型なら inherent と trait 実装をまとめて名前で絞り一意を要求する。
//! レシーバの型が分からない呼び出しは保留する。`with slot<T>` は型名が
//! そのまま分かるので常に、`with slot(v)` は `v` の型が分かるときだけ契約と
//! 突き合わせ、分からない提供は eval 側の同じ判定が実行時に止める。
//! 型の分からない式には診断を出さず、後続の change が一つずつ潰していく。
//!
//! 走査は式ごとに `Outcome` を返す1本(`walk`)で、型の推論と診断を同時に行う。
//! 宛先の型がある位置では期待型を内側の文脈依存な式(`nil`・配列リテラル・
//! `match` の arm・条件式の枝・ブロックの最後の式)へ配り、境界の照合は
//! `walk` の出口1箇所に閉じる(design.md 決定4)。値を産まない式は
//! `Diverges` として型と言い分けるので、`return` する枝に型を捏造しない。
//!
//! 走査は `requirement::scan` と同じ字句スコープ規則を持つが、運ぶ状態が
//! 違う(あちらは提供集合、こちらはローカル名と分かっている型)ので別に書いている。

use crate::ast::{
    BinOp, Expr, ExprKind, Head, Item, MatchArm, MatchPattern, PatternBinding, Program, Provision,
    Sig, Type, TypeKind, UnOp,
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
    /// variant の正準名 → その variant を作る constructor の署名。payload 型を
    /// そのまま引数、所属 enum を戻り値にしてあるので、個数・引数型・結果型の
    /// 検査が既存の呼び出し1本に乗る(design.md 決定4)。fieldless は引数0個
    ctors: BTreeMap<String, FnSig>,
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
///
/// 戻り値型は常に一つ決まる。注釈があればそれ、無ければ `unit`
/// (design.md 決定1)。「戻り値型が分からない呼び出し」は存在しない
#[derive(PartialEq, Eq)]
struct FnSig {
    has_self: bool,
    params: Vec<KnownType>,
    ret: KnownType,
}

/// 宣言された署名を検査用の形にする。引数名は実装側の局所名なので落とす。
fn signature(sig: &Sig) -> FnSig {
    FnSig {
        has_self: sig.has_self,
        params: sig.params.iter().map(|p| known(&p.ty)).collect(),
        ret: effective_ret(sig),
    }
}

/// 宣言の実効戻り値型。注釈の省略は `unit` を返す宣言と同じ意味で、
/// 本体から推論することはしない(design.md 決定1)。
fn effective_ret(sig: &Sig) -> KnownType {
    sig.ret.as_ref().map_or_else(|| plain("unit"), known)
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
/// それを刻む。各 `push` は文言だけを渡すままでよく、位置の管理は走査側の1本
/// (`walk` が式ごとに差し替える)に閉じる。
struct Out {
    diagnostics: Vec<Diag>,
    /// 走査に入る前だけ `None`。以降は必ず何かを指している
    span: Option<Span>,
    /// 診断を伴わずに型を出せなかった最初の式。ここが `Some` のまま診断が
    /// 空で終わるのは検査規則の抜けなので、`check` が最後に診断へ変える
    /// (design.md 決定3)
    unexplained: Option<Span>,
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

    /// いまの診断の件数。部分木を走査する前に取っておく
    fn count(&self) -> usize {
        self.diagnostics.len()
    }

    /// 子の走査が診断を1件も増やさなかったなら、型が出なかった理由をここで
    /// 位置付きで説明する。既に説明済みなら重ねない(design.md 決定3)。
    ///
    /// 「診断が増えたか」を唯一の判定にすることで、子が自分で説明するように
    /// なった時点で親の重複が自動的に消える。
    fn explain(&mut self, before: usize, msg: String) {
        if self.count() == before {
            self.push(msg);
        }
    }

    /// 型を出せなかった式を覚えておく。部分木が理由を説明していれば
    /// (走査で診断が増えていれば)覚えない。
    ///
    /// 検査が成功しかけているときだけ意味があるので、既に診断が1件でも
    /// あれば何もしない。「最初の1件」だけ残すのは、根本原因に近い位置を
    /// 指すため。
    fn note_unexplained(&mut self, before: usize, span: Span) {
        if self.count() == before && self.diagnostics.is_empty() && self.unexplained.is_none() {
            self.unexplained = Some(span);
        }
    }
}

/// 診断を全件返す。空なら struct の形と分かる enum 型は正しい。
pub fn check(program: &Program) -> Vec<Diag> {
    let mut out = Out {
        diagnostics: Vec::new(),
        span: None,
        unexplained: None,
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
                check_body(body, &decls, locals, &sig.name, &effective_ret(sig), out);
            }
            // test は呼び出されないので署名を持たないが、本体の最後の式と
            // `return` はどこかの型と照合されなければ検査が閉じない。
            // 値を返す先が無いので `unit` を実効戻り値にする
            Item::Test { name, body, .. } => {
                let ctx = format!("test \"{name}\"");
                check_body(body, &decls, Locals::new(), &ctx, &plain("unit"), out);
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
                    out.span = Some(sig.span);
                    check_body(body, &decls, locals, &ctx, &effective_ret(sig), out);
                }
            }
            _ => {}
        }
    }

    // 診断が空なのに型の出なかった式が残っているのは検査規則の抜け。
    // 後続段へ「全ての式が型を持つ」と渡せないので、その式を指して落とす
    // (design.md 決定3)
    if let Some(span) = out.unexplained
        && out.diagnostics.is_empty()
    {
        out.push_at(
            span,
            "この式の型が決まりません(型検査の規則が足りていません)".to_string(),
        );
    }

    std::mem::take(&mut out.diagnostics)
}

fn collect(program: &Program, out: &mut Out) -> Decls {
    let mut structs = BTreeMap::new();
    let mut variants = BTreeMap::new();
    let mut enums = BTreeMap::new();
    let mut ctors = BTreeMap::new();
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
                    variants.insert(variant.name.clone(), name.clone());
                    others.insert(variant.name.clone());
                    ctors.insert(
                        variant.name.clone(),
                        FnSig {
                            has_self: false,
                            params: variant.payload.iter().map(known).collect(),
                            ret: plain(name),
                        },
                    );
                }
                enums.insert(name.clone(), vs.iter().map(|v| v.name.clone()).collect());
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
        ctors,
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

/// 診断に出す戻り値型。省略した注釈は `unit` として出る。
fn returned(ty: &KnownType) -> String {
    format!("`{ty}`")
}

// ---------------------------------------------------------------------------
// 検査結果と期待型
// ---------------------------------------------------------------------------

/// 式の検査結果(design.md 決定3)。
///
/// `Diverges` はユーザーに見える型ではない。制御がそこから続かないので、
/// どんな期待型の位置にも収まる。`Poisoned` は具体型を出せなかった式で、
/// 親は型を捏造せず、既に説明済みの失敗へ診断を重ねない。
///
/// 検査が成功した(診断が空の)プログラムでは、値を産む式はすべて `Typed`
/// でなければならない。まだ黙って `Poisoned` を返す経路は後続の change が
/// 一つずつ位置付きの診断へ変えていく。
#[derive(Clone)]
enum Outcome {
    Typed(KnownType),
    Diverges,
    Poisoned,
}

impl Outcome {
    /// 具体型を持つならそれ。`Diverges` と `Poisoned` は `None`
    fn ty(&self) -> Option<&KnownType> {
        match self {
            Outcome::Typed(ty) => Some(ty),
            Outcome::Diverges | Outcome::Poisoned => None,
        }
    }
}

/// `unit` を産む式。`let` / 代入 / `assert` / ループ / 空ブロックの結果。
fn unit() -> Outcome {
    Outcome::Typed(plain("unit"))
}

/// 宛先の型へ値の型が収まるか。厳密一致か、同名の非 optional から optional への
/// 一方向の注入だけ(design.md 決定1)。値の側の推論型は変えない。
fn fits(actual: &KnownType, expected: &KnownType) -> bool {
    actual == expected || (actual.kind == expected.kind && !actual.optional && expected.optional)
}

/// 期待型のある位置。どこが期待しているかで診断の文言だけが変わる。
enum Site<'a> {
    /// 「{what}は `{expected}` ですが、`{actual}` です」
    What(&'a str),
    /// 呼び出しの実引数
    Arg { callee: &'a str, index: usize },
    /// struct リテラルのフィールド値と、フィールドへの代入
    Field { type_name: &'a str, field: &'a str },
    /// 明示 `return` と本体の最後の式
    Return,
}

impl Site<'_> {
    fn message(&self, ctx: &str, expected: &KnownType, actual: &str) -> String {
        match self {
            Site::What(what) => format!("{ctx}: {what}は `{expected}` ですが、`{actual}` です"),
            Site::Arg { callee, index } => format!(
                "{ctx}: `{callee}` の第 {} 引数は `{expected}` ですが、`{actual}` を渡しています",
                index + 1
            ),
            Site::Field { type_name, field } => format!(
                "{ctx}: `{type_name}` のフィールド `{field}` は `{expected}` ですが、`{actual}` を与えています"
            ),
            Site::Return => {
                format!("{ctx}: 戻り値は `{expected}` ですが、`{actual}` を返しています")
            }
        }
    }
}

/// 宛先の型と、合わなかったときの文言。`None` は期待型の無い位置(推論だけ)。
type Expect<'a> = Option<(&'a KnownType, &'a Site<'a>)>;

/// 期待型だけを取り出す。文言が要るのは境界の照合1箇所だけ。
fn want<'a>(expected: Expect<'a>) -> Option<&'a KnownType> {
    expected.map(|(ty, _)| ty)
}

/// 走査の間ずっと同じもの。式ごとに変わるのは `locals` と期待型だけなので、
/// 引数の列から追い出しておく。
struct Cx<'a> {
    decls: &'a Decls,
    /// 診断の頭に付く「どの宣言か」
    ctx: &'a str,
    /// いま検査している宣言の実効戻り値型
    ret: &'a KnownType,
}

// ---------------------------------------------------------------------------
// 走査
// ---------------------------------------------------------------------------

fn check_body(
    body: &[Expr],
    decls: &Decls,
    mut locals: Locals,
    ctx: &str,
    ret: &KnownType,
    out: &mut Out,
) {
    let cx = Cx { decls, ctx, ret };
    // 本体は値ベースなので最後の式も戻り値。明示 `return` と同じ位置で照合する
    block(body, Some((ret, &Site::Return)), &cx, &mut locals, out);
}

/// 式の列。値になるのは最後の式だけで、手前の式は値を産んでも捨てる
/// (design.md 決定5)。空ブロックは値を産まないので `unit`。
fn block(body: &[Expr], expected: Expect, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Outcome {
    let Some((last, init)) = body.split_last() else {
        return unit();
    };
    // 手前の式のどれかが制御を抜けるなら、この列は最後まで続かない。
    // ただし到達しない式も宣言の検査対象なので走査は続ける
    let mut diverged = false;
    for e in init {
        if matches!(synth(e, cx, locals, out), Outcome::Diverges) {
            diverged = true;
        }
    }
    let result = walk(last, expected, cx, locals, out);
    if diverged { Outcome::Diverges } else { result }
}

/// 期待型の無い位置の式。自分の型を推論するだけ。
fn synth(e: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Outcome {
    walk(e, None, cx, locals, out)
}

/// 検査中の式を診断の位置にして、種類ごとの規則へ回す。部分木から戻ったら
/// 外側の式へ span を戻す(戻さないと、子を見た後の親の診断が子の位置を指す)。
///
/// 最後に期待型との境界を1箇所で照合する。`Diverges` はそこを通らないので
/// 照合せず、`Poisoned` は既に説明済みなので重ねない(design.md 決定3・4)。
fn walk(e: &Expr, expected: Expect, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Outcome {
    let before = out.count();
    let outer = out.span.replace(e.span);
    let outcome = walk_kind(e, expected, cx, locals, out);
    out.span = outer;
    let outcome = match (&outcome, expected) {
        (Outcome::Typed(actual), Some((expected, site))) if !fits(actual, expected) => {
            out.push_at(e.span, site.message(cx.ctx, expected, &actual.to_string()));
            Outcome::Poisoned
        }
        _ => outcome,
    };
    // 検査を閉じる網。型を出せなかった式は理由の診断を伴っていなければならず、
    // 伴わないまま検査が成功しかけたら `check` が最後にここを指して落とす
    // (design.md 決定3、リスク「規則の抜けが Unknown を再生する」)
    if matches!(outcome, Outcome::Poisoned) {
        out.note_unexplained(before, e.span);
    }
    outcome
}

fn walk_kind(e: &Expr, expected: Expect, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Outcome {
    let decls = cx.decls;
    let ctx = cx.ctx;
    match &e.kind {
        ExprKind::Int(_) => Outcome::Typed(plain("int")),
        ExprKind::Str(_) => Outcome::Typed(plain("str")),
        ExprKind::Bool(_) => Outcome::Typed(plain("bool")),

        // `nil` は自分だけでは nominal 型を持たない。期待型が optional の
        // ときだけその型になる(design.md 決定7)
        ExprKind::Nil => match expected {
            Some((expected, _)) if expected.optional => Outcome::Typed(expected.clone()),
            Some((expected, site)) => {
                out.push_at(e.span, site.message(ctx, expected, "nil"));
                Outcome::Poisoned
            }
            None => Outcome::Poisoned,
        },

        ExprKind::Ident(name) => {
            let before = out.count();
            check_bare(name, decls, locals, ctx, out);
            match locals.get(name) {
                Some(Binding::Value(Some(ty))) => Outcome::Typed(ty.clone()),
                // 型の分からないローカル。束縛した側が既に診断している
                Some(Binding::Value(None)) => Outcome::Poisoned,
                // スロットは trait の窓であって値ではない
                Some(Binding::Slot(_)) => {
                    out.push(format!("{ctx}: `{name}` はスロットなので値になりません"));
                    Outcome::Poisoned
                }
                None => {
                    let outcome = bare_value(name, decls);
                    if matches!(outcome, Outcome::Poisoned) {
                        out.explain(before, format!("{ctx}: `{name}` は値として読めません"));
                    }
                    outcome
                }
            }
        }

        // 宣言済み enum を修飾した path は fieldless variant の値。payload を持つ
        // variant は限定 path の呼び出しでしか作れず、裸の path は値でも
        // first-class constructor でもない(design.md 決定1)。それ以外の path は
        // 関連関数や ambient の型射影なので、値にはならない
        ExprKind::Path(parts) => {
            let [enum_name, variant] = parts.as_slice() else {
                out.push(format!(
                    "{ctx}: `{}` は値として読めません",
                    parts.join("::")
                ));
                return Outcome::Poisoned;
            };
            if !decls.enums.contains_key(enum_name) {
                out.push(format!(
                    "{ctx}: `{enum_name}::{variant}` は値として読めません"
                ));
                return Outcome::Poisoned;
            }
            match payload_of(enum_name, variant, decls) {
                None => {
                    out.push(format!(
                        "{ctx}: `{enum_name}::{variant}` は `{enum_name}` の variant ではありません"
                    ));
                    Outcome::Poisoned
                }
                Some(payload) if !payload.is_empty() => {
                    out.push(format!(
                        "{ctx}: `{enum_name}::{variant}` は payload を {} 個取ります。`{enum_name}::{variant}(...)` で生成してください",
                        payload.len()
                    ));
                    Outcome::Poisoned
                }
                Some(_) => Outcome::Typed(plain(enum_name)),
            }
        }

        ExprKind::Field(recv, field) => field_read(recv, field, false, cx, locals, out),
        ExprKind::OptionalField(recv, field) => field_read(recv, field, true, cx, locals, out),

        ExprKind::Call(callee, args) => call(callee, args, cx, locals, out),

        ExprKind::StructLit { name, fields } => {
            check_literal(name, fields, decls, ctx, out);
            for (field, value) in fields {
                match declared_field(name, field, decls) {
                    Some(declared) => {
                        let site = Site::Field {
                            type_name: name,
                            field,
                        };
                        walk(value, Some((&declared, &site)), cx, locals, out);
                    }
                    // 宣言に無いフィールドは `check_literal` が報告済み
                    None => {
                        synth(value, cx, locals, out);
                    }
                }
            }
            Outcome::Typed(plain(name))
        }

        // 注釈があれば初期化子の期待型になり、そのまま束縛の型として固定される。
        // 無ければ初期化子の推論型だけが束縛の型(design.md 決定2)
        ExprKind::Let {
            name,
            annotation,
            value,
        } => {
            let ty = match annotation.as_ref().map(known) {
                Some(declared) => {
                    let what = format!("`{name}` の初期化子");
                    walk(
                        value,
                        Some((&declared, &Site::What(&what))),
                        cx,
                        locals,
                        out,
                    );
                    Some(declared)
                }
                None => {
                    let before = out.count();
                    match synth(value, cx, locals, out) {
                        Outcome::Typed(ty) => Some(ty),
                        // 初期化子だけでは型が決まらない束縛は型無しで作らない。
                        // 期待型を与えられる唯一の場所が注釈(design.md 決定2)
                        Outcome::Poisoned => {
                            out.explain(
                                before,
                                format!(
                                    "{ctx}: `{name}` の型が初期化子から決まりません。`let {name}: T = ...` と注釈してください"
                                ),
                            );
                            None
                        }
                        Outcome::Diverges => None,
                    }
                }
            };
            locals.insert(name.clone(), Binding::Value(ty));
            unit()
        }

        ExprKind::Assign { target, value } => {
            assign(target, value, cx, locals, out);
            unit()
        }

        ExprKind::Unary(UnOp::Neg, inner) => {
            let int = plain("int");
            let site = Site::What("単項 `-` の被演算子");
            walk(inner, Some((&int, &site)), cx, locals, out);
            Outcome::Typed(int)
        }

        ExprKind::Binary { op, lhs, rhs } => match op {
            // 評価器の整数演算をそのまま静的にする。暗黙変換も文字列連結も無い
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                let int = plain("int");
                let sym = symbol(*op);
                let left = Site::What(&format!("`{sym}` の左辺"));
                walk(lhs, Some((&int, &left)), cx, locals, out);
                let right = Site::What(&format!("`{sym}` の右辺"));
                walk(rhs, Some((&int, &right)), cx, locals, out);
                Outcome::Typed(int)
            }
            BinOp::Eq => equality(lhs, rhs, cx, locals, out),
            BinOp::Coalesce => coalesce(lhs, rhs, cx, locals, out),
        },

        // 値を運ぶかに関わらず制御を抜ける。運ぶ値は宣言の実効戻り値と照合する
        ExprKind::Return(value) => {
            if let Some(value) = value {
                walk(value, Some((cx.ret, &Site::Return)), cx, locals, out);
            }
            Outcome::Diverges
        }

        ExprKind::Assert(inner) => {
            let site = Site::What("`assert` の対象");
            walk(inner, Some((&plain("bool"), &site)), cx, locals, out);
            unit()
        }

        // 期待要素型があればそれを各要素へ配る。無ければ最初に型の分かった要素を
        // 以降の要素の期待型にする(design.md 決定4)
        ExprKind::Array(items) => {
            let contextual = want(expected).and_then(KnownType::element).cloned();
            let mut element = contextual.clone();
            let mut poisoned = false;
            for (n, item) in items.iter().enumerate() {
                let what = format!("配列の第 {} 要素", n + 1);
                let site = Site::What(&what);
                let outcome = match &element {
                    Some(element) => walk(item, Some((element, &site)), cx, locals, out),
                    None => synth(item, cx, locals, out),
                };
                match outcome {
                    Outcome::Typed(ty) if element.is_none() => element = Some(ty),
                    Outcome::Typed(_) | Outcome::Diverges => {}
                    Outcome::Poisoned => poisoned = true,
                }
            }
            match (want(expected), &contextual, element) {
                // 期待型のある位置は生成の境界。要素は個別に照合済みなので、
                // 配列全体はその期待型として適合する
                (Some(expected), Some(_), _) => Outcome::Typed(expected.clone()),
                // 型の分かる要素が一つも無い。空配列も、`nil` だけの配列もここ。
                // 後の使用から遡らず、この場で要素型を要求する
                (_, _, None) => {
                    out.push(format!(
                        "{ctx}: 配列の要素型が決まりません。宛先の型か `let xs: [T] = ...` の注釈で要素型を与えてください"
                    ));
                    Outcome::Poisoned
                }
                // どれかの要素が基準と食い違った。理由はその要素の位置にある
                (_, _, Some(_)) if poisoned => Outcome::Poisoned,
                (_, _, Some(element)) => Outcome::Typed(array_of(element)),
            }
        }

        // 第二級ブロックなので、内側の `let` は外へ漏れる(`requirement::scan`
        // と同じ規則)。枝へ入るときだけ `locals` を複製する
        ExprKind::Block(body) => block(body, expected, cx, locals, out),

        // 対象は既知の非 optional な enum で、arm はその全 variant を一度ずつ。
        // 結果型は期待型、無ければ最初に型の分かった arm を基準にする
        // (design.md 決定3・5)
        ExprKind::Match { subject, arms } => match_expr(expected, subject, arms, cx, locals, out),

        ExprKind::Head { head, body, orelse } => {
            head_expr(expected, head, body, orelse.as_deref(), cx, locals, out)
        }
    }
}

/// ローカルに隠されていない裸の名前の値。フィールド0個の struct と payload 0個の
/// enum variant だけが名前そのままで値になる(`check_bare` が診断する側)。
fn bare_value(name: &str, decls: &Decls) -> Outcome {
    if let Some(enum_name) = decls.variants.get(name) {
        return if decls.ctors[name].params.is_empty() {
            Outcome::Typed(plain(enum_name))
        } else {
            Outcome::Poisoned
        };
    }
    match decls.structs.get(name) {
        Some(fields) if fields.is_empty() => Outcome::Typed(plain(name)),
        _ => Outcome::Poisoned,
    }
}

/// 普通のフィールドの読みと optional field access。レシーバに要求する optional
/// 性と結果の optional bit だけが違うので1本にまとめる。
fn field_read(
    recv: &Expr,
    field: &str,
    optional: bool,
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> Outcome {
    let ctx = cx.ctx;
    let ty = match synth(recv, cx, locals, out) {
        Outcome::Typed(ty) => ty,
        // レシーバが値を産まないなら読みも起きない。理由はレシーバ側にある
        other => return other,
    };
    if optional {
        if !ty.optional {
            out.push(format!(
                "{ctx}: `.?{field}` のレシーバは optional である必要がありますが、`{ty}` です"
            ));
            return Outcome::Poisoned;
        }
    } else if ty.optional {
        out.push(format!(
            "{ctx}: optional 型 `{ty}` から `{field}` を読むには `.?{field}` を使うか、先に `??` で展開してください"
        ));
        return Outcome::Poisoned;
    }
    let Some(declared) = ty.name().and_then(|name| cx.decls.structs.get(name)) else {
        out.push(if optional {
            format!("{ctx}: `{ty}` の中身は struct ではないので `.?{field}` を読めません")
        } else {
            format!("{ctx}: `{ty}` は struct ではないので `{field}` を読めません")
        });
        return Outcome::Poisoned;
    };
    let Some(declared) = declared.get(field) else {
        out.push(if optional {
            format!(
                "{ctx}: `{}` にフィールド `{field}` はありません",
                ty.name().unwrap_or_default()
            )
        } else {
            format!("{ctx}: `{ty}` にフィールド `{field}` はありません")
        });
        return Outcome::Poisoned;
    };
    let mut result = declared.clone();
    // optional は1 bit。宣言型が既に `T?` でも `S?.?field` は `T?` のまま
    if optional {
        result.optional = true;
    }
    Outcome::Typed(result)
}

/// 代入。宛先の型が分かるときだけ値を照合する。束縛の型は宣言時に決まるので、
/// 後の代入では変えない(design.md 決定4)。
fn assign(target: &Expr, value: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) {
    match &target.kind {
        // 代入先の裸の名前は書き込み先であって値の読みではない
        ExprKind::Ident(name) => {
            let declared = match locals.get(name) {
                Some(Binding::Value(ty)) => ty.clone(),
                _ => None,
            };
            match declared {
                Some(declared) => {
                    let what = format!("`{name}` への代入");
                    walk(
                        value,
                        Some((&declared, &Site::What(&what))),
                        cx,
                        locals,
                        out,
                    );
                }
                None => {
                    synth(value, cx, locals, out);
                }
            }
        }
        ExprKind::Field(recv, field) => {
            // optional の中身を取り出す規則はまだ無いので、レシーバは
            // 非 optional と分かるときだけ宛先を引く
            let owner = synth(recv, cx, locals, out)
                .ty()
                .filter(|ty| !ty.optional)
                .and_then(KnownType::name)
                .map(str::to_string);
            let declared = owner
                .as_deref()
                .and_then(|owner| declared_field(owner, field, cx.decls));
            match (owner.as_deref(), declared) {
                (Some(type_name), Some(declared)) => {
                    let site = Site::Field { type_name, field };
                    walk(value, Some((&declared, &site)), cx, locals, out);
                }
                _ => {
                    synth(value, cx, locals, out);
                }
            }
        }
        _ => {
            synth(value, cx, locals, out);
        }
    }
}

/// 呼び出し。解決した署名から個数・引数型・結果型が決まる(design.md 決定6)。
fn call(callee: &Expr, args: &[Expr], cx: &Cx, locals: &mut Locals, out: &mut Out) -> Outcome {
    let sig = match resolve(callee, cx, locals, out) {
        Ok(sig) => sig,
        Err(outcome) => {
            // 解決できなくても実引数自身は検査する。期待型は配れない
            for arg in args {
                synth(arg, cx, locals, out);
            }
            return outcome;
        }
    };
    let name = member(callee);
    if args.len() == sig.params.len() {
        for (index, (arg, expected)) in args.iter().zip(&sig.params).enumerate() {
            let site = Site::Arg {
                callee: &name,
                index,
            };
            walk(arg, Some((expected, &site)), cx, locals, out);
        }
    } else {
        // 個数が合わなければ引数と宣言の対応が取れないので期待型は配らない
        for arg in args {
            synth(arg, cx, locals, out);
        }
        out.push(format!(
            "{}: `{name}` は引数を {} 個取りますが、{} 個渡しています",
            cx.ctx,
            sig.params.len(),
            args.len()
        ));
    }
    // 解決した呼び出しは実効戻り値型を持つ(design.md 決定1・6)
    Outcome::Typed(sig.ret.clone())
}

/// 呼び出し先の署名を選ぶ。レシーバの走査もここで1度だけ行うので、
/// 型を引くためにもう一周する必要が無い。
///
/// 選び方は評価器と同じ順序で、宣言済み enum の限定 variant を constructor として
/// 最初に見てから、スロット経由なら宣言 trait の契約だけ、具体型なら inherent と
/// trait 実装をまとめて名前で絞り一意を要求する(design.md 決定6)。
/// `Err` はそのまま呼び出し式の結果になる。
fn resolve<'d>(
    callee: &Expr,
    cx: &Cx<'d>,
    locals: &mut Locals,
    out: &mut Out,
) -> Result<&'d FnSig, Outcome> {
    let decls = cx.decls;
    let before = out.count();
    let selected = match &callee.kind {
        // 直接呼び出し。名前は値として読まれない
        ExprKind::Ident(name) => Ok(decls.fns.get(name)),
        ExprKind::Field(recv, name) => {
            // ambient スロット経由は宣言 trait の契約だけを見る。スロット名は
            // 値ではないのでレシーバとして走査しない
            if let ExprKind::Ident(recv_name) = &recv.kind
                && let Some(trait_name) = slot_trait(recv_name, decls, locals)
            {
                from_trait(&trait_name, name, true, decls)
            } else {
                match synth(recv, cx, locals, out) {
                    // optional の中身を取り出す規則はまだ無く、配列にメソッドも無い
                    Outcome::Typed(ty) => match ty.name().filter(|_| !ty.optional) {
                        Some(type_name) => from_type(type_name, name, true, decls),
                        None => Ok(None),
                    },
                    other => return Err(other),
                }
            }
        }
        ExprKind::Path(parts) => {
            let [first, name] = parts.as_slice() else {
                out.push(format!(
                    "{}: `{}` の呼び出し先が決まりません",
                    cx.ctx,
                    parts.join("::")
                ));
                return Err(Outcome::Poisoned);
            };
            // 宣言済み enum の variant なら constructor。関連関数や ambient の
            // 型射影より先に見る(design.md 決定6)
            if let Some(ctor) = variant_ctor(first, name, decls) {
                Ok(Some(ctor))
            } else {
                match slot_trait(first, decls, locals) {
                    Some(trait_name) => from_trait(&trait_name, name, false, decls),
                    None => from_type(first, name, false, decls),
                }
            }
        }
        _ => {
            synth(callee, cx, locals, out);
            Ok(None)
        }
    };
    match selected {
        Ok(Some(sig)) => Ok(sig),
        // 宣言を知らない関数・型・レシーバの形。検査済みプログラムでは
        // 呼び出し先が必ず一意に決まる(design.md 決定6)
        Ok(None) => {
            out.explain(
                before,
                format!(
                    "{}: `{}` の呼び出し先が決まりません",
                    cx.ctx,
                    member(callee)
                ),
            );
            Err(Outcome::Poisoned)
        }
        Err(message) => {
            out.push(format!("{}: {message}", cx.ctx));
            Err(Outcome::Poisoned)
        }
    }
}

/// 等価比較。`nil` は反対側の optional 性を文脈にする(design.md 決定7)。
/// 結果はどちらにせよ `bool`。
fn equality(lhs: &Expr, rhs: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Outcome {
    let result = Outcome::Typed(plain("bool"));
    let left_nil = matches!(lhs.kind, ExprKind::Nil);
    let right_nil = matches!(rhs.kind, ExprKind::Nil);
    // 両辺 `nil` は nominal な optional 型を決められない(design.md 決定7)
    if left_nil && right_nil {
        out.push(format!(
            "{}: `==` の両辺が `nil` なので optional の型が決まりません",
            cx.ctx
        ));
        return Outcome::Poisoned;
    }
    if left_nil || right_nil {
        let value = if left_nil { rhs } else { lhs };
        // 片側が `nil` なら、もう片側の型がそのまま optional 性の文脈になる
        if let Outcome::Typed(ty) = synth(value, cx, locals, out)
            && !ty.optional
        {
            let (left, right) = if left_nil {
                ("nil".to_string(), ty.to_string())
            } else {
                (ty.to_string(), "nil".to_string())
            };
            out.push(format!(
                "{}: `==` の両辺は同じ型である必要がありますが、`{left}` と `{right}` です",
                cx.ctx
            ));
        }
        return result;
    }
    let left = synth(lhs, cx, locals, out);
    let right = synth(rhs, cx, locals, out);
    if let (Outcome::Typed(left), Outcome::Typed(right)) = (&left, &right)
        && left != right
    {
        out.push(format!(
            "{}: `==` の両辺は同じ型である必要がありますが、`{left}` と `{right}` です",
            cx.ctx
        ));
    }
    result
}

/// `T? ?? T` は `T` を返す。右辺が枝を抜けるなら中身の型との照合は要らない
/// (design.md 決定7)。
fn coalesce(lhs: &Expr, rhs: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Outcome {
    let ctx = cx.ctx;
    // 左辺が裸の `nil` なら、右辺の非 optional な型がそのまま結果になる
    if matches!(lhs.kind, ExprKind::Nil) {
        return match synth(rhs, cx, locals, out) {
            Outcome::Typed(ty) if ty.optional => {
                out.push(format!(
                    "{ctx}: `??` の右辺には非 optional の値が必要ですが、`{ty}` です"
                ));
                Outcome::Poisoned
            }
            other => other,
        };
    }
    let before = out.count();
    let left = match synth(lhs, cx, locals, out) {
        Outcome::Typed(ty) => ty,
        other => {
            if matches!(other, Outcome::Poisoned) {
                out.explain(before, format!("{ctx}: `??` の左辺の型が決まりません"));
            }
            synth(rhs, cx, locals, out);
            return other;
        }
    };
    if !left.optional {
        out.push(format!(
            "{ctx}: `??` の左辺は optional である必要がありますが、`{left}` です"
        ));
        synth(rhs, cx, locals, out);
        return Outcome::Poisoned;
    }
    let inner = KnownType {
        kind: left.kind,
        optional: false,
    };
    let site = Site::What("`??` の右辺");
    match walk(rhs, Some((&inner, &site)), cx, locals, out) {
        // 右辺が値を産まずに枝を終えても、続く経路の値は左辺の中身
        Outcome::Typed(_) | Outcome::Diverges => Outcome::Typed(inner),
        Outcome::Poisoned => Outcome::Poisoned,
    }
}

/// `match` の結果型。payload の束縛も arm 本体の `let` も他の arm や後続へ
/// 漏らさないよう、arm ごとに `locals` を複製して入る。
fn match_expr(
    expected: Expect,
    subject: &Expr,
    arms: &[MatchArm],
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> Outcome {
    let before = out.count();
    let matched = matched_enum(subject, cx, locals, out);
    check_arms(arms, matched.as_deref(), cx.decls, cx.ctx, out);

    let mut result = want(expected).cloned();
    let mut poisoned = false;
    // arm が1つも無い(空 enum の網羅的な match)ときも、値は産まれない
    let mut continues = false;
    for arm in arms {
        let mut inner = arm_locals(arm, cx.decls, locals);
        // guard は payload を見られるが、本体へ束縛を漏らさないよう複製で検査する
        if let Some(guard) = &arm.guard {
            let mut guard_locals = inner.clone();
            let site = Site::What("arm の guard");
            let before = out.count();
            let outcome = walk(
                guard,
                Some((&plain("bool"), &site)),
                cx,
                &mut guard_locals,
                out,
            );
            // guard が偽になりうるかは網羅性に効くので、実行前に `bool` を確定する
            if matches!(outcome, Outcome::Poisoned) {
                out.explain(
                    before,
                    format!("{}: arm の guard の型が決まりません", cx.ctx),
                );
            }
        }
        let what = format!("arm `{}` の値", arm.pattern.label());
        let site = Site::What(&what);
        let outcome = match &result {
            Some(result) => walk(&arm.body, Some((result, &site)), cx, &mut inner, out),
            None => synth(&arm.body, cx, &mut inner, out),
        };
        match outcome {
            Outcome::Typed(ty) => {
                continues = true;
                if result.is_none() {
                    result = Some(ty);
                }
            }
            // この arm は値を産まずに抜ける。結果型の基準にはならない
            Outcome::Diverges => {}
            Outcome::Poisoned => {
                continues = true;
                poisoned = true;
            }
        }
    }

    match want(expected) {
        // 全 arm が抜けるなら合流点も抜ける。空 enum を 0 arm で網羅した
        // `match` もここ。宛先があっても値は産まれない(design.md 決定5)
        _ if !continues => Outcome::Diverges,
        // 期待型のある位置は arm ごとに照合済みなので、式全体では二度言わない
        Some(expected) => Outcome::Typed(expected.clone()),
        None if poisoned => Outcome::Poisoned,
        None => match result {
            Some(result) => Outcome::Typed(result),
            None => {
                out.explain(
                    before,
                    format!("{}: `match` の結果型が決まりません", cx.ctx),
                );
                Outcome::Poisoned
            }
        },
    }
}

/// `match` の対象の enum。既知の非 optional な enum のときだけ返す。
///
/// 型不明の対象を実行時へ委ねると arm の所属・網羅性・結果型の基準を決められない
/// ので、ここで診断する(design.md 決定3)。
fn matched_enum(subject: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Option<String> {
    let before = out.count();
    let ty = match synth(subject, cx, locals, out) {
        Outcome::Typed(ty) => ty,
        // 対象が抜けるなら arm へ入らない
        Outcome::Diverges => return None,
        Outcome::Poisoned => {
            out.explain(
                before,
                format!("{}: `match` の対象の型が決まりません", cx.ctx),
            );
            return None;
        }
    };
    match ty.name() {
        Some(name) if !ty.optional && cx.decls.enums.contains_key(name) => Some(name.to_string()),
        _ => {
            out.push(format!(
                "{}: `match` の対象は非 optional な enum である必要がありますが、`{ty}` です",
                cx.ctx
            ));
            None
        }
    }
}

/// `Head` で始まる式。`with` と条件式は本体の値を産み、ループは `unit`
/// (design.md 決定5)。
fn head_expr(
    expected: Expect,
    head: &Head,
    body: &Expr,
    orelse: Option<&Expr>,
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> Outcome {
    match head {
        Head::Ambient(binders) => {
            // 提供値は外側で評価される
            for binder in binders {
                let ty = match binder {
                    Provision::Type { type_name, .. } => Some(plain(type_name)),
                    Provision::Value { value, .. } => synth(value, cx, locals, out).ty().cloned(),
                };
                check_provision(binder, ty.as_ref(), cx, out);
            }
            // 本体では内側の束縛が勝つ。スロットでない名前は requirement 側が
            // 報告するので、ここでは型不明の値にする
            let mut inner = locals.clone();
            for binder in binders {
                let slot = binder.slot();
                let binding = match cx.decls.slots.trait_of(slot) {
                    Some(trait_name) => Binding::Slot(trait_name.to_string()),
                    None => Binding::Value(None),
                };
                inner.insert(slot.to_string(), binding);
            }
            walk(body, expected, cx, &mut inner, out)
        }
        Head::If(condition) | Head::Elif(condition) => {
            let site = Site::What("条件");
            walk(condition, Some((&plain("bool"), &site)), cx, locals, out);
            // `else` で終わらない連鎖はどの枝の値も使わないので `unit` を産む。
            // 使われない値に期待型を課さないため、枝へ配るのもそのときだけ
            // (design.md 決定5)
            let valued = has_else(orelse);
            let branch = if valued { expected } else { None };
            let taken = walk(body, branch, cx, &mut locals.clone(), out);
            let Some(orelse) = orelse else {
                return unit();
            };
            let other = walk(orelse, branch, cx, &mut locals.clone(), out);
            if valued {
                merge(expected, taken, other)
            } else {
                unit()
            }
        }
        Head::While(condition) => {
            let site = Site::What("条件");
            walk(condition, Some((&plain("bool"), &site)), cx, locals, out);
            synth(body, cx, &mut locals.clone(), out);
            unit()
        }
        Head::For { var, iter } => {
            let element = iterated(iter, cx, locals, out);
            let mut inner = locals.clone();
            inner.insert(var.clone(), Binding::Value(element));
            synth(body, cx, &mut inner, out);
            unit()
        }
        Head::Else => walk(body, expected, cx, &mut locals.clone(), out),
    }
}

/// 条件式の連鎖が `else` で終わるか。`elif` は入れ子の `Head` として畳まれて
/// いるので、末尾まで辿る。終わらない連鎖はどの枝も値として使われない。
fn has_else(orelse: Option<&Expr>) -> bool {
    match orelse.map(|e| &e.kind) {
        Some(ExprKind::Head {
            head: Head::Else, ..
        }) => true,
        Some(ExprKind::Head { orelse, .. }) => has_else(orelse.as_deref()),
        _ => false,
    }
}

/// `if`/`elif`/`else` の合流。続く枝の型が結果になる(design.md 決定5)。
fn merge(expected: Expect, taken: Outcome, other: Outcome) -> Outcome {
    match (taken, other) {
        // 片方が抜けるなら、続く方の型がそのまま結果
        (Outcome::Diverges, other) | (other, Outcome::Diverges) => other,
        (Outcome::Poisoned, _) | (_, Outcome::Poisoned) => Outcome::Poisoned,
        (Outcome::Typed(taken), Outcome::Typed(other)) => match want(expected) {
            // 枝ごとに照合済みなので、合流点では期待型がそのまま結果
            Some(expected) => Outcome::Typed(expected.clone()),
            None if taken == other => Outcome::Typed(taken),
            None => Outcome::Poisoned,
        },
    }
}

/// `for x in xs` のループ変数の型。反復対象は非 optional な配列でなければ
/// ならない(design.md 決定5)。
fn iterated(iter: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Option<KnownType> {
    let before = out.count();
    let outcome = synth(iter, cx, locals, out);
    let Outcome::Typed(ty) = outcome else {
        // 反復対象の型が決まらないとループ変数の型も決まらない
        if matches!(outcome, Outcome::Poisoned) {
            out.explain(
                before,
                format!("{}: `for` の反復対象の型が決まりません", cx.ctx),
            );
        }
        return None;
    };
    let Some(element) = ty.element() else {
        out.push(format!(
            "{}: `for` の反復対象は配列である必要がありますが、`{ty}` です",
            cx.ctx
        ));
        return None;
    };
    if ty.optional {
        out.push(format!(
            "{}: optional な配列 `{ty}` はそのまま反復できません。先に `??` で展開してください",
            cx.ctx
        ));
        return None;
    }
    Some(element.clone())
}

/// `with` の提供がスロットの契約を満たすか見る。`with db<Postgres>` は型名が
/// そのまま分かるので常に、`with db(v)` は `v` の型が分かるときだけ見る
/// (design.md 決定3)。スロットでない名前は requirement / eval 側が報告する。
///
/// eval.rs にも同じ判定がある。あちらは型の分からない提供を実行時に止める網。
fn check_provision(binder: &Provision, ty: Option<&KnownType>, cx: &Cx, out: &mut Out) {
    let Some(want) = cx.decls.slots.trait_of(binder.slot()) else {
        return;
    };
    let Some(ty) = ty else {
        // 型の分からない提供を実行時へ回さない(design.md 決定3)
        out.push(format!(
            "{}: `{}` に提供する値の型が決まりません",
            cx.ctx,
            binder.slot()
        ));
        return;
    };
    // 提供できるのは trait を実装した具体型そのものだけ。配列と optional は
    // 名前を持たないので、この時点で落ちる
    let implemented = !ty.optional
        && ty.name().is_some_and(|name| {
            cx.decls
                .trait_impls
                .contains(&(name.to_string(), want.to_string()))
        });
    if !implemented {
        out.push(format!(
            "{}: `{ty}` は `{want}` を実装していないので `{}` に提供できません",
            cx.ctx,
            binder.slot()
        ));
    }
}

// ---------------------------------------------------------------------------
// 宣言の索引を引くだけのヘルパ
// ---------------------------------------------------------------------------

/// arm 本体から見えるローカル。pattern の名前は対応する宣言 payload 型を持ち、
/// 同名の外側束縛をこの arm の間だけ隠す。`_` は名前を作らない
/// (design.md 決定5)。
fn arm_locals(arm: &MatchArm, decls: &Decls, locals: &Locals) -> Locals {
    let mut inner = locals.clone();
    // `_` は variant も payload も晒さないので、外側のローカルがそのまま見える
    let MatchPattern::Variant {
        enum_name,
        variant,
        bindings,
    } = &arm.pattern
    else {
        return inner;
    };
    let payload = payload_of(enum_name, variant, decls).unwrap_or(&[]);
    for (n, binding) in bindings.iter().enumerate() {
        if let PatternBinding::Bind(name) = binding {
            // 個数が合わないときは `check_arms` が診断済み。型は付けずに束縛だけ作る
            inner.insert(name.clone(), Binding::Value(payload.get(n).cloned()));
        }
    }
    inner
}

/// arm の集合が対象 enum の宣言 variant を余さず一度ずつ扱うか見る。`_` が
/// あれば残りは全部そこへ行くので、一意で最後であることだけを課す
/// (design.md 決定3)。対象の enum が分からないときは arm 自身の整合だけを見る。
fn check_arms(arms: &[MatchArm], matched: Option<&str>, decls: &Decls, ctx: &str, out: &mut Out) {
    // 重複は guard の有無に関わらず禁じるので、限定 arm は全部ここへ入れる
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    // guard は偽になりうるので、網羅を満たすのは guard の無い arm だけ
    // (design.md 決定4)
    let mut unconditional: BTreeSet<&str> = BTreeSet::new();
    // 呼び出し時点の span は `match` 式全体。arm を指す診断の間だけ差し替える
    let whole = out.span;
    // 先頭 `_` の span。後続 arm が現れたときに「最後でない」と言う位置
    let mut catch_all: Option<Span> = None;
    let mut said_non_final = false;
    for arm in arms {
        // 後続 arm は到達しないが、それ自体の整合は続けて見る
        if let Some(first) = catch_all
            && !said_non_final
        {
            out.span = Some(first);
            out.push(format!("{ctx}: `_` は最後の arm でなければなりません"));
            said_non_final = true;
        }
        out.span = Some(arm.span);
        let MatchPattern::Variant {
            enum_name,
            variant,
            bindings,
        } = &arm.pattern
        else {
            if catch_all.is_none() {
                catch_all = Some(arm.span);
            } else {
                out.push(format!("{ctx}: arm `_` が重複しています"));
            }
            continue;
        };
        let arm_name = arm.pattern.label();
        if !decls.enums.contains_key(enum_name) {
            out.push(format!("{ctx}: `{enum_name}` は enum ではありません"));
            continue;
        }
        let Some(payload) = payload_of(enum_name, variant, decls) else {
            out.push(format!(
                "{ctx}: `{arm_name}` は `{enum_name}` の variant ではありません"
            ));
            continue;
        };
        check_pattern(&arm_name, payload, bindings, ctx, out);
        let Some(matched) = matched else { continue };
        if enum_name != matched {
            out.push(format!(
                "{ctx}: arm `{arm_name}` は `{matched}` の variant ではありません"
            ));
            continue;
        }
        if !covered.insert(variant.as_str()) {
            out.push(format!("{ctx}: arm `{arm_name}` が重複しています"));
        }
        if arm.guard.is_none() {
            unconditional.insert(variant.as_str());
        }
    }

    out.span = whole;

    // 欠落は「無いもの」なので指すべき arm が無い。式全体が唯一正しい位置。
    // `_` があれば欠落は残らない。全 variant を書いた上での `_` は冗長だが、
    // 到達しない式を言う一般の診断がまだ無いので黙って許す(design.md 決定3)。
    // guard 付きの arm は偽になりうるので、欠落は無条件 arm だけから測る
    let Some(matched) = matched else { return };
    if catch_all.is_some() {
        return;
    }
    let missing: Vec<&str> = decls.enums[matched]
        .iter()
        .map(|variant| short_name(variant))
        .filter(|variant| !unconditional.contains(variant))
        .collect();
    if !missing.is_empty() {
        out.push(format!(
            "{ctx}: `{matched}` の variant {} を扱っていません",
            quoted(&missing)
        ));
    }
}

/// pattern の要素が宣言 payload と1対1に並ぶか見る。同じ名前を二度束縛すると
/// 先の payload が黙って消えるので、それも診断する(design.md 決定3)。
fn check_pattern(
    arm_name: &str,
    payload: &[KnownType],
    bindings: &[PatternBinding],
    ctx: &str,
    out: &mut Out,
) {
    if bindings.len() != payload.len() {
        out.push(format!(
            "{ctx}: arm `{arm_name}` は payload を {} 個束縛しますが、`{arm_name}` の payload は {} 個です",
            bindings.len(),
            payload.len()
        ));
        return;
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for binding in bindings {
        if let PatternBinding::Bind(name) = binding
            && !seen.insert(name)
        {
            out.push(format!(
                "{ctx}: arm `{arm_name}` の pattern が `{name}` を二度束縛しています"
            ));
        }
    }
}

/// 宣言済み struct の宣言フィールドの型。期待型を配るためだけに引く。
fn declared_field(type_name: &str, field: &str, decls: &Decls) -> Option<KnownType> {
    decls.structs.get(type_name)?.get(field).cloned()
}

/// `Enum::Variant` が指す宣言済み variant の正準名。
fn declared_variant<'d>(enum_name: &str, variant: &str, decls: &'d Decls) -> Option<&'d String> {
    decls
        .enums
        .get(enum_name)?
        .iter()
        .find(|declared| short_name(declared) == variant)
}

/// 限定 variant の constructor 署名。
fn variant_ctor<'d>(enum_name: &str, variant: &str, decls: &'d Decls) -> Option<&'d FnSig> {
    decls
        .ctors
        .get(declared_variant(enum_name, variant, decls)?)
}

/// 宣言された payload 型の並び。宣言に無い variant なら `None`。
fn payload_of<'d>(enum_name: &str, variant: &str, decls: &'d Decls) -> Option<&'d [KnownType]> {
    Some(variant_ctor(enum_name, variant, decls)?.params.as_slice())
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

/// 裸の名前が値になれるのは、隠されていないフィールド0個の struct か
/// payload 0個の enum variant のときだけ。
fn check_bare(name: &str, decls: &Decls, locals: &Locals, ctx: &str, out: &mut Out) {
    if locals.contains_key(name) {
        return;
    }
    // 裸の payload variant は構築にならない。限定 path の呼び出しを要求する
    // (design.md 決定1)
    if let Some(enum_name) = decls.variants.get(name)
        && let payload = &decls.ctors[name].params
        && !payload.is_empty()
    {
        out.push(format!(
            "{ctx}: `{name}` は payload を {} 個取ります。`{enum_name}::{}(...)` で生成してください",
            payload.len(),
            short_name(name)
        ));
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
                 fn main() { let u = User { id = 1, rank = 2 } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 宣言と順序が違っても通る() {
        assert!(
            errors(
                "struct User { id: int\nrank: int }\n\
                 fn main() { let u = User { rank = 2, id = 1 } }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 未知のstructを報告する() {
        let e = only("fn main() { let m = Missing { x = 1 } }\n");
        assert!(e.contains("struct `Missing` は宣言されていません"), "{e}");
        assert!(e.starts_with("main: "), "{e}");
    }

    #[test]
    fn structでない宣言をstructとして使うと報告する() {
        let e =
            only("trait Database { fn find(self) }\nfn main() { let d = Database { x = 1 } }\n");
        assert!(e.contains("`Database` は struct ではありません"), "{e}");
    }

    #[test]
    fn enumをstructとして生成すると報告する() {
        let e = only("enum Rank { Bronze Gold }\nfn main() { let r = Rank { x = 1 } }\n");
        assert!(e.contains("`Rank` は struct ではありません"), "{e}");
    }

    #[test]
    fn 不足フィールドを報告する() {
        let e = only(
            "struct User { id: int\nrank: int }\n\
             fn main() { let u = User { id = 1 } }\n",
        );
        assert!(e.contains("`rank`"), "{e}");
        assert!(!e.contains("`id`"), "{e}");
    }

    #[test]
    fn 余分なフィールドを報告する() {
        let e = only(
            "struct User { id: int }\n\
             fn main() { let u = User { id = 1, nope = 2 } }\n",
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
             test \"t\" { let u = User {} }\n",
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].starts_with("impl Store::make: "), "{errors:?}");
        assert!(errors[1].starts_with("test \"t\": "), "{errors:?}");
    }

    // ---- 3. bare struct と字句スコープ ----

    #[test]
    fn フィールド0個のstructは名前だけで値になれる() {
        assert!(errors("struct Gold {}\nfn main(-> Gold) { Gold }\n").is_empty());
    }

    #[test]
    fn フィールドを持つstructの裸の名前を報告する() {
        let e = only("struct User { id: int }\nfn main() { User }\n");
        assert!(e.contains("`User` はフィールドを 1 個持ちます"), "{e}");
    }

    #[test]
    fn 同名の引数はstruct名を隠す() {
        assert!(errors("struct User { id: int }\nfn f(User: int -> int) { User }\n").is_empty());
    }

    #[test]
    fn letはstruct名を隠す() {
        assert!(
            errors("struct User { id: int }\nfn f(n: int -> int) { let User = n\nUser }\n")
                .is_empty()
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
        // 責められるのは呼び出し先で、フィールドを持つ struct の裸の読みではない
        let e = only("struct User { id: int }\nfn main() { User(1) }\n");
        assert!(e.contains("`User` の呼び出し先が決まりません"), "{e}");
    }

    // ---- 4. 分かる enum 型 ----

    /// 検査環境の境界。何が「型の分かる入口」かをここで固定する
    const RANKS: &str = "enum Rank { Bronze Gold }\n\
                         enum Grade { Low High }\n\
                         struct User { rank: Rank }\n";

    #[test]
    fn 同じenumのvariantはstruct生成で受理される() {
        assert!(
            errors(&format!(
                "{RANKS}fn main() {{ let u = User {{ rank = Gold }} }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 別のenumのvariantをstruct生成で報告する() {
        let e = only(&format!(
            "{RANKS}fn main() {{ let u = User {{ rank = High }} }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
        assert!(e.contains("`rank`"), "{e}");
    }

    #[test]
    fn variantを束縛したローカルも型が分かる() {
        let e = only(&format!(
            "{RANKS}fn main() {{\n let g = High\n let u = User {{ rank = g }}\n}}\n"
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
                "{RANKS}fn take(r: Rank -> Rank) {{ r }}\n\
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
        // enum を修飾していない path は値にならないので、実行前に落ちる
        let e = only(&format!("{RANKS}fn f() {{ User::nope }}\n"));
        assert!(e.contains("`User::nope` は値として読めません"), "{e}");
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
                "{RANKS}fn f({subject}) {{ let n = match {} {{ Rank::Bronze: 1\nRank::Gold: 2 }} }}\n",
                subject.split(':').next().unwrap()
            ));
            assert!(e.contains("非 optional な enum"), "{subject}: {e}");
            assert!(e.contains(expected), "{subject}: {e}");
        }

        // 対象の型が決まらない `match` も実行前に落ちる。根本は束縛の側なので
        // どちらも位置付きで出る
        let e = errors(&format!(
            "{RANKS}fn f() {{\n\
             \x20 let unknown = nil\n\
             \x20 let n = match unknown {{ Rank::Bronze: 1\nRank::Gold: 2 }}\n\
             }}\n"
        ));
        assert!(
            e.iter()
                .any(|e| e.contains("`unknown` の型が初期化子から決まりません")),
            "{e:?}"
        );
        assert!(
            e.iter()
                .any(|e| e.contains("`match` の対象の型が決まりません")),
            "{e:?}"
        );
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
    fn catch_all_armは残りのvariantを網羅する() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank -> int) {{ match r {{ Rank::Gold: 1\n_: 0 }} }}\n"
            ))
            .is_empty()
        );
        // payload を持つ variant も `_` が引き受ける
        assert!(
            errors(&format!(
                "{LOOKUP}fn f(l: Lookup -> str) {{\n\
                 \x20 match l {{\n\
                 \x20   Lookup::Missing(reason): reason\n\
                 \x20   _: \"other\"\n\
                 \x20 }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    /// 全 variant を書いた上での `_` は到達しないが、到達しない式を言う一般の
    /// 診断がまだ無いので黙って許す(design.md 決定3)
    #[test]
    fn 全variantを書いた後のcatch_allは許す() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank -> int) {{\n\
                 \x20 match r {{ Rank::Bronze: 1\nRank::Gold: 2\n_: 0 }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 重複したcatch_allを報告する() {
        let errors = errors(&format!(
            "{RANKS}fn f(r: Rank -> int) {{ match r {{ Rank::Gold: 1\n_: 0\n_: 2 }} }}\n"
        ));
        // 2つ目の `_` は重複であり、同時に1つ目を最後でなくする
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("`_` は最後の arm"), "{errors:?}");
        assert!(errors[1].contains("arm `_` が重複"), "{errors:?}");
    }

    #[test]
    fn catch_allの後ろにarmは書けない() {
        let errors = errors(&format!(
            "{RANKS}fn f(r: Rank -> int) {{ match r {{ _: 0\nRank::Gold: 1 }} }}\n"
        ));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("`_` は最後の arm"), "{errors:?}");
    }

    /// `_` を巡る診断はどれも arm を指す。欠落だけが match 式全体を指す
    #[test]
    fn catch_allの診断はそのarmを指す() {
        let src = format!("{RANKS}fn f(r: Rank -> int) {{ match r {{ _: 0\nRank::Gold: 1 }} }}\n");
        assert_eq!(spanned(&src, "最後の arm"), "_: 0");

        let src =
            format!("{RANKS}fn f(r: Rank -> int) {{ match r {{ Rank::Gold: 1\n_: 0\n_: 2 }} }}\n");
        assert_eq!(spanned(&src, "重複"), "_: 2");
    }

    // ---- 4d. arm の guard ----

    #[test]
    fn bool_のguardは診断を出さない() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank, ready: bool -> int) {{\n\
                 \x20 match r {{\n\
                 \x20   Rank::Gold if ready: 1\n\
                 \x20   _: 0\n\
                 \x20 }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の分かる非bool_のguardを報告する() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank, n: int -> int) {{ match r {{ Rank::Gold if n: 1\n_: 0 }} }}\n"
        ));
        assert!(e.contains("arm の guard"), "{e}");
        assert!(e.contains("`bool` ですが、`int` です"), "{e}");
    }

    /// 診断は arm 全体でなく guard 式そのものを指す(design.md 決定5)
    #[test]
    fn guardの診断はguard式を指す() {
        let src = format!(
            "{RANKS}fn f(r: Rank, n: int -> int) {{ match r {{ Rank::Gold if n: 1\n_: 0 }} }}\n"
        );
        assert_eq!(spanned(&src, "arm の guard"), "n");
    }

    /// `if` の条件と同じ推論境界。型が分からない guard は実行時へ委ねる
    /// guard が偽になりうるかは網羅性に効くので、`bool` を実行前に確定する
    #[test]
    fn 型の決まらないguardを実行前に報告する() {
        let e = errors(&format!(
            "{RANKS}fn f(r: Rank -> int) {{\n\
             \x20 let unknown = nil\n\
             \x20 match r {{ Rank::Gold if unknown: 1\n_: 0 }}\n\
             }}\n"
        ));
        assert!(
            e.iter()
                .any(|e| e.contains("arm の guard の型が決まりません")),
            "{e:?}"
        );
    }

    #[test]
    fn guardはpayloadの束縛をその型ごと見る() {
        assert!(
            errors(&format!(
                "{LOOKUP}fn f(l: Lookup, n: int -> int) {{\n\
                 \x20 match l {{\n\
                 \x20   Lookup::Found(user, count) if count == n: count\n\
                 \x20   _: 0\n\
                 \x20 }}\n\
                 }}\n"
            ))
            .is_empty()
        );

        // payload の型は guard の中でも効く
        let e = only(&format!(
            "{LOOKUP}fn f(l: Lookup -> int) {{\n\
             \x20 match l {{\n\
             \x20   Lookup::Found(user, count) if user: count\n\
             \x20   _: 0\n\
             \x20 }}\n\
             }}\n"
        ));
        assert!(e.contains("`bool` ですが、`User` です"), "{e}");
    }

    /// guard は偽になりうるので、その variant を網羅したことにはならない
    /// (design.md 決定4)
    #[test]
    fn guard付きのarmだけでは網羅にならない() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank, ready: bool -> int) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Bronze: 1\n\
             \x20   Rank::Gold if ready: 2\n\
             \x20 }}\n\
             }}\n"
        ));
        assert!(e.contains("`Gold`"), "{e}");
        assert!(!e.contains("`Bronze`"), "{e}");
    }

    #[test]
    fn guard付きのarmは最後のcatch_allで網羅になる() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank, ready: bool -> int) {{\n\
                 \x20 match r {{\n\
                 \x20   Rank::Bronze: 1\n\
                 \x20   Rank::Gold if ready: 2\n\
                 \x20   _: 0\n\
                 \x20 }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    /// guard の有無に関わらず、同じ variant の arm は一度だけ
    #[test]
    fn guardがあってもvariantの重複は報告する() {
        let errors = errors(&format!(
            "{RANKS}fn f(r: Rank, ready: bool -> int) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Gold if ready: 1\n\
             \x20   Rank::Gold: 2\n\
             \x20   Rank::Bronze: 3\n\
             \x20 }}\n\
             }}\n"
        ));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("arm `Rank::Gold` が重複"), "{errors:?}");
    }

    /// guard は選択だけを決める。結果型は従来どおり本体から測る
    #[test]
    fn guard付きのarmの値も結果型で照合する() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank, ready: bool -> str) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Gold if ready: 1\n\
             \x20   _: \"other\"\n\
             \x20 }}\n\
             }}\n"
        ));
        assert!(e.contains("arm `Rank::Gold` の値"), "{e}");
        assert!(e.contains("`str` ですが、`int` です"), "{e}");
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
                "{RANKS}fn take(s: str -> str) {{ s }}\n\
                 fn f(r: Rank) {{ let s = take(match r {{ Rank::Bronze: \"b\"\nRank::Gold: \"g\" }}) }}\n"
            ))
            .is_empty()
        );

        let e = only(&format!(
            "{RANKS}fn take(s: str -> str) {{ s }}\n\
             fn f(r: Rank) {{ let s = take(match r {{ Rank::Bronze: \"b\"\nRank::Gold: 1 }}) }}\n"
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

    /// `_` の本体も他の arm と同じ結果型の検査を受け、診断では `_` と綴られる
    #[test]
    fn catch_allの本体も結果型を照合する() {
        assert!(
            errors(&format!(
                "{RANKS}fn take(s: str -> str) {{ s }}\n\
                 fn f(r: Rank) {{ let s = take(match r {{ Rank::Gold: \"g\"\n_: \"other\" }}) }}\n"
            ))
            .is_empty()
        );

        let e = only(&format!(
            "{RANKS}fn take(s: str -> str) {{ s }}\n\
             fn f(r: Rank) {{ let s = take(match r {{ Rank::Gold: \"g\"\n_: 0 }}) }}\n"
        ));
        assert!(e.contains("arm `_` の値"), "{e}");
        assert!(e.contains("`str`"), "{e}");
        assert!(e.contains("`int`"), "{e}");
    }

    /// 期待型が無ければ最初に型の分かる arm が基準。`_` が先頭でも同じ
    #[test]
    fn catch_allも結果型の基準になれる() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank) {{\n\
             \x20 let label = match r {{ _: \"other\" }}\n\
             \x20 let n = label + 1\n\
             }}\n"
        ));
        assert!(e.contains("`str`"), "{e}");
    }

    #[test]
    fn matchの結果型は後続の検査へ届く() {
        // 束縛を経由しても推論した結果型が残る
        let e = only(&format!(
            "{RANKS}fn take(n: int -> int) {{ n }}\n\
             fn f(r: Rank) {{\n\
             \x20 let label = match r {{ Rank::Bronze: \"b\"\nRank::Gold: \"g\" }}\n\
             \x20 let n = take(label)\n\
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

    /// ブロック形の arm は最後の式の型を持ち、`return` する arm は値を産まない。
    /// 結果型は「続く arm」から決まる(design.md 決定5)
    #[test]
    fn 抜けるarmは結果型の基準にならない() {
        let e = only(&format!(
            "{RANKS}fn take(n: int -> int) {{ n }}\n\
             fn f(r: Rank -> int) {{\n\
             \x20 take(match r {{\n\
             \x20   Rank::Bronze {{ \"b\" }}\n\
             \x20   Rank::Gold: return 0\n\
             \x20 }})\n\
             }}\n"
        ));
        // 期待型 `int` は arm へそのまま配られるので、責められるのはブロック形の
        // arm ひとつ。`return` する arm には期待型を課さない
        assert!(e.contains("arm `Rank::Bronze` の値"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`str`"), "{e}");
    }

    /// 全 arm が抜ける `match` は値を産まないので、期待型と照合されない
    #[test]
    fn 全armが抜けるmatchは値を産まない() {
        let e = errors(&format!(
            "{RANKS}fn f(r: Rank -> int) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Bronze: return 1\n\
             \x20   Rank::Gold: return 2\n\
             \x20 }}\n\
             }}\n"
        ));
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn armの束縛は他のarmと後続へ漏れない() {
        let errors = errors(&format!(
            "{RANKS}fn f(r: Rank) {{\n\
             \x20 match r {{\n\
             \x20   Rank::Bronze {{ let u = User {{ rank = Gold }}\nassert u.rank == Gold }}\n\
             \x20   Rank::Gold {{ u.rank = Low }}\n\
             \x20 }}\n\
             \x20 u.rank = Low\n\
             }}\n"
        ));
        // 隣の arm と後続では `u` の型が分からないので、宛先の照合が起きない。
        // 漏れていれば `Rank` への `Grade` の代入として報告されるはず
        assert!(
            !errors.iter().any(|e| e.contains("`Grade`")),
            "隣の arm と後続では `u` の型が分からないので照合しない: {errors:?}"
        );
    }

    // ---- 4d. payload を持つ variant ----

    const LOOKUP: &str = "struct User { rank: Rank }\n\
                          enum Rank { Bronze Gold }\n\
                          enum Lookup { Found(User, int) Missing(str) Skipped }\n";

    #[test]
    fn 限定した構築と分解は診断を出さない() {
        assert!(
            errors(&format!(
                "{LOOKUP}fn take(u: User -> User) {{ u }}\n\
                 fn f(u: User -> str) {{\n\
                 \x20 match Lookup::Found(u, 1) {{\n\
                 \x20   Lookup::Found(found, n) {{ take(found)\n\"found\" }}\n\
                 \x20   Lookup::Missing(reason): reason\n\
                 \x20   Lookup::Skipped: \"skipped\"\n\
                 \x20 }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 構築の個数違いを報告する() {
        for (call, expected) in [
            ("Lookup::Found(u)", "1 個渡しています"),
            ("Lookup::Found(u, 1, 2)", "3 個渡しています"),
        ] {
            let e = only(&format!("{LOOKUP}fn f(u: User) {{ let l = {call} }}\n"));
            assert!(e.contains("`Lookup::Found` は引数を 2 個取ります"), "{e}");
            assert!(e.contains(expected), "{e}");
        }
    }

    #[test]
    fn 構築の引数の型違いを報告する() {
        let e = only(&format!(
            "{LOOKUP}fn f(u: User) {{ let l = Lookup::Found(u, \"x\") }}\n"
        ));
        assert!(e.contains("`Lookup::Found` の第 2 引数"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`str`"), "{e}");
    }

    #[test]
    fn payload_variantは構築しないと値にならない() {
        // 限定 path も裸の名前も、first-class な constructor にはしない
        for (expr, named) in [("Lookup::Found", "`Lookup::Found`"), ("Found", "`Found`")] {
            let e = only(&format!("{LOOKUP}fn f() {{ {expr} }}\n"));
            assert!(e.contains(named), "{e}");
            assert!(e.contains("payload を 2 個取ります"), "{e}");
        }
    }

    #[test]
    fn 構築した値は所属enumの型を持つ() {
        let e = only(&format!(
            "{LOOKUP}fn take(r: Rank -> Rank) {{ r }}\n\
             fn f(u: User) {{ let r = take(Lookup::Skipped) }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Lookup`"), "{e}");

        let e = only(&format!(
            "{LOOKUP}fn take(r: Rank -> Rank) {{ r }}\n\
             fn f(u: User) {{ let r = take(Lookup::Found(u, 1)) }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Lookup`"), "{e}");
    }

    #[test]
    fn payload引数にもoptionalへの注入が効く() {
        assert!(
            errors(
                "enum Box { One(int?) }\n\
                 fn f(-> Box) {{ Box::One(1) }}\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn patternの個数違いを報告する() {
        for (pattern, expected) in [
            ("Lookup::Found(a)", "payload を 1 個束縛します"),
            ("Lookup::Found(a, b, c)", "payload を 3 個束縛します"),
            ("Lookup::Skipped(a)", "payload を 1 個束縛します"),
        ] {
            let arm_enum = pattern.split('(').next().unwrap();
            let e = only(&format!(
                "{LOOKUP}fn f(l: Lookup -> int) {{\n\
                 \x20 match l {{\n\
                 \x20   {pattern}: 1\n\
                 \x20   Lookup::Missing(_): 2\n\
                 \x20   {}: 3\n\
                 \x20 }}\n\
                 }}\n",
                if arm_enum == "Lookup::Skipped" {
                    "Lookup::Found(_, _)"
                } else {
                    "Lookup::Skipped"
                }
            ));
            assert!(e.contains(expected), "{pattern}: {e}");
        }
    }

    #[test]
    fn 同じ名前を二度束縛するpatternを報告する() {
        let e = only(&format!(
            "{LOOKUP}fn f(l: Lookup -> int) {{\n\
             \x20 match l {{\n\
             \x20   Lookup::Found(x, x): 1\n\
             \x20   Lookup::Missing(_): 2\n\
             \x20   Lookup::Skipped: 3\n\
             \x20 }}\n\
             }}\n"
        ));
        assert!(e.contains("`x` を二度束縛"), "{e}");
    }

    #[test]
    fn 束縛の型は既存の検査へ流れる() {
        // field の読み・呼び出しの引数・代入・戻り値の4つを1本の match で通す
        let errors = errors(&format!(
            "{LOOKUP}fn take(n: int -> int) {{ n }}\n\
             fn f(l: Lookup -> int) {{\n\
             \x20 match l {{\n\
             \x20   Lookup::Found(found, n) {{ found.rank = Gold\ntake(found) }}\n\
             \x20   Lookup::Missing(reason): return reason\n\
             \x20   Lookup::Skipped: 0\n\
             \x20 }}\n\
             }}\n"
        ));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("`take` の第 1 引数"), "{errors:?}");
        assert!(errors[0].contains("`User`"), "{errors:?}");
        assert!(errors[1].contains("戻り値は `int`"), "{errors:?}");
        assert!(errors[1].contains("`str`"), "{errors:?}");
    }

    #[test]
    fn 束縛はその本体の間だけ外側を隠す() {
        // arm の中では payload の型、隣の arm と後続では外側の型
        let errors = errors(&format!(
            "{LOOKUP}fn f(l: Lookup, reason: int -> int) {{\n\
             \x20 match l {{\n\
             \x20   Lookup::Found(_, n): n\n\
             \x20   Lookup::Missing(reason): reason\n\
             \x20   Lookup::Skipped: reason\n\
             \x20 }}\n\
             }}\n"
        ));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("arm `Lookup::Missing` の値"),
            "{errors:?}"
        );
        assert!(errors[0].contains("`str`"), "{errors:?}");
    }

    #[test]
    fn discardは外側の名前を隠さない() {
        // 同じ位置を `reason` で束縛すると `str` になって落ちる
        // (上の「束縛はその本体の間だけ外側を隠す」)。`_` なら外側の
        // `reason: int` がそのまま見え続ける
        assert!(
            errors(&format!(
                "{LOOKUP}fn f(l: Lookup, reason: int -> int) {{\n\
                 \x20 match l {{\n\
                 \x20   Lookup::Found(_, _): reason\n\
                 \x20   Lookup::Missing(_): reason\n\
                 \x20   Lookup::Skipped: reason\n\
                 \x20 }}\n\
                 }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn fieldless_enumの既存の検査は変わらない() {
        // payload を持つ variant を1つも使わないプログラムは診断が増えない
        assert!(
            errors(&format!(
                "{RANKS}fn label(r: Rank -> str) {{\n\
                 \x20 match r {{ Rank::Bronze: \"b\"\nRank::Gold: \"g\" }}\n\
                 }}\n\
                 fn f(-> Rank) {{ Rank::Gold }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn variantでないpath呼び出しは従来の解決のまま() {
        // 関連関数・ambient の型射影は constructor に横取りされない
        assert!(
            errors(&format!(
                "{LOOKUP}struct Store {{}}\n\
                 impl Store {{ fn make(-> User) {{ User {{ rank = Gold }} }} }}\n\
                 fn f(-> User) {{ Store::make() }}\n"
            ))
            .is_empty()
        );
        // 宣言済み enum を修飾していても variant でなければ constructor に
        // ならず、呼び出し先が一意に決まらないので実行前に落ちる
        let e = only(&format!("{LOOKUP}fn f() {{ Lookup::Nope(1) }}\n"));
        assert!(
            e.contains("`Lookup::Nope` の呼び出し先が決まりません"),
            "{e}"
        );
    }

    #[test]
    fn payloadの診断は責めるべき式を指す() {
        assert_eq!(
            spanned(
                &format!("{LOOKUP}fn f(u: User) {{ let l = Lookup::Found(u, \"x\") }}\n"),
                "第 2 引数"
            ),
            "\"x\""
        );
        assert_eq!(
            spanned(
                &format!(
                    "{LOOKUP}fn f(l: Lookup -> int) {{\n\
                     \x20 match l {{\n\
                     \x20   Lookup::Found(a): 1\n\
                     \x20   Lookup::Missing(_): 2\n\
                     \x20   Lookup::Skipped: 3\n\
                     \x20 }}\n\
                     }}\n"
                ),
                "payload を 1 個束縛"
            ),
            "Lookup::Found(a): 1"
        );
    }

    // ---- 4e. let の型注釈 ----

    #[test]
    fn 注釈どおりの初期化子は受理される() {
        assert!(errors("fn main() { let n: int = 1 }\n").is_empty());
        assert!(errors(&format!("{RANKS}fn main() {{ let g: Rank = Gold }}\n")).is_empty());
    }

    #[test]
    fn 注釈と違う初期化子を報告する() {
        let e = only("fn main() { let n: int = true }\n");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`bool`"), "{e}");
        assert!(e.contains("`n`"), "{e}");
    }

    /// 注釈は宛先なので、非 optional から optional への注入がそのまま効く
    #[test]
    fn optional注釈は非optionalの値を受け取れる() {
        let e = errors(&format!("{RANKS}fn main(u: User) {{ let o: User? = u }}\n"));
        assert!(e.is_empty(), "{e:?}");
    }

    #[test]
    fn optionalな値は非optional注釈へ入らない() {
        let e = only(&format!("{RANKS}fn main(u: User?) {{ let o: User = u }}\n"));
        assert!(e.contains("`User`"), "{e}");
        assert!(e.contains("`User?`"), "{e}");
    }

    /// 束縛の型は注釈で固定される。初期化子の推論型では上書きしない
    #[test]
    fn 注釈で固定した型は後の参照に届く() {
        let e = only(&format!(
            "{RANKS}fn main() {{\n let g: Grade = Low\n let u = User {{ rank = g }}\n}}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 注釈で固定した型は再代入にも効く() {
        let e = only("fn main() {\n let n: int = 1\n n = true\n}\n");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`bool`"), "{e}");
    }

    #[test]
    fn 注釈の初期化子の診断は初期化子を指す() {
        assert_eq!(
            spanned("fn main() { let n: int = true }\n", "の初期化子"),
            "true"
        );
    }

    // ---- 4f. 制御を抜ける式と診断の重複 ----

    /// `return` は値を産まずに枝を終えるので、どんな期待型の位置にも収まる
    /// (design.md 決定3)
    #[test]
    fn 抜ける枝は期待型と照合されない() {
        let e = errors(&format!(
            "{RANKS}fn f(r: Rank -> Rank) {{\n\
             \x20 if r == Gold {{ return Bronze }} else {{ return Gold }}\n\
             }}\n\
             fn g(o: Rank? -> Rank) {{ o ?? return Bronze }}\n\
             fn h(r: Rank -> str) {{ match r {{ Rank::Bronze: \"b\"\nRank::Gold: return \"g\" }} }}\n"
        ));
        assert!(e.is_empty(), "{e:?}");
    }

    /// `else` で終わらない条件式はどの枝の値も使わないので `unit` を産む。
    /// 使われない値に期待型を課さない(design.md 決定5)
    #[test]
    fn elseの無い条件式はunitを産む() {
        let e = errors(&format!(
            "{RANKS}fn f(r: Rank) {{ if r == Gold {{ 1 }} }}\n"
        ));
        assert!(e.is_empty(), "{e:?}");

        let e = errors(&format!(
            "{RANKS}fn f(r: Rank) {{ if r == Gold {{ 1 }} elif r == Bronze {{ \"x\" }} }}\n"
        ));
        assert!(e.is_empty(), "{e:?}");

        // `else` があれば枝の値が使われるので、期待型と照合する
        let e = only(&format!(
            "{RANKS}fn f(r: Rank -> int) {{ if r == Gold {{ 1 }} else {{ \"x\" }} }}\n"
        ));
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`str`"), "{e}");
    }

    /// ループと空ブロックは値を産まないので `unit`
    #[test]
    fn ループと空ブロックはunitを産む() {
        let e = errors(&format!(
            "{RANKS}fn f(xs: [Rank]) {{ for x in xs {{ assert x == Gold }} }}\n\
             fn g(r: Rank) {{ while r == Gold {{ assert true }} }}\n\
             fn h() {{}}\n"
        ));
        assert!(e.is_empty(), "{e:?}");
    }

    /// `with` と第二級ブロックは本体の最後の式の値をそのまま産む
    /// (design.md 決定5)
    #[test]
    fn withとブロックは本体の値を産む() {
        let e = only(&format!("{RANKS}fn f(r: Rank -> int) {{ {{ Gold }} }}\n"));
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
    }

    /// 子が理由を説明していない `Poisoned` だけを親が説明する。子が自分で
    /// 診断を出すようになれば重複は自動的に消える(design.md 決定3)
    #[test]
    fn 説明済みの失敗に診断を重ねない() {
        // レシーバの型が決まらないので `match` の対象も決まらないが、
        // 報告するのは根本の1件だけ
        let e = only(&format!(
            "{RANKS}fn f(u: User?) {{ let n = match u.rank {{ Rank::Bronze: 1\nRank::Gold: 2 }} }}\n"
        ));
        assert!(e.contains("optional 型 `User?`"), "{e}");
    }

    /// 呼ばれない宣言も検査する。到達性には依らせない
    #[test]
    fn 実行されない宣言の型エラーも報告する() {
        let e = only(&format!(
            "{RANKS}fn never_called(-> int) {{ Gold }}\nfn main() {{ assert true }}\n"
        ));
        assert!(e.starts_with("never_called: "), "{e}");
    }

    // ---- 4g. 空 enum の match ----

    /// 空 enum を 0 arm で網羅した `match` は値を産まない。宛先があっても
    /// 期待型と照合しない(design.md 決定5)
    #[test]
    fn 空enumの0armmatchは値を産まない() {
        const NEVER: &str = "enum Never {}\n";

        // 戻り値の位置
        let e = errors(&format!(
            "{NEVER}fn f(n: Never -> int) {{ match n {{}} }}\n"
        ));
        assert!(e.is_empty(), "{e:?}");

        // 宛先の位置
        let e = errors(&format!(
            "{NEVER}fn f(n: Never -> int) {{ let x: int = match n {{}}\n 0 }}\n"
        ));
        assert!(e.is_empty(), "{e:?}");

        // 引数の位置
        let e = errors(&format!(
            "{NEVER}fn take(x: int -> int) {{ x }}\n\
             fn f(n: Never -> int) {{ take(match n {{}}) }}\n"
        ));
        assert!(e.is_empty(), "{e:?}");
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
        assert!(
            errors("fn f(a: int, b: int -> int) { a }\nfn main(n: int) { let r = f(n, n) }\n")
                .is_empty()
        );
    }

    #[test]
    fn 引数が足りない呼び出しを報告する() {
        let e = only("fn f(a: int, b: int -> int) { a }\nfn main(n: int) { let r = f(n) }\n");
        assert!(e.starts_with("main: "), "{e}");
        assert!(e.contains("`f`"), "{e}");
        assert!(e.contains("2 個取ります"), "{e}");
        assert!(e.contains("1 個渡しています"), "{e}");
    }

    #[test]
    fn 引数が多すぎる呼び出しを報告する() {
        let e = only("fn f(a: int -> int) { a }\nfn main(n: int) { let r = f(n, n) }\n");
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
                "{RANKS}fn f(r: Rank -> Rank) {{ r }}\nfn main() {{ let x = f(Gold) }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の違う引数を位置付きで報告する() {
        let e = only(&format!(
            "{RANKS}fn f(a: Rank, b: Rank -> Rank) {{ a }}\nfn main() {{ let x = f(Gold, High) }}\n"
        ));
        assert!(e.contains("`f` の第 2 引数"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 非optional引数は同名のoptional引数型へ注入できる() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank? -> Rank?) {{ r }}\nfn main(r: Rank) {{ let x = f(r) }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn optional引数は非optional引数型へ注入できない() {
        let e = only(&format!(
            "{RANKS}fn f(r: Rank -> Rank) {{ r }}\nfn main(r: Rank?) {{ let x = f(r) }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`Rank?`"), "{e}");
    }

    #[test]
    fn nil引数は期待するoptional性と照合する() {
        assert!(
            errors(&format!(
                "{RANKS}fn f(r: Rank? -> Rank?) {{ r }}\nfn main() {{ let x = f(nil) }}\n"
            ))
            .is_empty()
        );
        let e = only(&format!(
            "{RANKS}fn f(r: Rank -> Rank) {{ r }}\nfn main() {{ let x = f(nil) }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`nil`"), "{e}");
    }

    // ---- 7. 直接呼び出しの戻り値型 ----

    #[test]
    fn 呼び出し結果を束縛したローカルは型が分かる() {
        let e = only(&format!(
            "{RANKS}fn pick(-> Grade) {{ Low }}\n\
             fn main() {{\n let g = pick()\n let u = User {{ rank = g }}\n}}\n"
        ));
        assert!(e.contains("`Grade`"), "{e}");
    }

    #[test]
    fn 呼び出し結果はそのままフィールド検査に届く() {
        let e = only(&format!(
            "{RANKS}fn pick(-> Grade) {{ Low }}\nfn main() {{ let u = User {{ rank = pick() }} }}\n"
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

    /// 前提が反転したテスト。注釈の省略は `unit` を返す宣言そのものなので、
    /// 呼び出しの結果型が分からないことはもう起きない(design.md 決定1)
    #[test]
    fn 戻り値型を省略した関数の結果はunitとして届く() {
        let e = only(&format!(
            "{RANKS}fn pick() {{ assert true }}\n\
             fn main() {{ let u = User {{ rank = pick() }} }}\n"
        ));
        assert!(e.contains("`Rank`"), "{e}");
        assert!(e.contains("`unit`"), "{e}");
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

    /// **BREAKING**: 注釈を省略した宣言が `unit` 以外の値で終わるとエラー。
    /// 本体から戻り値型を推論すると再帰と相互再帰に制約解決が要る(design.md 決定1)
    #[test]
    fn 戻り値型を省略して値を返す関数を報告する() {
        let e = only(&format!("{RANKS}fn pick() {{ Gold }}\n"));
        assert!(e.contains("`unit`"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");

        // 明示 `return` も同じ実効戻り値と照合する
        let e = only(&format!("{RANKS}fn pick() {{ return Gold }}\n"));
        assert!(e.contains("`unit`"), "{e}");
        assert!(e.contains("`Rank`"), "{e}");
    }

    #[test]
    fn 戻り値型を省略した宣言はunitで終われる() {
        let e = errors(&format!(
            "{RANKS}fn stamp(u: User) {{ u.rank = Gold }}\n\
             fn check(u: User) {{ assert u.rank == Gold }}\n\
             fn bind(u: User) {{ let r = u.rank }}\n\
             fn nothing() {{}}\n"
        ));
        assert!(e.is_empty(), "{e:?}");
    }

    /// 実効戻り値は宣言だけから決まるので、本体を辿らずに引ける。
    /// 再帰と相互再帰がそのまま通る(design.md 決定1)
    #[test]
    fn 再帰する宣言も実効戻り値で検査できる() {
        let e = errors(
            "fn down(n: int -> int) { down(n - 1) }\n\
             fn ping(n: int -> int) { pong(n - 1) }\n\
             fn pong(n: int -> int) { ping(n - 1) }\n",
        );
        assert!(e.is_empty(), "{e:?}");
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
                "fn f(a: int, b: bool, c: str -> int) { a }\n\
                 fn main() { let r = f(1, true, \"x\") }\n",
            )
            .is_empty()
        );
    }

    #[test]
    fn 型の違うリテラル引数を報告する() {
        let e = only("fn f(a: int -> int) { a }\nfn main() { let r = f(\"x\") }\n");
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
            "fn bool(-> int) { 1 }\n",
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

    /// 型の決まらないレシーバは、フィールドの読みを実行時へ委ねずに落とす
    #[test]
    fn 型の決まらないレシーバのフィールドを実行前に報告する() {
        let e = errors(&format!(
            "{NESTED}fn f() {{\n\
             \x20 let unknown = nil\n\
             \x20 for x in unknown {{ x.nope }}\n\
             \x20 unknown.?nope\n\
             }}\n"
        ));
        // 根本は束縛。反復対象も optional field access もそこから説明される
        assert!(
            e.iter()
                .any(|e| e.contains("`unknown` の型が初期化子から決まりません")),
            "{e:?}"
        );
        assert!(
            e.iter()
                .any(|e| e.contains("`for` の反復対象の型が決まりません")),
            "{e:?}"
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
                "{OPTIONAL_FIELDS}fn take(s: str? -> str?) {{ s }}\n\
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
            "{OPTIONAL_FIELDS}fn take(n: int? -> int?) {{ n }}\n\
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
                 fn main() { let r = f(1, 2) }\n",
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
        let e = only("fn f(r: str -> str) { r }\nfn main(n: int) { let x = f(n + 1) }\n");
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
        // 両辺 `nil` はどちらからも nominal な optional 型を決められない
        let e = only("fn f(-> bool) { nil == nil }\n");
        assert!(e.contains("両辺が `nil`"), "{e}");
    }

    // ---- 12. optional fallback ----

    #[test]
    fn optionalと同じ中身のfallbackは中身の型になる() {
        assert!(
            errors(&format!(
                "{RANKS}fn take(r: Rank -> Rank) {{ r }}\n\
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
            "{RANKS}fn take(g: Grade -> Grade) {{ g }}\n\
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
                "{RANKS}fn take(r: Rank -> Rank) {{ r }}\nfn f() {{ let r = take(nil ?? Gold) }}\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn 型の決まらない左辺のfallbackを実行前に報告する() {
        let e = errors(&format!(
            "{RANKS}fn f(-> Grade) {{\n\
             \x20 let unknown = nil\n\
             \x20 unknown ?? Gold\n\
             }}\n"
        ));
        assert!(
            e.iter()
                .any(|e| e.contains("`unknown` の型が初期化子から決まりません")),
            "{e:?}"
        );

        // 注釈を与えれば通る
        let e = errors(&format!(
            "{RANKS}fn f(-> Rank) {{\n\
             \x20 let known: Rank? = nil\n\
             \x20 known ?? Gold\n\
             }}\n"
        ));
        assert!(e.is_empty(), "{e:?}");
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
    fn 型の決まらない条件と表明を実行前に報告する() {
        let e =
            errors("fn f() {\n let unknown = nil\n for x in unknown { if x { assert x } }\n}\n");
        assert!(
            e.iter()
                .any(|e| e.contains("`unknown` の型が初期化子から決まりません")),
            "{e:?}"
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
        // 代入から遡って型を決めるには分岐の合流と到達性が要る(design.md 決定4)。
        // 遡らない代わりに、束縛の時点で注釈を要求する
        let e = errors(&format!(
            "{FIELDS}fn f() {{\n let r = nil\n r = Gold\n r = Low\n}}\n"
        ));
        assert!(
            e.iter()
                .any(|e| e.contains("`r` の型が初期化子から決まりません")),
            "{e:?}"
        );

        // 注釈があれば宛先の型が決まるので、食い違う代入が報告される
        let e = only(&format!(
            "{FIELDS}fn f() {{\n let r: Rank? = nil\n r = Gold\n r = Low\n}}\n"
        ));
        assert!(e.contains("`r` への代入"), "{e}");
        assert!(e.contains("`Rank?`"), "{e}");
        assert!(e.contains("`Grade`"), "{e}");
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
                "fn take(xs: [int] -> [int]) { xs }\n\
                 fn main() { let r = take([1, 2, 3]) }\n",
            )
            .is_empty()
        );
        // 推論した配列型は束縛を越えて既存の照合へ届く
        let e = only(
            "fn take(xs: [str] -> [str]) { xs }\nfn main() { let xs = [1, 2]\nlet r = take(xs) }\n",
        );
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
        // 空配列も、`nil` しか無い配列も、後の使用から遡って型を得ない。
        // 遡らない代わりに、リテラルの時点で要素型を要求する
        let e = errors(
            "fn main() {\n\
             \x20 let empty = []\n\
             \x20 let nils = [nil, nil]\n\
             }\n",
        );
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(
            e.iter().all(|e| e.contains("配列の要素型が決まりません")),
            "{e:?}"
        );

        // 注釈か宛先の型があれば要素型が決まる
        let e = errors(
            "struct User { id: int }\n\
             fn take(xs: [User] -> [User]) { xs }\n\
             fn main() {\n\
             \x20 let empty: [User] = []\n\
             \x20 let nils: [User?] = [nil, nil]\n\
             \x20 let direct = take([])\n\
             }\n",
        );
        assert!(e.is_empty(), "{e:?}");
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
                "{ARRAYS}fn take(xs: [User]? -> [User]?) {{ xs }}\nfn main(xs: [User]) {{ let r = take(xs) }}\n"
            ))
            .is_empty(),
            "外側の optional への注入は既存の規則どおり通る"
        );

        let e = only(&format!(
            "{ARRAYS}fn take(xs: [User?] -> [User?]) {{ xs }}\nfn main(xs: [User]) {{ let r = take(xs) }}\n"
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
             \x20 fn extra(self -> int) {{ 1 }}\n\
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
                "{CALLS}fn f(store: Store -> User?) {{ with db(store) {{ db.find(1) }} }}\n"
            ))
            .is_empty()
        );

        let e = only(&format!(
            "{CALLS}fn f(db: Store) {{ with db(db) {{ db.save(1) }} }}\n"
        ));
        assert!(e.contains("`Database` に `save` はありません"), "{e}");
    }

    /// レシーバの型が決まらない呼び出しは解決できないので実行前に落ちる。
    /// 根本は束縛なので、そこだけを指す(design.md 決定6)
    #[test]
    fn 型の決まらないレシーバの呼び出しを実行前に報告する() {
        let e = only(&format!(
            "{CALLS}fn f() {{\n\
             \x20 let u = nil\n\
             \x20 u.nope(1)\n\
             }}\n"
        ));
        assert!(e.contains("`u` の型が初期化子から決まりません"), "{e}");
    }

    /// 配列と optional にはメンバーが無いので、レシーバの型が分かっても
    /// 呼び出し先は決まらない
    #[test]
    fn メンバーを持たないレシーバの呼び出しを報告する() {
        let e = only(&format!("{CALLS}fn f(xs: [Store]) {{ xs.nope(1) }}\n"));
        assert!(e.contains("`nope` の呼び出し先が決まりません"), "{e}");

        let e = only(&format!("{CALLS}fn f(s: Store?) {{ s.nope(1) }}\n"));
        assert!(e.contains("`nope` の呼び出し先が決まりません"), "{e}");
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
        let e = only(&format!("{CALLS}fn f(s: Store) {{ let u = s.find() }}\n"));
        assert!(e.contains("`find` は引数を 1 個取ります"), "{e}");

        let e = only(&format!(
            "{CALLS}fn f(s: Store) {{ let u = s.find(\"x\") }}\n"
        ));
        assert!(e.contains("`find` の第 1 引数"), "{e}");
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`str`"), "{e}");

        let e = only(&format!("{CALLS}fn f() {{ let s = Store::new(1) }}\n"));
        assert!(e.contains("`Store::new` は引数を 0 個取ります"), "{e}");
    }

    #[test]
    fn 解決した呼び出しの引数にも既存のoptional規則が効く() {
        assert!(
            errors(&format!(
                "{CALLS}impl Store {{ fn take(self, u: User? -> User?) {{ u }} }}\n\
                 fn f(s: Store, u: User) {{\n\
                 \x20 let a = s.take(u)\n\
                 \x20 let b = s.take(nil)\n\
                 }}\n"
            ))
            .is_empty(),
            "`T` から `T?` への注入は宛先の規則どおり通る"
        );

        let e = only(&format!(
            "{CALLS}impl Store {{ fn take(self, u: User -> User) {{ u }} }}\n\
             fn f(s: Store, u: User? -> User) {{ s.take(u) }}\n"
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
    fn 戻り値型を省略したメソッドの結果もunitとして届く() {
        let e = only(&format!(
            "{CALLS}impl Store {{ fn nothing(self) {{ assert true }} }}\n\
             fn f(s: Store -> int) {{ s.nothing() }}\n"
        ));
        assert!(e.contains("`int`"), "{e}");
        assert!(e.contains("`unit`"), "{e}");
    }

    /// 契約側が注釈を省略していれば、実装側も `unit` を返さなければならない。
    /// 省略と `-> unit` は同じ宣言(design.md 決定1)
    #[test]
    fn trait契約の省略した戻り値はunitとして照合する() {
        let e = errors(
            "struct Store {}\n\
             trait Sink { fn put(self, n: int) }\n\
             impl Sink for Store { fn put(self, n: int -> unit) { assert n == n } }\n",
        );
        assert!(e.is_empty(), "{e:?}");

        let e = only(
            "struct Store {}\n\
             trait Sink { fn put(self, n: int) }\n\
             impl Sink for Store { fn put(self, n: int -> int) { n } }\n",
        );
        assert!(e.contains("`put` の戻り値"), "{e}");
        assert!(e.contains("`unit`"), "{e}");
        assert!(e.contains("`int`"), "{e}");
    }

    // ---- 全域性の網 ----

    /// 型を出せなかった式が診断なしで残ったら、規則の抜けとして落とす。
    /// この網があるので、新しい式の形を足したときに黙って Unknown が
    /// 戻ることはない(design.md 決定3)
    #[test]
    fn 説明の無い型不明は規則の抜けとして落ちる() {
        // 捨てられる裸の `nil` は誰も期待型を与えないので、この網だけが拾う
        let e = only("fn f() {\n nil\n assert true\n}\n");
        assert!(e.contains("型検査の規則が足りていません"), "{e}");
        assert_eq!(
            spanned("fn f() {\n nil\n assert true\n}\n", "規則が"),
            "nil"
        );
    }

    /// 実行前に落ちる診断はすべて位置を持つ。位置の無い診断は render できない
    #[test]
    fn 全域性を閉じた診断は位置を持つ() {
        for (src, part, pointed) in [
            // 注釈を促す診断は束縛そのものを指す
            (
                "fn f() { let x = nil }\n",
                "初期化子から決まりません",
                "let x = nil",
            ),
            (
                "fn f() { let xs = [] }\n",
                "配列の要素型が決まりません",
                "[]",
            ),
            (
                "fn f(-> bool) { nil == nil }\n",
                "両辺が `nil`",
                "nil == nil",
            ),
            ("fn f() { nope() }\n", "呼び出し先が決まりません", "nope()"),
            (
                "struct S { a: int }\nfn f() { S }\n",
                "フィールドを 1 個",
                "S",
            ),
        ] {
            assert_eq!(spanned(src, part), pointed, "{src}");
        }
    }

    /// 呼ばれない宣言も同じ規則で閉じている。検査は到達性に依らない
    #[test]
    fn 呼ばれない宣言も全域性を閉じる() {
        let e = errors(
            "struct S { a: int }\n\
             fn unused(s: S?) {\n\
             \x20 let x = nil\n\
             \x20 let xs = []\n\
             \x20 s.nope()\n\
             }\n\
             fn main() { assert true }\n",
        );
        assert_eq!(e.len(), 3, "{e:?}");
        assert!(e.iter().all(|e| e.starts_with("unused: ")), "{e:?}");
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
