//! struct の形と、全ての式・呼び出しの型検査(docs/overview.md「検査がいま保証すること」)。
//!
//! モジュール解決済みの `Program` を受け取り、宣言と使用を突き合わせる。
//! ここを通ったプログラムでは
//!
//!   - 全ての struct リテラルが宣言済み struct を指し、宣言フィールドを
//!     過不足なく一度ずつ持つ
//!   - ローカルに隠されていない bare struct 値はフィールド0個
//!   - `int` / `bool` / `str` / `unit` を名乗る宣言が無い
//!   - 非 optional のレシーバからのフィールドの読みは、宣言済み
//!     struct の宣言フィールドを指す
//!   - 算術と単項 `-` の被演算子は `int`、`==` の両辺は同じ型、
//!     `if` / `elif` / `while` の条件と `assert` の対象は `bool`
//!   - struct リテラルのフィールド値・フィールドへの代入・ローカルへの再代入は、
//!     宛先の型と適合する
//!   - trait `impl` は宣言済み trait と struct を指し、契約のメソッドを
//!     過不足なく一度ずつ、宣言どおりのレシーバ・引数型・戻り値型で持つ
//!   - 直接呼び出し・メソッド・関連関数は一意の宣言へ解決され、`.` と `::` は
//!     宣言された `self` の有無と一致する
//!   - 解決した呼び出しは宣言どおりの引数の個数を持ち、全ての引数は
//!     宣言された引数型と適合する
//!   - 全ての関数・trait メンバー・実装メソッドは実効戻り値型を持つ。注釈が
//!     あればそれ、無ければ `unit`。明示 `return` と最後の式はその型と
//!     適合する(注釈を省略して `unit` 以外を返すのはエラー)
//!   - 配列リテラルの要素は、期待要素型があればそれと、無ければ互いに適合する
//!   - `for` の反復対象は非 optional な配列で、ループ変数は要素型を持つ
//!   - `with` の提供はスロットの trait を実装した具体型
//!   - 限定 variant の呼び出しは宣言 payload と同じ個数の引数を持ち、
//!     全ての引数は対応する payload 型と適合する。payload を持つ variant は
//!     呼び出さない限り値にならない
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
//! レシーバ型、呼び出し先、引数型、または `with slot(v)` の提供値型を
//! 決められなければ、その式を指す診断を出して評価前に止める。検査成功後に
//! Unknown は残らない。eval 側の同じ判定は検査器の不具合に備えた防御として残す。
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
    AccessMode, BinOp, Expr, ExprKind, Head, Item, MatchArm, MatchPattern, PatternBinding, Program,
    Provision, ReceiverMode, Sig, Type, TypeKind, TypeMode, UnOp,
};
use crate::diag::Diag;
use crate::hir;
use crate::lex::Span;
use crate::module::short_name;
use crate::requirement::Slots;
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
    /// スロット名 → trait 名。`requirement` の表をそのまま借りる
    slots: Slots,
    /// struct ではない宣言名(trait / effect / fn / enum / variant)。
    /// 「未知」と「struct ではない」を言い分けるためだけに持つ
    others: BTreeSet<String>,
    /// 正準名から HIR の ID を引く橋
    ids: Ids,
    /// 型注釈の名前を解決する表。`let` の注釈と式の型を下ろすのに要る
    nominal: Nominal,
}

/// 正準名 → HIR の ID。AST が名前で書かれている間だけ要る橋で、HIR の中には
/// 出て行かない(design.md 決定1)。
#[derive(Default)]
struct Ids {
    structs: BTreeMap<String, hir::StructId>,
    /// (struct の正準名, フィールド名) → フィールド
    fields: BTreeMap<(String, String), hir::FieldId>,
    enums: BTreeMap<String, hir::EnumId>,
    /// variant の正準名 → variant
    variants: BTreeMap<String, hir::VariantId>,
    traits: BTreeMap<String, hir::TraitId>,
    /// (trait の正準名, メソッド名) → 契約メソッド
    trait_methods: BTreeMap<(String, String), hir::TraitMethodId>,
    slots: BTreeMap<String, hir::SlotId>,
    /// トップレベル関数
    fns: BTreeMap<String, hir::CallableId>,
    /// (具体型の正準名, メンバー名) → 本体。inherent と trait 実装を混ぜるのは
    /// 呼び出し側の絞り方と同じ。同名が複数残ることもあるので候補の列で持つ
    /// (`from_type` と同じく、一意でなければ呼び出しを曖昧として落とす)
    methods: BTreeMap<(String, String), Vec<hir::CallableId>>,
    /// (具体型の正準名, trait の正準名) → `impl`
    trait_impls: BTreeMap<(String, String), hir::TraitImplId>,
}

/// 宣言パスの前半で分かる名前だけ。型注釈の解決に要る分で、宣言の前方参照を
/// 許すために本体より前に全部揃える(design.md 決定7の宣言パス)。
#[derive(Default)]
struct Nominal {
    /// struct と enum の正準名 → HIR の型の形
    types: BTreeMap<String, hir::TypeKind>,
    traits: BTreeMap<String, hir::TraitId>,
}

/// 段3で本体を埋める先。宣言パスが item 順(`impl` の中はメソッド順)に積み、
/// 本体の検査が同じ順で取り出す。
///
/// `Discard` は置き場所を決められなかった宣言(struct でない型の `impl` など)。
/// 本体の検査は続けるが HIR には残さない。そういう宣言は必ず診断を伴うので、
/// 成功した HIR に欠けは無い。
#[derive(Clone, Copy)]
enum Target {
    Callable(hir::CallableId),
    Test(hir::TestId),
    Discard,
}

/// 分かっている型。同一性は形と後置 `?` と借用の強さの一致
/// (nominal, design.md 決定1・3)。配列は要素型まで含めて一致しないと同じ型では
/// ない(要素型は不変)。所有 `T`・共有 `&T`・排他 `&mut T` は別の型。
#[derive(Clone, PartialEq, Eq)]
struct KnownType {
    /// `&T` / `&mut T`。所有値は `None`。`kind` と `optional` は常に
    /// **借用先の所有の形**を表すので、名前・要素・宣言の索引は借用を通して引ける
    reference: Option<hir::RefKind>,
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
        if let Some(kind) = self.reference {
            write!(f, "{}", kind.spelling())?;
        }
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
    /// `self` / `&self` / `&mut self`。取らないなら `None`(design.md 決定2)。
    /// trait 実装はここまで含めて契約と一致していなければならない
    receiver: Option<ReceiverMode>,
    params: Vec<KnownType>,
    ret: KnownType,
}

/// 宣言された署名を検査用の形にする。引数名は実装側の局所名なので落とす。
fn signature(sig: &Sig) -> FnSig {
    FnSig {
        receiver: sig.receiver,
        params: sig.params.iter().map(|p| known(&p.ty)).collect(),
        ret: effective_ret(sig),
    }
}

/// レシーバの局所束縛の型。`&self` は `&T`、`&mut self` は `&mut T`、
/// `self` は所有の `T`(design.md 決定2)。
fn receiver_type(mode: ReceiverMode, type_name: &str) -> KnownType {
    KnownType {
        reference: match mode {
            ReceiverMode::Owned => None,
            ReceiverMode::Shared => Some(hir::RefKind::Shared),
            ReceiverMode::Mutable => Some(hir::RefKind::Mutable),
        },
        ..plain(type_name)
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
    /// 値の束縛。型が分かっているものだけ型を持つ。`LocalId` は本体の中で
    /// 一意で、`Locals` の複製がそのまま HIR の字句スコープになる
    Value(Option<KnownType>, hir::LocalId),
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
        reference: ref_kind(ty.mode),
        kind,
        optional: ty.optional,
    }
}

/// 注釈の所有モードを借用の強さへ。所有 `T` は借用ではないので `None`。
fn ref_kind(mode: TypeMode) -> Option<hir::RefKind> {
    match mode {
        TypeMode::Owned => None,
        TypeMode::Shared => Some(hir::RefKind::Shared),
        TypeMode::Mutable => Some(hir::RefKind::Mutable),
    }
}

/// 後置 `?` の付かない所有型。variant / struct リテラル / self に使う。
fn plain(name: &str) -> KnownType {
    KnownType {
        reference: None,
        kind: KnownKind::Named(name.to_string()),
        optional: false,
    }
}

/// 後置 `?` の付かない所有の配列型。
fn array_of(element: KnownType) -> KnownType {
    KnownType {
        reference: None,
        kind: KnownKind::Array(Box::new(element)),
        optional: false,
    }
}

/// 構文木の `effect` 宣言から、スロット名 → trait 名の表を作る。
///
/// これがないと `db.save(u)` を見たとき、`db` がスロットなのかただのローカル
/// 変数なのか区別がつかない。名前で引く必要があるのは検査の間だけなので、
/// この表を作るのも検査器の仕事(下ろした後は `SlotId` で足りる)。
fn collect_slots(program: &Program) -> Slots {
    let mut map = std::collections::HashMap::new();
    for item in &program.items {
        if let Item::Effect {
            slot, trait_name, ..
        } = item
        {
            // 重複は要求解析の診断で止めるため、どちらが残るかは観測されない
            map.insert(slot.clone(), trait_name.clone());
        }
    }
    Slots { map }
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
    /// 既に報告した「宣言されていない型」の名前。同じ名前が注釈のあちこちに
    /// 書かれていても診断は最初の1件だけにする
    unknown_types: BTreeSet<String>,
    /// いま下ろしている本体。式と局所束縛はここへ確保する。
    ///
    /// 診断と同じ受け皿に載せてあるのは、検査と下ろしが**同じ1回の走査**
    /// (design.md 決定7)で、走査の全ての関数が既にこれを可変で持って
    /// いるため。宣言ごとに `check_body` が差し替える
    body: hir::Body,
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

/// 型検査と HIR の構築。**同じ1回の走査**で型と呼び出し先を決めながら下ろすので、
/// 検査が知った事実を後から復元し直すことがない(design.md 決定7)。
///
/// 診断が1件でもあれば下ろしかけた HIR は捨てる。返った HIR には
/// `Poison` な式も未解決の型も残らない。
pub fn check_and_lower(program: &Program) -> Result<hir::Program, Vec<Diag>> {
    let mut out = Out {
        diagnostics: Vec::new(),
        span: None,
        unexplained: None,
        unknown_types: BTreeSet::new(),
        body: hir::Body::default(),
    };
    let (decls, mut lowered, targets) = collect(program, &mut out);
    // 宣言が全部揃ってから所有の内包を見る。無限の大きさの型は本体の検査より
    // 前に止める(design.md 決定10)
    check_type_cycles(&lowered, &mut out);
    let out = &mut out;
    let mut targets = targets.into_iter();

    for item in &program.items {
        out.span = Some(item.span());
        match item {
            Item::Fn { sig, body, .. } => {
                let target = next_target(&mut targets);
                check_body(
                    body,
                    Some(sig),
                    None,
                    &decls,
                    &sig.name,
                    &effective_ret(sig),
                    target,
                    &mut lowered,
                    out,
                );
            }
            // test は呼び出されないので署名を持たないが、本体の最後の式と
            // `return` はどこかの型と照合されなければ検査が閉じない。
            // 値を返す先が無いので `unit` を実効戻り値にする
            Item::Test { name, body, .. } => {
                let target = next_target(&mut targets);
                let ctx = format!("test \"{name}\"");
                check_body(
                    body,
                    None,
                    None,
                    &decls,
                    &ctx,
                    &plain("unit"),
                    target,
                    &mut lowered,
                    out,
                );
            }
            Item::Impl {
                type_name, methods, ..
            } => {
                for (sig, body) in methods {
                    let target = next_target(&mut targets);
                    let ctx = format!("impl {type_name}::{}", sig.name);
                    out.span = Some(sig.span);
                    check_body(
                        body,
                        Some(sig),
                        sig.receiver.map(|mode| receiver_type(mode, type_name)),
                        &decls,
                        &ctx,
                        &effective_ret(sig),
                        target,
                        &mut lowered,
                        out,
                    );
                }
            }
            _ => {}
        }
    }
    debug_assert!(
        targets.next().is_none(),
        "宣言パスが積んだ本体の数が検査した本体の数と合いません"
    );

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

    if !out.diagnostics.is_empty() {
        // 部分的な HIR は渡さない。不完全な参照が後段へ漏れる口を作らないため
        return Err(std::mem::take(&mut out.diagnostics));
    }
    // 検査が成功したのに型の出ない式が残っているのは検査器の不具合。
    // 後段は「全ての式が具体型を持つ」前提で書かれているので、ここで落とす
    if let Some(span) = lowered.poisoned() {
        return Err(vec![Diag::at(
            span,
            "この式の型が決まりません(型検査の規則が足りていません)".to_string(),
        )]);
    }
    Ok(lowered)
}

/// 宣言パスが積んだ次の置き場所。数が合わないのは宣言パスと本体の走査が
/// 食い違ったときだけなので、そこで落とす。
fn next_target(targets: &mut std::vec::IntoIter<Target>) -> Target {
    targets
        .next()
        .expect("宣言パスが積んだ本体を同じ順で取り出す")
}

fn collect(program: &Program, out: &mut Out) -> (Decls, hir::Program, Vec<Target>) {
    let mut structs = BTreeMap::new();
    let mut variants = BTreeMap::new();
    let mut enums = BTreeMap::new();
    let mut ctors = BTreeMap::new();
    let mut fns = BTreeMap::new();
    let mut traits: BTreeMap<String, BTreeMap<String, FnSig>> = BTreeMap::new();
    let mut impls: BTreeMap<String, Vec<(String, FnSig)>> = BTreeMap::new();
    let mut others = BTreeSet::new();

    // 宣言の前半。型注釈が前方参照できるよう、名前と ID だけ先に全部揃える
    let mut lowered = hir::Program::default();
    let nominal = collect_nominal(program, &mut lowered);
    let mut ids = Ids::default();
    let mut targets: Vec<Target> = Vec::new();
    let mut pending: Vec<PendingImpl> = Vec::new();

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
            Item::Struct { name, fields, span } => {
                let owner = nominal.struct_of(name).expect("宣言パス前半で確保済み");
                ids.structs.insert(name.clone(), owner);
                let mut declared: BTreeMap<String, KnownType> = BTreeMap::new();
                let mut duplicates = BTreeSet::new();
                for crate::ast::FieldDecl {
                    name: field,
                    ty,
                    indirect,
                } in fields
                {
                    let previous = declared.insert(field.clone(), known(ty));
                    if previous.is_some() {
                        duplicates.insert(field.clone());
                    }
                    check_type_shape(
                        ty,
                        &RefSite::Owned,
                        &format!("struct `{name}` のフィールド `{field}`"),
                        out,
                    );
                    // フィールド単体の span は構文木が持たないので宣言全体を指す
                    let id = lowered.fields.alloc(hir::FieldDecl {
                        name: field.clone(),
                        owner,
                        ty: lower_type(ty, &nominal, out),
                        indirect: *indirect,
                        span: *span,
                    });
                    lowered.structs.get_mut(owner).fields.push(id);
                    ids.fields.insert((name.clone(), field.clone()), id);
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
                name,
                variants: vs,
                span,
            } => {
                others.insert(name.clone());
                let owner = nominal.enum_of(name).expect("宣言パス前半で確保済み");
                ids.enums.insert(name.clone(), owner);
                for variant in vs {
                    variants.insert(variant.name.clone(), name.clone());
                    others.insert(variant.name.clone());
                    ctors.insert(
                        variant.name.clone(),
                        FnSig {
                            receiver: None,
                            params: variant.payload.iter().map(|p| known(&p.ty)).collect(),
                            ret: plain(name),
                        },
                    );
                    for (position, payload) in variant.payload.iter().enumerate() {
                        check_type_shape(
                            &payload.ty,
                            &RefSite::Owned,
                            &format!(
                                "enum `{name}` の variant `{}` の第 {} payload",
                                short_name(&variant.name),
                                position + 1
                            ),
                            out,
                        );
                    }
                    let id = lowered.variants.alloc(hir::VariantDecl {
                        name: variant.name.clone(),
                        owner,
                        payload: variant
                            .payload
                            .iter()
                            .map(|p| hir::PayloadDecl {
                                ty: lower_type(&p.ty, &nominal, out),
                                indirect: p.indirect,
                            })
                            .collect(),
                        span: *span,
                    });
                    lowered.enums.get_mut(owner).variants.push(id);
                    ids.variants.insert(variant.name.clone(), id);
                }
                enums.insert(name.clone(), vs.iter().map(|v| v.name.clone()).collect());
            }
            // trait のメンバー名の重複は宣言の誤りだが、契約としては一意に保つ。
            // 実装側の過不足は `check_impl` が契約と突き合わせて報告する
            Item::Trait { name, methods, .. } => {
                others.insert(name.clone());
                let owner = nominal.traits[name];
                ids.traits.insert(name.clone(), owner);
                for sig in methods {
                    check_signature_shape(sig, &format!("trait {name}::{}", sig.name), out);
                    let id = lowered.trait_methods.alloc(hir::TraitMethodDecl {
                        name: sig.name.clone(),
                        owner,
                        receiver: sig.receiver,
                        params: sig
                            .params
                            .iter()
                            .map(|p| lower_type(&p.ty, &nominal, out))
                            .collect(),
                        ret: lower_ret(sig, &nominal, out),
                        span: sig.span,
                    });
                    lowered.traits.get_mut(owner).methods.push(id);
                    ids.trait_methods
                        .insert((name.clone(), sig.name.clone()), id);
                }
                traits.insert(
                    name.clone(),
                    methods
                        .iter()
                        .map(|sig| (sig.name.clone(), signature(sig)))
                        .collect(),
                );
            }
            // スロットは trait の窓なので、trait でないものを名指したら
            // 実装を選ぶ手がかりが無い。HIR に載せる前にここで止める
            Item::Effect {
                slot,
                trait_name,
                span,
            } => {
                others.insert(slot.clone());
                match nominal.traits.get(trait_name) {
                    Some(trait_) => {
                        let id = lowered.slots.alloc(hir::SlotDecl {
                            name: slot.clone(),
                            trait_: *trait_,
                            span: *span,
                        });
                        ids.slots.insert(slot.clone(), id);
                    }
                    None => out.push(format!(
                        "effect `{slot}`: `{trait_name}` は trait ではありません"
                    )),
                }
            }
            Item::Fn { sig, span, .. } => {
                others.insert(sig.name.clone());
                fns.insert(sig.name.clone(), signature(sig));
                check_signature_shape(sig, &sig.name, out);
                let id = lowered.callables.alloc(callable_shell(
                    sig,
                    hir::CallableOwner::Free,
                    *span,
                    &nominal,
                    out,
                ));
                ids.fns.insert(sig.name.clone(), id);
                targets.push(Target::Callable(id));
                lowered.bodies.push(hir::BodyId::Callable(id));
            }
            Item::Impl {
                trait_name,
                type_name,
                methods,
                span,
            } => {
                let entry = impls.entry(type_name.clone()).or_default();
                for (sig, _) in methods {
                    entry.push((sig.name.clone(), signature(sig)));
                }
                lower_impl(
                    item,
                    trait_name,
                    type_name,
                    methods,
                    *span,
                    &nominal,
                    &mut ids,
                    &mut lowered,
                    &mut targets,
                    &mut pending,
                    out,
                );
            }
            Item::Test {
                name,
                body: _,
                span,
            } => {
                let id = lowered.tests.alloc(hir::TestDecl {
                    name: name.clone(),
                    body: hir::Body::default(),
                    span: *span,
                });
                targets.push(Target::Test(id));
                lowered.bodies.push(hir::BodyId::Test(id));
            }
        }
    }

    // trait の契約が全部揃ってから実装の表を埋める(`impl` は前方参照できる)
    link_trait_impls(pending, &ids, &mut lowered);

    let decls = Decls {
        structs,
        variants,
        enums,
        ctors,
        fns,
        traits,
        impls,
        slots: collect_slots(program),
        others,
        ids,
        nominal,
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

    (decls, lowered, targets)
}

impl Nominal {
    fn struct_of(&self, name: &str) -> Option<hir::StructId> {
        match self.types.get(name) {
            Some(hir::TypeKind::Struct(id)) => Some(*id),
            _ => None,
        }
    }

    fn enum_of(&self, name: &str) -> Option<hir::EnumId> {
        match self.types.get(name) {
            Some(hir::TypeKind::Enum(id)) => Some(*id),
            _ => None,
        }
    }
}

/// 宣言パスの前半。struct / enum / trait の名前と ID だけを確保する。
///
/// 型注釈は後から宣言された型も名指せるので、注釈を解決する前に全部の
/// 名前が要る(design.md 決定7)。
fn collect_nominal(program: &Program, lowered: &mut hir::Program) -> Nominal {
    let mut nominal = Nominal::default();
    for item in &program.items {
        match item {
            Item::Struct { name, span, .. } => {
                let id = lowered.structs.alloc(hir::StructDecl {
                    name: name.clone(),
                    fields: Vec::new(),
                    span: *span,
                });
                nominal
                    .types
                    .insert(name.clone(), hir::TypeKind::Struct(id));
            }
            Item::Enum { name, span, .. } => {
                let id = lowered.enums.alloc(hir::EnumDecl {
                    name: name.clone(),
                    variants: Vec::new(),
                    span: *span,
                });
                nominal.types.insert(name.clone(), hir::TypeKind::Enum(id));
            }
            Item::Trait { name, span, .. } => {
                let id = lowered.traits.alloc(hir::TraitDecl {
                    name: name.clone(),
                    methods: Vec::new(),
                    span: *span,
                });
                nominal.traits.insert(name.clone(), id);
            }
            _ => {}
        }
    }
    nominal
}

/// 型注釈を HIR の型へ。宣言されていない名前を報告するのはここだけ。
fn lower_type(ty: &Type, nominal: &Nominal, out: &mut Out) -> hir::Type {
    report_unknown(ty, nominal, out);
    lower_known(&known(ty), nominal)
}

/// 注釈の名前が宣言を指しているか。
///
/// 宣言されていない名前を通していた頃は、その型の値がどの宣言のものか誰にも
/// 分からないまま検査を抜けられた。後段が「全ての型は宣言を指す」前提で
/// 書かれる以上、名前を書ける唯一の場所である注釈で閉じる(design.md 決定7)。
fn report_unknown(ty: &Type, nominal: &Nominal, out: &mut Out) {
    match &ty.kind {
        TypeKind::Array(element) => report_unknown(element, nominal, out),
        TypeKind::Named(name) => {
            if builtin(name).is_none()
                && !nominal.types.contains_key(name)
                // 同じ名前を注釈のあちこちで見ても言うのは一度だけ
                && out.unknown_types.insert(name.clone())
            {
                out.push(format!("型 `{name}` は宣言されていません"));
            }
        }
    }
}

/// 分かっている型を HIR の型へ。
///
/// 解決できない名前は `Poison` になる。式の型の名前はどれも注釈か宣言から
/// 来ているので、解決できないものは `report_unknown` が既に報告している
/// (診断が空のまま `Poison` が残ることはない)。
fn lower_known(ty: &KnownType, nominal: &Nominal) -> hir::Type {
    let kind = match &ty.kind {
        KnownKind::Array(element) => hir::TypeKind::Array(Box::new(lower_known(element, nominal))),
        KnownKind::Named(name) => match builtin(name) {
            Some(builtin) => hir::TypeKind::Builtin(builtin),
            None => nominal
                .types
                .get(name)
                .cloned()
                .unwrap_or(hir::TypeKind::Poison),
        },
    };
    hir::Type {
        reference: ty.reference,
        kind,
        optional: ty.optional,
    }
}

/// 型注釈のどこに参照を書けるか(design.md 決定3、tasks 2.3)。
///
/// この版の参照はローカル・引数・戻り値・射影にだけ現れる。所有の内側
/// (struct のフィールド・enum の payload・配列の要素)へは置けず、optional も
/// 付けられない。後で解禁するときに増えるのは受理する位置だけなので、
/// 判定を1本にまとめてある。
enum RefSite {
    /// 引数・戻り値・`let` の注釈。最も外側にだけ参照を書ける
    Outermost,
    /// struct のフィールドと enum の payload。所有値しか置けない
    Owned,
}

fn check_type_shape(ty: &Type, site: &RefSite, position: &str, out: &mut Out) {
    if ty.mode != TypeMode::Owned {
        match site {
            RefSite::Owned => out.push(format!(
                "{position}には参照型を書けません。集約の中に借用を置くのはこの版では未対応です"
            )),
            RefSite::Outermost if ty.optional => {
                out.push(format!("{position}の型{}", optional_reference(&known(ty))));
            }
            RefSite::Outermost => {}
        }
    }
    // 配列の要素は所有の内側。`&[T]` は書けるが `[&T]` は書けない
    if let TypeKind::Array(element) = &ty.kind {
        let inner = format!("{position}の配列要素");
        check_type_shape(element, &RefSite::Owned, &inner, out);
    }
}

/// optional な参照を断る文言(design.md 決定3、tasks 2.3)。注釈から来た型でも
/// `??` が導いた型でも同じ理由なので1本にしてある。
fn optional_reference(ty: &KnownType) -> String {
    format!(" `{ty}` は optional な参照です。参照に後置 `?` は付けられません")
}

/// 宣言の実効戻り値型を HIR へ。注釈の省略は `unit` を返す宣言と同じ意味。
fn lower_ret(sig: &Sig, nominal: &Nominal, out: &mut Out) -> hir::Type {
    match &sig.ret {
        Some(ty) => lower_type(ty, nominal, out),
        None => hir::Type::unit(),
    }
}

/// 組み込み型を名乗る名前。正準名は修飾されているので末尾だけを見る。
fn builtin(name: &str) -> Option<hir::Builtin> {
    match short_name(name) {
        "int" => Some(hir::Builtin::Int),
        "bool" => Some(hir::Builtin::Bool),
        "str" => Some(hir::Builtin::Str),
        "unit" => Some(hir::Builtin::Unit),
        _ => None,
    }
}

/// 本体が空の callable。署名だけを下ろす。`self` と引数の局所束縛、および
/// 本体の式は、本体を検査するときに同じ走査で確保する(design.md 決定7)。
fn callable_shell(
    sig: &Sig,
    owner: hir::CallableOwner,
    span: Span,
    nominal: &Nominal,
    out: &mut Out,
) -> hir::Callable {
    hir::Callable {
        name: sig.name.clone(),
        owner,
        receiver: sig.receiver,
        params: Vec::new(),
        ret: lower_ret(sig, nominal, out),
        body: hir::Body::default(),
        span,
    }
}

/// 署名の引数と戻り値に書かれた参照の位置を見る。参照そのものは受理する位置
/// なので、見るのは入れ子と optional だけ(tasks 2.3)。
fn check_signature_shape(sig: &Sig, ctx: &str, out: &mut Out) {
    for param in &sig.params {
        check_type_shape(
            &param.ty,
            &RefSite::Outermost,
            &format!("{ctx} の引数 `{}`", param.name),
            out,
        );
    }
    if let Some(ret) = &sig.ret {
        check_type_shape(ret, &RefSite::Outermost, &format!("{ctx} の戻り値"), out);
    }
}

/// `impl` を HIR へ。実装先が struct でなければ置き場所が無いので、本体の
/// 検査だけ続けて HIR には残さない(`Target::Discard`)。
///
/// 契約メソッドから実装本体への表はここでは埋めない。`impl` は trait の宣言より
/// 前に書けるので、trait の契約が全部揃ってから `link_trait_impls` が埋める。
#[allow(clippy::too_many_arguments)]
fn lower_impl(
    item: &Item,
    trait_name: &Option<String>,
    type_name: &str,
    methods: &[(Sig, Vec<Expr>)],
    span: Span,
    nominal: &Nominal,
    ids: &mut Ids,
    lowered: &mut hir::Program,
    targets: &mut Vec<Target>,
    pending: &mut Vec<PendingImpl>,
    out: &mut Out,
) {
    let type_ = nominal.struct_of(type_name);
    // trait 側の誤りは `check_impl` が契約と突き合わせて報告する。
    // inherent な `impl` はそこを通らないので、実装先だけここで見る
    if type_.is_none() && trait_name.is_none() {
        out.span = Some(item.span());
        out.push(format!(
            "impl {type_name}: `{type_name}` は struct ではありません"
        ));
    }
    let owner = match (type_, trait_name) {
        (Some(type_), Some(trait_name)) => match nominal.traits.get(trait_name) {
            Some(trait_) => {
                let id = lowered.trait_impls.alloc(hir::TraitImplDecl {
                    trait_: *trait_,
                    type_,
                    methods: BTreeMap::new(),
                    span,
                });
                ids.trait_impls
                    .insert((type_name.to_string(), trait_name.clone()), id);
                Some(hir::CallableOwner::TraitImpl(id))
            }
            None => None,
        },
        (Some(type_), None) => Some(hir::CallableOwner::Inherent(type_)),
        (None, _) => None,
    };
    for (sig, _) in methods {
        check_signature_shape(sig, &format!("impl {type_name}::{}", sig.name), out);
        let Some(owner) = owner else {
            targets.push(Target::Discard);
            continue;
        };
        let id = lowered
            .callables
            .alloc(callable_shell(sig, owner, sig.span, nominal, out));
        ids.methods
            .entry((type_name.to_string(), sig.name.clone()))
            .or_default()
            .push(id);
        if let (hir::CallableOwner::TraitImpl(impl_), Some(trait_name)) = (owner, trait_name) {
            pending.push(PendingImpl {
                impl_,
                trait_name: trait_name.clone(),
                method: sig.name.clone(),
                callable: id,
            });
        }
        targets.push(Target::Callable(id));
        lowered.bodies.push(hir::BodyId::Callable(id));
    }
}

/// trait の契約が揃うのを待っている実装メソッド1つ。
struct PendingImpl {
    impl_: hir::TraitImplId,
    trait_name: String,
    method: String,
    callable: hir::CallableId,
}

/// 契約メソッド → 実装本体の表を埋める。スロット呼び出しはこの表を1手で引くので、
/// 実行時に名前で候補を探す必要がなくなる(design.md 決定6)。
///
/// 契約に無いメソッドは `check_impl` が報告するので、ここでは黙って落とす。
fn link_trait_impls(pending: Vec<PendingImpl>, ids: &Ids, lowered: &mut hir::Program) {
    for entry in pending {
        if let Some(method) = ids
            .trait_methods
            .get(&(entry.trait_name, entry.method))
            .copied()
        {
            lowered
                .trait_impls
                .get_mut(entry.impl_)
                .methods
                .insert(method, entry.callable);
        }
    }
}

// ---------------------------------------------------------------------------
// 所有の内包グラフ(design.md 決定10)
// ---------------------------------------------------------------------------

/// 型の内包グラフの節点。値の型を持つ宣言だけが節点になる。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Node {
    Struct(hir::StructId),
    Enum(hir::EnumId),
}

/// 「A の中に B の値がそのまま並ぶ」1本の辺。`indirect` な宣言は辺を作らない
/// ので、間接化された再帰は循環にならない(design.md 決定10)。
struct Edge {
    to: Node,
    /// 宣言を指す位置。`indirect` を足すべき場所でもある
    span: Span,
    /// 診断に出す辺の綴り
    label: String,
}

/// 内包グラフを作り、`indirect` を1本も含まない循環を全部報告する。
///
/// 宣言の並びと辺の並びは宣言順なので、同じプログラムからは常に同じ診断が
/// 同じ順で出る。
fn check_type_cycles(lowered: &hir::Program, out: &mut Out) {
    let mut graph: BTreeMap<Node, Vec<Edge>> = BTreeMap::new();
    for (id, decl) in lowered.structs.iter() {
        let edges = graph.entry(Node::Struct(id)).or_default();
        for field in &decl.fields {
            let field = &lowered.fields[*field];
            if field.indirect {
                continue;
            }
            for to in inline_targets(&field.ty) {
                edges.push(Edge {
                    to,
                    span: field.span,
                    label: format!(
                        "`{}` のフィールド `{}` が `{}` を直接持っています",
                        short_name(&decl.name),
                        field.name,
                        show_node(lowered, to)
                    ),
                });
            }
        }
    }
    for (id, decl) in lowered.enums.iter() {
        let edges = graph.entry(Node::Enum(id)).or_default();
        for variant in &decl.variants {
            let variant = &lowered.variants[*variant];
            for (position, payload) in variant.payload.iter().enumerate() {
                if payload.indirect {
                    continue;
                }
                for to in inline_targets(&payload.ty) {
                    edges.push(Edge {
                        to,
                        span: variant.span,
                        label: format!(
                            "`{}` の variant `{}` の第 {} payload が `{}` を直接持っています",
                            short_name(&decl.name),
                            short_name(&variant.name),
                            position + 1,
                            show_node(lowered, to)
                        ),
                    });
                }
            }
        }
    }

    let mut visited: BTreeSet<Node> = BTreeSet::new();
    let mut on_stack: Vec<Node> = Vec::new();
    let mut path: Vec<&Edge> = Vec::new();
    let mut reported: BTreeSet<Node> = BTreeSet::new();
    let nodes: Vec<Node> = graph.keys().copied().collect();
    for node in nodes {
        visit_containment(
            node,
            &graph,
            lowered,
            &mut visited,
            &mut on_stack,
            &mut path,
            &mut reported,
            out,
        );
    }
}

/// その型の値の中に**そのまま並ぶ**名前付き型。optional と配列は所有の内側
/// なので辿り、参照は所有ではないので辿らない(design.md 決定10)。
fn inline_targets(ty: &hir::Type) -> Vec<Node> {
    if ty.reference.is_some() {
        return Vec::new();
    }
    match &ty.kind {
        hir::TypeKind::Struct(id) => vec![Node::Struct(*id)],
        hir::TypeKind::Enum(id) => vec![Node::Enum(*id)],
        hir::TypeKind::Array(element) => inline_targets(element),
        hir::TypeKind::Builtin(_) | hir::TypeKind::Poison => Vec::new(),
    }
}

fn show_node(lowered: &hir::Program, node: Node) -> String {
    short_name(match node {
        Node::Struct(id) => &lowered.structs[id].name,
        Node::Enum(id) => &lowered.enums[id].name,
    })
    .to_string()
}

fn node_span(lowered: &hir::Program, node: Node) -> Span {
    match node {
        Node::Struct(id) => lowered.structs[id].span,
        Node::Enum(id) => lowered.enums[id].span,
    }
}

/// 深さ優先で1節点を訪ねる。いま辿っている経路の上の節点へ戻る辺が循環。
#[allow(clippy::too_many_arguments)]
fn visit_containment<'g>(
    node: Node,
    graph: &'g BTreeMap<Node, Vec<Edge>>,
    lowered: &hir::Program,
    visited: &mut BTreeSet<Node>,
    on_stack: &mut Vec<Node>,
    path: &mut Vec<&'g Edge>,
    reported: &mut BTreeSet<Node>,
    out: &mut Out,
) {
    if !visited.insert(node) {
        return;
    }
    on_stack.push(node);
    for edge in graph.get(&node).into_iter().flatten() {
        if let Some(start) = on_stack.iter().position(|n| *n == edge.to) {
            // 同じ強連結成分は一度だけ報告する
            if reported.contains(&edge.to) {
                continue;
            }
            let cycle: Vec<&Edge> = path[start..].iter().copied().chain([edge]).collect();
            report_cycle(lowered, edge.to, &cycle, out);
            reported.extend(on_stack[start..].iter().copied());
            continue;
        }
        path.push(edge);
        visit_containment(
            edge.to, graph, lowered, visited, on_stack, path, reported, out,
        );
        path.pop();
    }
    on_stack.pop();
}

fn report_cycle(lowered: &hir::Program, start: Node, cycle: &[&Edge], out: &mut Out) {
    let related = cycle
        .iter()
        .map(|edge| Diag::at(edge.span, edge.label.clone()).label("この所有エッジ"))
        .collect();
    out.diagnostics.push(
        Diag::at(
            node_span(lowered, start),
            format!(
                "型 `{}` の所有が循環しています。値の大きさが決まりません",
                show_node(lowered, start)
            ),
        )
        .label("ここから始まる循環")
        .help("循環の辺のどれかに `indirect` を付けて所有を間接化してください(同じ型が別の循環にも入っていれば、それも順に出ます)")
        .related(related),
    );
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
        // レシーバの所有モードまで契約と一致していなければならない
        // (method-call-type-checking spec「Trait receiver mismatch」)
        if actual.receiver != declared.receiver {
            out.push(format!(
                "{ctx}: `{}` のレシーバは {} ですが、{} を宣言しています",
                sig.name,
                show_receiver(declared.receiver),
                show_receiver(actual.receiver)
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

/// 診断に出すレシーバの綴り。
fn show_receiver(receiver: Option<ReceiverMode>) -> &'static str {
    match receiver {
        None => "レシーバ無し",
        Some(ReceiverMode::Owned) => "`self`",
        Some(ReceiverMode::Shared) => "`&self`",
        Some(ReceiverMode::Mutable) => "`&mut self`",
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

/// 宛先の型へ値の型が収まるか。厳密一致か、同名の非 optional から optional への
/// 一方向の注入だけ(design.md 決定1)。値の側の推論型は変えない。
fn fits(actual: &KnownType, expected: &KnownType) -> bool {
    actual == expected
        || (actual.kind == expected.kind
            // 借用の強さは注入では変わらない。`T` と `&T` は別の宛先
            && actual.reference == expected.reference
            && !actual.optional
            && expected.optional)
}

/// 期待型のある位置。どこが期待しているかで診断の文言だけが変わる。
enum Site<'a> {
    /// 「{what}は `{expected}` ですが、`{actual}` です」
    What(&'a str),
    /// 呼び出しの実引数
    Arg { callee: &'a str, index: usize },
    /// メソッド呼び出しのレシーバ(tasks 5.2)
    Receiver { callee: &'a str },
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
            Site::Receiver { callee } => format!(
                "{ctx}: `{callee}` のレシーバは `{expected}` ですが、`{actual}` を渡しています"
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

/// 走査から戻るもの。検査側の分類と、arena に入った式の ID。
///
/// どんな式でも ID を1つ持つので、親は子を必ず指せる。型を出せなかった式は
/// `ExprResult::Poison` として arena に残るが、診断を伴わずに外へ出ることは
/// ない(`hir::Program::poisoned`)。
#[derive(Clone)]
struct Checked {
    outcome: Outcome,
    id: hir::ExprId,
}

/// `walk_kind` が返すもの。span と結果分類を付けて arena へ入れるのは `walk`。
struct Lowered {
    outcome: Outcome,
    kind: hir::ExprKind,
}

fn lowered(outcome: Outcome, kind: hir::ExprKind) -> Lowered {
    Lowered { outcome, kind }
}

fn typed(ty: KnownType, kind: hir::ExprKind) -> Lowered {
    lowered(Outcome::Typed(ty), kind)
}

/// `unit` を産む式。`let` / 代入 / `assert` / ループ / 空ブロックの結果。
fn produces_unit(kind: hir::ExprKind) -> Lowered {
    typed(plain("unit"), kind)
}

/// 型を出せなかった式。理由の診断は既に出ている(design.md 決定3)。
fn poison() -> Lowered {
    lowered(Outcome::Poisoned, hir::ExprKind::Poison)
}

/// 部分式が制御を抜けたので、この式は値を産まない。実行されるのはその部分式
/// までなので、それを評価するだけの形へ畳む。
fn diverged(child: hir::ExprId) -> Lowered {
    lowered(Outcome::Diverges, hir::ExprKind::Block(vec![child]))
}

/// 局所束縛を1つ確保する。型は HIR の型へ下ろして持たせる。
fn alloc_local(
    name: &str,
    ty: Option<&KnownType>,
    span: Span,
    decls: &Decls,
    out: &mut Out,
) -> hir::LocalId {
    alloc_binding(name, ty, false, span, decls, out)
}

/// 可変性まで指定して局所束縛を確保する。`let mut` だけが真を渡す
/// (design.md 決定2)。
fn alloc_binding(
    name: &str,
    ty: Option<&KnownType>,
    mutable: bool,
    span: Span,
    decls: &Decls,
    out: &mut Out,
) -> hir::LocalId {
    let ty = ty.map(|ty| lower_known(ty, &decls.nominal));
    out.body.alloc_local(hir::LocalDecl {
        name: name.to_string(),
        ty,
        mutable,
        span,
    })
}

/// 1つの宣言の本体を検査して HIR へ下ろす。
///
/// 宣言パスが確保した置き場所から本体を借り出し、`self` と引数の局所束縛を
/// 確保してから走査に入る。式は `out.body` へ溜まる。
#[allow(clippy::too_many_arguments)]
fn check_body(
    body: &[Expr],
    sig: Option<&Sig>,
    receiver: Option<KnownType>,
    decls: &Decls,
    ctx: &str,
    ret: &KnownType,
    target: Target,
    lowered: &mut hir::Program,
    out: &mut Out,
) {
    let borrowed = match target {
        Target::Callable(id) => std::mem::take(&mut lowered.callables.get_mut(id).body),
        Target::Test(id) => std::mem::take(&mut lowered.tests.get_mut(id).body),
        // 置き場所を決められなかった宣言。検査は続けるが下ろした先は捨てる
        Target::Discard => hir::Body::default(),
    };
    let outer = std::mem::replace(&mut out.body, borrowed);

    let span = sig.map_or(ret_span(out), |sig| sig.span);
    let mut locals = Locals::new();
    // `self` を先に確保するので `LocalId` は self → 宣言順の引数の順になる
    let receiver_local = receiver.map(|ty| {
        let id = alloc_local("self", Some(&ty), span, decls, out);
        locals.insert("self".to_string(), Binding::Value(Some(ty), id));
        id
    });
    let params: Vec<hir::LocalId> = sig
        .map(|sig| {
            sig.params
                .iter()
                .map(|p| {
                    let ty = known(&p.ty);
                    let id = alloc_local(&p.name, Some(&ty), span, decls, out);
                    locals.insert(p.name.clone(), Binding::Value(Some(ty), id));
                    id
                })
                .collect()
        })
        .unwrap_or_default();

    let cx = Cx { decls, ctx, ret };
    // 本体は値ベースなので最後の式も戻り値。明示 `return` と同じ位置で照合する
    let (_, root) = sequence(body, Some((ret, &Site::Return)), &cx, &mut locals, out);
    out.body.root = root;
    out.body.receiver = receiver_local;

    let filled = std::mem::replace(&mut out.body, outer);
    match target {
        Target::Callable(id) => {
            let callable = lowered.callables.get_mut(id);
            callable.params = params;
            callable.body = filled;
        }
        Target::Test(id) => lowered.tests.get_mut(id).body = filled,
        Target::Discard => {}
    }
}

/// 署名を持たない本体(`test`)の局所束縛に使う位置。宣言全体を指す
fn ret_span(out: &Out) -> Span {
    out.span.expect("宣言の走査に入る前に span が入っている")
}

/// 式の列。値になるのは最後の式だけで、手前の式は値を産んでも捨てる
/// (design.md 決定5)。空の列は値を産まないので `unit`。
fn sequence(
    body: &[Expr],
    expected: Expect,
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> (Outcome, Vec<hir::ExprId>) {
    let Some((last, init)) = body.split_last() else {
        return (Outcome::Typed(plain("unit")), Vec::new());
    };
    // 手前の式のどれかが制御を抜けるなら、この列は最後まで続かない。
    // ただし到達しない式も宣言の検査対象なので走査は続ける
    let mut ids = Vec::with_capacity(body.len());
    let mut diverged = false;
    for e in init {
        let checked = synth(e, cx, locals, out);
        if matches!(checked.outcome, Outcome::Diverges) {
            diverged = true;
        }
        ids.push(checked.id);
    }
    let result = walk(last, expected, cx, locals, out);
    ids.push(result.id);
    let outcome = if diverged {
        Outcome::Diverges
    } else {
        result.outcome
    };
    (outcome, ids)
}

fn block(body: &[Expr], expected: Expect, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Lowered {
    let (outcome, ids) = sequence(body, expected, cx, locals, out);
    lowered(outcome, hir::ExprKind::Block(ids))
}

/// 期待型の無い位置の式。自分の型を推論するだけ。
fn synth(e: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Checked {
    walk(e, None, cx, locals, out)
}

/// 検査中の式を診断の位置にして、種類ごとの規則へ回す。部分木から戻ったら
/// 外側の式へ span を戻す(戻さないと、子を見た後の親の診断が子の位置を指す)。
///
/// 期待型との境界の照合はここ1箇所。`Diverges` はそこを通らないので照合せず、
/// `Poisoned` は既に説明済みなので重ねない(design.md 決定3・4)。
/// 最後に、下ろした形と結果分類と span を1つの HIR 式として arena へ入れる。
fn walk(e: &Expr, expected: Expect, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Checked {
    let before = out.count();
    let outer = out.span.replace(e.span);
    let Lowered { outcome, kind } = walk_kind(e, expected, cx, locals, out);
    out.span = outer;
    let outcome = match (&outcome, expected) {
        (Outcome::Typed(actual), Some((expected, site))) if !fits(actual, expected) => {
            out.push_at(e.span, site.message(cx.ctx, expected, &actual.to_string()));
            Outcome::Poisoned
        }
        _ => outcome,
    };
    // 検査を閉じる網。型を出せなかった式は理由の診断を伴っていなければならず、
    // 伴わないまま検査が成功しかけたら `check_and_lower` が最後にここを指して落とす
    // (design.md 決定3、リスク「規則の抜けが Unknown を再生する」)
    if matches!(outcome, Outcome::Poisoned) {
        out.note_unexplained(before, e.span);
    }
    let result = match &outcome {
        Outcome::Typed(ty) => hir::ExprResult::Value(lower_known(ty, &cx.decls.nominal)),
        Outcome::Diverges => hir::ExprResult::Diverges,
        Outcome::Poisoned => hir::ExprResult::Poison,
    };
    let id = out.body.alloc_expr(hir::Expr {
        result,
        kind,
        span: e.span,
    });
    Checked { outcome, id }
}

fn walk_kind(e: &Expr, expected: Expect, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Lowered {
    let decls = cx.decls;
    let ctx = cx.ctx;
    match &e.kind {
        ExprKind::Access { mode, place } => access(*mode, place, cx, locals, out),

        ExprKind::Int(n) => typed(plain("int"), hir::ExprKind::Int(*n)),
        ExprKind::Str(s) => typed(plain("str"), hir::ExprKind::Str(s.clone())),
        ExprKind::Bool(b) => typed(plain("bool"), hir::ExprKind::Bool(*b)),

        // `nil` は自分だけでは nominal 型を持たない。期待型が optional の
        // ときだけその型になる(design.md 決定7)
        ExprKind::Nil => match expected {
            Some((expected, _)) if expected.optional => typed(expected.clone(), hir::ExprKind::Nil),
            Some((expected, site)) => {
                out.push_at(e.span, site.message(ctx, expected, "nil"));
                lowered(Outcome::Poisoned, hir::ExprKind::Nil)
            }
            None => lowered(Outcome::Poisoned, hir::ExprKind::Nil),
        },

        ExprKind::Ident(name) => {
            let before = out.count();
            check_bare(name, decls, locals, ctx, out);
            match locals.get(name) {
                Some(Binding::Value(Some(ty), local)) => {
                    typed(ty.clone(), hir::ExprKind::Local(*local))
                }
                // 型の分からないローカル。束縛した側が既に診断している
                Some(Binding::Value(None, local)) => {
                    lowered(Outcome::Poisoned, hir::ExprKind::Local(*local))
                }
                // スロットは trait の窓であって値ではない
                Some(Binding::Slot(_)) => {
                    out.push(format!("{ctx}: `{name}` はスロットなので値になりません"));
                    poison()
                }
                None => {
                    let value = bare_value(name, decls);
                    if matches!(value.outcome, Outcome::Poisoned) {
                        out.explain(before, format!("{ctx}: `{name}` は値として読めません"));
                    }
                    value
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
                return poison();
            };
            if !decls.enums.contains_key(enum_name) {
                out.push(format!(
                    "{ctx}: `{enum_name}::{variant}` は値として読めません"
                ));
                return poison();
            }
            match payload_of(enum_name, variant, decls) {
                None => {
                    out.push(format!(
                        "{ctx}: `{enum_name}::{variant}` は `{enum_name}` の variant ではありません"
                    ));
                    poison()
                }
                Some(payload) if !payload.is_empty() => {
                    out.push(format!(
                        "{ctx}: `{enum_name}::{variant}` は payload を {} 個取ります。`{enum_name}::{variant}(...)` で生成してください",
                        payload.len()
                    ));
                    poison()
                }
                Some(_) => typed(
                    plain(enum_name),
                    hir::ExprKind::Variant(variant_id(enum_name, variant, decls)),
                ),
            }
        }

        ExprKind::Field(recv, field) => field_read(recv, field, false, cx, locals, out),
        ExprKind::OptionalField(recv, field) => field_read(recv, field, true, cx, locals, out),

        ExprKind::Call(callee, args) => call(callee, args, cx, locals, out),

        ExprKind::StructLit { name, fields } => {
            check_literal(name, fields, decls, ctx, out);
            let mut given = Vec::with_capacity(fields.len());
            for (field, value) in fields {
                match declared_field(name, field, decls) {
                    Some(declared) => {
                        let site = Site::Field {
                            type_name: name,
                            field,
                        };
                        let checked = walk(value, Some((&declared, &site)), cx, locals, out);
                        if let Some(id) = decls.ids.fields.get(&(name.clone(), field.clone())) {
                            given.push((*id, checked.id));
                        }
                    }
                    // 宣言に無いフィールドは `check_literal` が報告済み
                    None => {
                        synth(value, cx, locals, out);
                    }
                }
            }
            match decls.ids.structs.get(name) {
                Some(struct_) => typed(
                    plain(name),
                    hir::ExprKind::StructLit {
                        struct_: *struct_,
                        fields: given,
                    },
                ),
                // 宣言されていない struct。`check_literal` が報告済み
                None => lowered(Outcome::Typed(plain(name)), hir::ExprKind::Poison),
            }
        }

        // 注釈があれば初期化子の期待型になり、そのまま束縛の型として固定される。
        // 無ければ初期化子の推論型だけが束縛の型(design.md 決定2)
        ExprKind::Let {
            name,
            mutable,
            annotation,
            value,
        } => {
            if let Some(annotation) = annotation {
                report_unknown(annotation, &decls.nominal, out);
                check_type_shape(
                    annotation,
                    &RefSite::Outermost,
                    &format!("{ctx}: 局所束縛 `{name}`"),
                    out,
                );
            }
            let (ty, value_id) = match annotation.as_ref().map(known) {
                Some(declared) => {
                    let what = format!("`{name}` の初期化子");
                    let checked = walk(
                        value,
                        Some((&declared, &Site::What(&what))),
                        cx,
                        locals,
                        out,
                    );
                    (Some(declared), checked.id)
                }
                None => {
                    let before = out.count();
                    let checked = synth(value, cx, locals, out);
                    let ty = match &checked.outcome {
                        Outcome::Typed(ty) => Some(ty.clone()),
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
                    };
                    (ty, checked.id)
                }
            };
            let local = alloc_binding(name, ty.as_ref(), *mutable, e.span, decls, out);
            locals.insert(name.clone(), Binding::Value(ty, local));
            produces_unit(hir::ExprKind::Let {
                local,
                value: value_id,
            })
        }

        ExprKind::Assign { target, value } => produces_unit(assign(target, value, cx, locals, out)),

        ExprKind::Unary(UnOp::Neg, inner) => {
            let int = plain("int");
            let site = Site::What("単項 `-` の被演算子");
            let checked = walk(inner, Some((&int, &site)), cx, locals, out);
            typed(int, hir::ExprKind::Neg(checked.id))
        }

        ExprKind::Binary { op, lhs, rhs } => match op {
            // 評価器の整数演算をそのまま静的にする。暗黙変換も文字列連結も無い
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                let int = plain("int");
                let sym = symbol(*op);
                let left = Site::What(&format!("`{sym}` の左辺"));
                let lhs = walk(lhs, Some((&int, &left)), cx, locals, out);
                let right = Site::What(&format!("`{sym}` の右辺"));
                let rhs = walk(rhs, Some((&int, &right)), cx, locals, out);
                typed(
                    int,
                    hir::ExprKind::Arith {
                        op: arith(*op),
                        lhs: lhs.id,
                        rhs: rhs.id,
                    },
                )
            }
            BinOp::Eq => equality(lhs, rhs, cx, locals, out),
            BinOp::Coalesce => coalesce(lhs, rhs, cx, locals, out),
        },

        // 値を運ぶかに関わらず制御を抜ける。運ぶ値は宣言の実効戻り値と照合する
        ExprKind::Return(value) => {
            let id = value
                .as_ref()
                .map(|value| walk(value, Some((cx.ret, &Site::Return)), cx, locals, out).id);
            lowered(Outcome::Diverges, hir::ExprKind::Return(id))
        }

        ExprKind::Assert(inner) => {
            let site = Site::What("`assert` の対象");
            let checked = walk(inner, Some((&plain("bool"), &site)), cx, locals, out);
            produces_unit(hir::ExprKind::Assert(checked.id))
        }

        // 期待要素型があればそれを各要素へ配る。無ければ最初に型の分かった要素を
        // 以降の要素の期待型にする(design.md 決定4)
        ExprKind::Array(items) => {
            let contextual = want(expected).and_then(KnownType::element).cloned();
            let mut element = contextual.clone();
            let mut poisoned = false;
            let mut ids = Vec::with_capacity(items.len());
            for (n, item) in items.iter().enumerate() {
                let what = format!("配列の第 {} 要素", n + 1);
                let site = Site::What(&what);
                let checked = match &element {
                    Some(element) => walk(item, Some((element, &site)), cx, locals, out),
                    None => synth(item, cx, locals, out),
                };
                ids.push(checked.id);
                match checked.outcome {
                    Outcome::Typed(ty) if element.is_none() => element = Some(ty),
                    Outcome::Typed(_) | Outcome::Diverges => {}
                    Outcome::Poisoned => poisoned = true,
                }
            }
            let kind = hir::ExprKind::Array(ids);
            match (want(expected), &contextual, element) {
                // 期待型のある位置は生成の境界。要素は個別に照合済みなので、
                // 配列全体はその期待型として適合する
                (Some(expected), Some(_), _) => typed(expected.clone(), kind),
                // 型の分かる要素が一つも無い。空配列も、`nil` だけの配列もここ。
                // 後の使用から遡らず、この場で要素型を要求する
                (_, _, None) => {
                    out.push(format!(
                        "{ctx}: 配列の要素型が決まりません。宛先の型か `let xs: [T] = ...` の注釈で要素型を与えてください"
                    ));
                    lowered(Outcome::Poisoned, kind)
                }
                // どれかの要素が基準と食い違った。理由はその要素の位置にある
                (_, _, Some(_)) if poisoned => lowered(Outcome::Poisoned, kind),
                (_, _, Some(element)) => typed(array_of(element), kind),
            }
        }

        // 第二級ブロックなので、内側の `let` は外へ漏れる(`requirement::scan`
        // と同じ規則)。枝へ入るときだけ `locals` を複製する
        ExprKind::Block(body) => block(body, expected, cx, locals, out),

        // 対象は既知の非 optional な enum で、arm はその全 variant を一度ずつ。
        // 結果型は期待型、無ければ最初に型の分かった arm を基準にする
        // (design.md 決定3・5)
        ExprKind::Match { subject, arms } => match_expr(expected, subject, arms, cx, locals, out),

        ExprKind::Head { head, body, orelse } => head_expr(
            expected,
            head,
            body,
            orelse.as_deref(),
            e.span,
            cx,
            locals,
            out,
        ),
    }
}

/// 限定 variant が指す宣言の ID。宣言済み enum の variant だと分かった後に引く。
fn variant_id(enum_name: &str, variant: &str, decls: &Decls) -> hir::VariantId {
    let canonical =
        declared_variant(enum_name, variant, decls).expect("宣言済み variant だと分かっている");
    decls.ids.variants[canonical]
}

fn arith(op: BinOp) -> hir::ArithOp {
    match op {
        BinOp::Add => hir::ArithOp::Add,
        BinOp::Sub => hir::ArithOp::Sub,
        BinOp::Mul => hir::ArithOp::Mul,
        BinOp::Div => hir::ArithOp::Div,
        BinOp::Eq | BinOp::Coalesce => unreachable!("算術だけを回す"),
    }
}

/// ローカルに隠されていない裸の名前の値。フィールド0個の struct と payload 0個の
/// enum variant だけが名前そのままで値になる(`check_bare` が診断する側)。
fn bare_value(name: &str, decls: &Decls) -> Lowered {
    if let Some(enum_name) = decls.variants.get(name) {
        return if decls.ctors[name].params.is_empty() {
            typed(
                plain(enum_name),
                hir::ExprKind::Variant(decls.ids.variants[name]),
            )
        } else {
            poison()
        };
    }
    match decls.structs.get(name) {
        Some(fields) if fields.is_empty() => typed(
            plain(name),
            hir::ExprKind::UnitStruct(decls.ids.structs[name]),
        ),
        _ => poison(),
    }
}

/// `&place` / `&mut place` / `move place`。
///
/// 修飾は完成した場所の型に掛かるので、期待型は場所へ配らずに、作った参照を
/// `walk` の出口で照合する。借用がいつまで生きるか・移動してよいかはここでは
/// 見ない。それは所有権解析の仕事(design.md 決定3)。
fn access(mode: AccessMode, place: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Lowered {
    let checked = synth(place, cx, locals, out);
    let ty = match &checked.outcome {
        Outcome::Typed(ty) => ty.clone(),
        // 場所が値を産まないなら修飾も起きない。理由は場所の側にある
        Outcome::Diverges => return diverged(checked.id),
        Outcome::Poisoned => return poison(),
    };
    let kind = hir::ExprKind::Access {
        mode,
        place: checked.id,
    };
    let reference = match mode {
        // `move` は所有をそのまま運ぶので静的型は変わらない
        AccessMode::Move => return lowered(Outcome::Typed(ty), kind),
        AccessMode::Shared => hir::RefKind::Shared,
        AccessMode::Mutable => hir::RefKind::Mutable,
    };
    // optional な参照は作れない(design.md 決定3、tasks 2.3)
    if ty.optional {
        out.push(format!(
            "{}: optional な値 `{ty}` への参照は作れません。先に `??` で展開してください",
            cx.ctx
        ));
        return lowered(Outcome::Poisoned, kind);
    }
    typed(
        KnownType {
            reference: Some(reference),
            ..ty
        },
        kind,
    )
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
) -> Lowered {
    let ctx = cx.ctx;
    let receiver = synth(recv, cx, locals, out);
    let ty = match &receiver.outcome {
        Outcome::Typed(ty) => ty.clone(),
        // レシーバが値を産まないなら読みも起きない。理由はレシーバ側にある
        Outcome::Diverges => return diverged(receiver.id),
        Outcome::Poisoned => return poison(),
    };
    if optional {
        if !ty.optional {
            out.push(format!(
                "{ctx}: `.?{field}` のレシーバは optional である必要がありますが、`{ty}` です"
            ));
            return poison();
        }
    } else if ty.optional {
        out.push(format!(
            "{ctx}: optional 型 `{ty}` から `{field}` を読むには `.?{field}` を使うか、先に `??` で展開してください"
        ));
        return poison();
    }
    let Some(type_name) = ty.name() else {
        out.push(non_struct_message(ctx, &ty, field, optional));
        return poison();
    };
    let Some(declared) = cx.decls.structs.get(type_name) else {
        out.push(non_struct_message(ctx, &ty, field, optional));
        return poison();
    };
    let Some(declared) = declared.get(field) else {
        out.push(if optional {
            format!("{ctx}: `{type_name}` にフィールド `{field}` はありません")
        } else {
            format!("{ctx}: `{ty}` にフィールド `{field}` はありません")
        });
        return poison();
    };
    let mut result = declared.clone();
    // optional は1 bit。宣言型が既に `T?` でも `S?.?field` は `T?` のまま
    if optional {
        result.optional = true;
    }
    typed(
        result,
        hir::ExprKind::Field {
            recv: receiver.id,
            field: cx.decls.ids.fields[&(type_name.to_string(), field.to_string())],
            optional,
        },
    )
}

fn non_struct_message(ctx: &str, ty: &KnownType, field: &str, optional: bool) -> String {
    if optional {
        format!("{ctx}: `{ty}` の中身は struct ではないので `.?{field}` を読めません")
    } else {
        format!("{ctx}: `{ty}` は struct ではないので `{field}` を読めません")
    }
}

/// 代入。宛先の型が分かるときだけ値を照合する。束縛の型は宣言時に決まるので、
/// 後の代入では変えない(design.md 決定4)。
///
/// 宛先は局所束縛か宣言フィールドのどちらかでなければならない。実行時に必ず
/// 失敗する左辺(スロット・struct でない値・呼び出しの結果)はここで止める。
fn assign(
    target: &Expr,
    value: &Expr,
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> hir::ExprKind {
    let ctx = cx.ctx;
    match &target.kind {
        // 代入先の裸の名前は書き込み先であって値の読みではない
        ExprKind::Ident(name) => match locals.get(name).cloned() {
            Some(Binding::Value(declared, local)) => {
                let value = match &declared {
                    Some(declared) => {
                        let what = format!("`{name}` への代入");
                        walk(value, Some((declared, &Site::What(&what))), cx, locals, out)
                    }
                    None => synth(value, cx, locals, out),
                };
                hir::ExprKind::AssignLocal {
                    local,
                    value: value.id,
                }
            }
            Some(Binding::Slot(_)) => {
                synth(value, cx, locals, out);
                out.push(format!("{ctx}: `{name}` はスロットなので代入できません"));
                hir::ExprKind::Poison
            }
            // トップレベルのスロットは `locals` に居ないが、書き込み先でもない
            None if cx.decls.slots.is_slot(name) => {
                synth(value, cx, locals, out);
                out.push(format!("{ctx}: `{name}` はスロットなので代入できません"));
                hir::ExprKind::Poison
            }
            // 検査器が知らない名前への代入は新しい束縛を作る。読み出しは
            // 「値として読めません」になるので、この束縛は誰にも読めない
            None => {
                let value = synth(value, cx, locals, out);
                let local = alloc_local(name, value.outcome.ty(), target.span, cx.decls, out);
                hir::ExprKind::AssignLocal {
                    local,
                    value: value.id,
                }
            }
        },
        ExprKind::Field(recv, field) => {
            let receiver = synth(recv, cx, locals, out);
            let ty = receiver.outcome.ty().cloned();
            // 値の照合は、レシーバが非 optional と分かるときだけ(既存の規則)。
            // 宛先のフィールドは optional の中身でも同じ宣言を指す
            let checkable = ty
                .as_ref()
                .filter(|ty| !ty.optional)
                .and_then(KnownType::name)
                .map(str::to_string);
            let declared = checkable
                .as_deref()
                .and_then(|owner| declared_field(owner, field, cx.decls));
            let value = match (checkable.as_deref(), declared) {
                (Some(type_name), Some(declared)) => {
                    let site = Site::Field { type_name, field };
                    walk(value, Some((&declared, &site)), cx, locals, out)
                }
                _ => synth(value, cx, locals, out),
            };
            let owner = ty.as_ref().and_then(KnownType::name);
            let resolved = owner.and_then(|owner| {
                cx.decls
                    .ids
                    .fields
                    .get(&(owner.to_string(), field.clone()))
                    .copied()
            });
            match (resolved, ty) {
                (Some(id), _) => hir::ExprKind::AssignField {
                    recv: receiver.id,
                    field: id,
                    value: value.id,
                },
                // 宣言に無いフィールドへの代入は、宣言どおりの形をした struct 値を
                // 評価器へ渡す約束を破る。読みと同じ文言でここで止める
                (None, Some(ty)) => {
                    out.push(if cx.decls.structs.contains_key(ty.name().unwrap_or("")) {
                        format!("{ctx}: `{ty}` にフィールド `{field}` はありません")
                    } else {
                        format!("{ctx}: `{ty}` は struct ではないので `{field}` に代入できません")
                    });
                    hir::ExprKind::Poison
                }
                // レシーバの型が出なかった。理由はレシーバ側にある
                (None, None) => hir::ExprKind::Poison,
            }
        }
        _ => {
            synth(target, cx, locals, out);
            synth(value, cx, locals, out);
            out.push(format!("{ctx}: この左辺には代入できません"));
            hir::ExprKind::Poison
        }
    }
}

/// 解決した呼び出し先。署名は引数の検査に、宛先は下ろしに使う。
struct Resolved<'d> {
    sig: &'d FnSig,
    target: CallTarget,
}

/// 既に選ばれている呼び出し先(design.md 決定6)。
///
/// `Unresolvable` は宣言を HIR に載せられなかったときだけで、その宣言は
/// 必ず診断を伴う。
enum CallTarget {
    Direct(hir::CallableId),
    Method {
        callable: hir::CallableId,
        recv: hir::ExprId,
    },
    Associated(hir::CallableId),
    Slot {
        slot: hir::SlotId,
        method: hir::TraitMethodId,
        receiver: hir::SlotReceiver,
        /// スロットを名指している部分の位置。呼び出し全体より狭い
        slot_span: Span,
    },
    Ctor(hir::VariantId),
    Unresolvable,
}

/// 実引数とレシーバの所有モードの照合(tasks 5.1 / 5.2)。
///
/// 通常の型の照合と同じ規則を使い、合わないときだけ**共有借用を1つ**自動で挿す
/// (design.md 決定2)。`&mut`・`move`・`clone` は観測できる状態・所有・費用を
/// 動かすので、決して補わない。
///
/// 挿すのは場所のときだけ。挿さない2つの場合はどちらも所有権解析が同じ結論を
/// 出す — 参照はそのまま共有として再借用され(`bare_place` の再借用)、一時値は
/// 呼び出しの間だけ借りられる所有の値で別名が存在しない。ノードは「ソースに
/// 書いていない借用」を HIR とスナップショットに見せるためのもので、
/// 分類そのものは `Need` から決まる(design.md リスク「Render ... the inserted
/// shared borrow in diagnostics and HIR snapshots」)。
fn conform(
    checked: &Checked,
    expected: &KnownType,
    site: &Site,
    span: Span,
    cx: &Cx,
    out: &mut Out,
) -> hir::ExprId {
    // 値を産まない式。理由は式の側にある
    let Some(actual) = checked.outcome.ty().cloned() else {
        return checked.id;
    };
    if fits(&actual, expected) {
        return checked.id;
    }
    // `&T` の位置は、借用先の所有の形が同じなら所有の場所からも埋まる。
    // 既に参照で持っているものは、そのまま共有として再借用される
    if expected.reference == Some(hir::RefKind::Shared)
        && actual.kind == expected.kind
        && actual.optional == expected.optional
    {
        // `move` して共有借用を渡すと、呼び出し側は値を失うのに渡るのは借用
        // だけになる。自動借用があるので修飾そのものが要らない
        if let hir::ExprKind::Access {
            mode: AccessMode::Move,
            ..
        } = out.body.expr(checked.id).kind
        {
            out.diagnostics.push(
                Diag::at(
                    span,
                    format!(
                        "{}: 共有借用を受け取る位置に `move` は掛けられません",
                        cx.ctx
                    ),
                )
                .label("共有借用の位置への `move`")
                .help("`move` を外してください。共有借用は自動で挿さります"),
            );
            return checked.id;
        }
        return match actual.reference {
            Some(_) => checked.id,
            // 一時値は既に所有者で、呼び出しの間だけ借りられる。借用を挿すと
            // 「場所ではない値への修飾」になってしまうので、場所にだけ挿す
            None if !out.body.is_place(checked.id) => checked.id,
            None => shared_borrow(checked.id, &actual, span, cx, out),
        };
    }
    let mut diagnostic = Diag::at(span, site.message(cx.ctx, expected, &actual.to_string()));
    // 所有の値を排他の位置へ渡している = 呼び出し側の修飾が抜けている
    if expected.reference == Some(hir::RefKind::Mutable) && actual.reference.is_none() {
        diagnostic = diagnostic.help("`&mut` を付けて排他借用を渡してください");
    }
    out.diagnostics.push(diagnostic);
    checked.id
}

/// 自動で挿す共有借用。ソースに `&place` と書いたときと同じ形になる。
fn shared_borrow(
    place: hir::ExprId,
    ty: &KnownType,
    span: Span,
    cx: &Cx,
    out: &mut Out,
) -> hir::ExprId {
    let borrowed = KnownType {
        reference: Some(hir::RefKind::Shared),
        ..ty.clone()
    };
    out.body.alloc_expr(hir::Expr {
        result: hir::ExprResult::Value(lower_known(&borrowed, &cx.decls.nominal)),
        kind: hir::ExprKind::Access {
            mode: AccessMode::Shared,
            place,
        },
        span,
    })
}

/// 実引数1つ。共有借用を受け取る位置だけは所有の値も収まるので、期待型を
/// 配らずに走査してから照合する(tasks 5.1)。
fn argument(
    arg: &Expr,
    expected: &KnownType,
    site: &Site,
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> hir::ExprId {
    // `nil` は期待型が無いと型を出せない。参照にはなれないので、照合も
    // 従来どおり `walk` の出口に任せる
    if expected.reference != Some(hir::RefKind::Shared) || matches!(arg.kind, ExprKind::Nil) {
        return walk(arg, Some((expected, site)), cx, locals, out).id;
    }
    // 配列リテラルは要素型を期待型から取る。リテラルが参照になることは無いので、
    // 借用先の所有の形をそのまま配れば推論も照合も従来どおり閉じる
    if matches!(arg.kind, ExprKind::Array(_)) {
        let owned = KnownType {
            reference: None,
            ..expected.clone()
        };
        return walk(arg, Some((&owned, site)), cx, locals, out).id;
    }
    let checked = walk(arg, None, cx, locals, out);
    conform(&checked, expected, site, arg.span, cx, out)
}

/// 呼び出し。解決した署名から個数・引数型・結果型が決まる(design.md 決定6)。
fn call(callee: &Expr, args: &[Expr], cx: &Cx, locals: &mut Locals, out: &mut Out) -> Lowered {
    let resolved = match resolve(callee, args.len(), cx, locals, out) {
        Ok(resolved) => resolved,
        Err(lowered) => {
            // 解決できなくても実引数自身は検査する。期待型は配れない
            for arg in args {
                synth(arg, cx, locals, out);
            }
            return lowered;
        }
    };
    let name = member(callee);
    let ret = resolved.sig.ret.clone();
    if args.len() != resolved.sig.params.len() {
        // 個数が合わなければ引数と宣言の対応が取れないので期待型は配らない
        for arg in args {
            synth(arg, cx, locals, out);
        }
        out.push(format!(
            "{}: `{name}` は引数を {} 個取りますが、{} 個渡しています",
            cx.ctx,
            resolved.sig.params.len(),
            args.len()
        ));
        return lowered(Outcome::Typed(ret), hir::ExprKind::Poison);
    }
    let mut ids = Vec::with_capacity(args.len());
    for (index, (arg, expected)) in args.iter().zip(&resolved.sig.params).enumerate() {
        let site = Site::Arg {
            callee: &name,
            index,
        };
        ids.push(argument(arg, expected, &site, cx, locals, out));
    }
    let kind = match resolved.target {
        CallTarget::Direct(callable) => hir::ExprKind::Call(hir::Call::Direct {
            callable,
            args: ids,
        }),
        CallTarget::Method { callable, recv } => hir::ExprKind::Call(hir::Call::Method {
            callable,
            recv,
            args: ids,
        }),
        CallTarget::Associated(callable) => hir::ExprKind::Call(hir::Call::Associated {
            callable,
            args: ids,
        }),
        CallTarget::Slot {
            slot,
            method,
            receiver,
            slot_span,
        } => hir::ExprKind::Call(hir::Call::Slot {
            slot,
            method,
            receiver,
            slot_span,
            args: ids,
        }),
        CallTarget::Ctor(variant) => hir::ExprKind::Call(hir::Call::Ctor { variant, args: ids }),
        CallTarget::Unresolvable => hir::ExprKind::Poison,
    };
    // 解決した呼び出しは実効戻り値型を持つ(design.md 決定1・6)
    typed(ret, kind)
}

/// 呼び出し先の署名と宛先を選ぶ。レシーバの走査もここで1度だけ行うので、
/// 型を引くためにもう一周する必要が無い。
///
/// 選び方は評価器と同じ順序で、宣言済み enum の限定 variant を constructor として
/// 最初に見てから、スロット経由なら宣言 trait の契約だけ、具体型なら inherent と
/// trait 実装をまとめて名前で絞り一意を要求する(design.md 決定6)。
/// `Err` はそのまま呼び出し式の下ろした形になる。
fn resolve<'d>(
    callee: &Expr,
    arity: usize,
    cx: &Cx<'d>,
    locals: &mut Locals,
    out: &mut Out,
) -> Result<Resolved<'d>, Lowered> {
    let decls = cx.decls;
    let before = out.count();
    let selected = match &callee.kind {
        // 直接呼び出し。名前は値として読まれない
        ExprKind::Ident(name) => Ok(decls.fns.get(name).map(|sig| {
            let target = decls
                .ids
                .fns
                .get(name)
                .map_or(CallTarget::Unresolvable, |id| CallTarget::Direct(*id));
            (sig, target)
        })),
        ExprKind::Field(recv, name) => {
            // ambient スロット経由は宣言 trait の契約だけを見る。スロット名は
            // 値ではないのでレシーバとして走査しない
            if let ExprKind::Ident(recv_name) = &recv.kind
                && let Some(trait_name) = slot_trait(recv_name, decls, locals)
            {
                from_trait(&trait_name, name, true, decls).map(|found| {
                    found.map(|sig| {
                        (
                            sig,
                            slot_target(
                                recv_name,
                                &trait_name,
                                name,
                                hir::SlotReceiver::Value,
                                callee.span,
                                decls,
                            ),
                        )
                    })
                })
            } else {
                let receiver = synth(recv, cx, locals, out);
                match &receiver.outcome {
                    // 組み込みの `clone()`。同名の宣言メソッドがあればそちらが勝つ
                    Outcome::Typed(ty)
                        if name == "clone" && arity == 0 && !declares_clone(ty, decls) =>
                    {
                        return Err(clone_of(receiver.id, ty, callee.span, cx, out));
                    }
                    // optional の中身を取り出す規則はまだ無く、配列にメソッドも無い
                    Outcome::Typed(ty) => match ty.name().filter(|_| !ty.optional) {
                        Some(type_name) => from_type(type_name, name, true, decls).map(|found| {
                            found.map(|sig| {
                                // レシーバの所有モードを署名と突き合わせる。
                                // `&self` へは共有借用を1つ挿す(tasks 5.2)
                                let recv = conform_receiver(
                                    sig, type_name, name, &receiver, recv.span, cx, out,
                                );
                                (
                                    sig,
                                    concrete_target(type_name, name, decls)
                                        .map_or(CallTarget::Unresolvable, |callable| {
                                            CallTarget::Method { callable, recv }
                                        }),
                                )
                            })
                        }),
                        None => Ok(None),
                    },
                    Outcome::Diverges => return Err(diverged(receiver.id)),
                    Outcome::Poisoned => return Err(poison()),
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
                return Err(poison());
            };
            // 宣言済み enum の variant なら constructor。関連関数や ambient の
            // 型射影より先に見る(design.md 決定6)
            if let Some(ctor) = variant_ctor(first, name, decls) {
                Ok(Some((
                    ctor,
                    CallTarget::Ctor(variant_id(first, name, decls)),
                )))
            } else {
                match slot_trait(first, decls, locals) {
                    Some(trait_name) => from_trait(&trait_name, name, false, decls).map(|found| {
                        found.map(|sig| {
                            (
                                sig,
                                slot_target(
                                    first,
                                    &trait_name,
                                    name,
                                    hir::SlotReceiver::Type,
                                    callee.span,
                                    decls,
                                ),
                            )
                        })
                    }),
                    None => from_type(first, name, false, decls).map(|found| {
                        found.map(|sig| {
                            (
                                sig,
                                concrete_target(first, name, decls)
                                    .map_or(CallTarget::Unresolvable, CallTarget::Associated),
                            )
                        })
                    }),
                }
            }
        }
        _ => {
            synth(callee, cx, locals, out);
            Ok(None)
        }
    };
    match selected {
        Ok(Some((sig, target))) => Ok(Resolved { sig, target }),
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
            Err(poison())
        }
        Err(message) => {
            out.push(format!("{}: {message}", cx.ctx));
            Err(poison())
        }
    }
}

/// その型が `clone` という名前のメソッドを自分で宣言しているか。
///
/// 宣言があれば組み込みは引っ込む。`clone()` を検査器が知っているのはこの版に
/// `Clone` 契約がまだ無いからで、既存の宣言を黙って隠すと後で契約へ移すときに
/// 意味が変わってしまう(design.md 決定4)。
fn declares_clone(ty: &KnownType, decls: &Decls) -> bool {
    ty.name()
        .filter(|_| !ty.optional)
        .and_then(|name| decls.impls.get(name))
        .is_some_and(|members| members.iter().any(|(member, _)| member == "clone"))
}

/// 組み込みの `clone()`(design.md 決定4、tasks 6.5)。
///
/// 結果は常に**所有**。`&T` は借用先を所有の `T` へ複製し、`&mut T` は複製
/// できない。深さは実行時の構造そのままで、ここは型だけを決める。
fn clone_of(recv: hir::ExprId, ty: &KnownType, span: Span, cx: &Cx, out: &mut Out) -> Lowered {
    let kind = hir::ExprKind::Clone(recv);
    if ty.reference == Some(hir::RefKind::Mutable) {
        out.diagnostics.push(
            Diag::at(
                span,
                format!("{}: 排他借用 `{ty}` は clone できません", cx.ctx),
            )
            .label("複製できない排他借用")
            .help("共有借用か所有の値から clone してください"),
        );
        return lowered(Outcome::Poisoned, kind);
    }
    typed(
        KnownType {
            reference: None,
            ..ty.clone()
        },
        kind,
    )
}

/// レシーバの所有モードを署名と突き合わせる(tasks 5.2)。
///
/// `&self` には所有の場所から共有借用を1つ挿す。`&mut self` と消費 `self` は
/// 状態と所有を動かすので、呼び出し側に `&mut` / `move` が書かれていなければ
/// ここで断る(design.md 決定2)。消費レシーバに `move` が要ることは静的型では
/// 言い分けられないので、その1件だけは所有権解析が見る。
fn conform_receiver(
    sig: &FnSig,
    type_name: &str,
    method: &str,
    receiver: &Checked,
    span: Span,
    cx: &Cx,
    out: &mut Out,
) -> hir::ExprId {
    let Some(mode) = sig.receiver else {
        return receiver.id;
    };
    let callee = format!("{type_name}::{method}");
    let site = Site::Receiver { callee: &callee };
    conform(
        receiver,
        &receiver_type(mode, type_name),
        &site,
        span,
        cx,
        out,
    )
}

/// スロット経由の宛先。実行する本体はその場の提供が決めるので、ここでは
/// スロットと契約メソッドだけを指す(design.md 決定6)。
fn slot_target(
    slot: &str,
    trait_name: &str,
    method: &str,
    receiver: hir::SlotReceiver,
    slot_span: Span,
    decls: &Decls,
) -> CallTarget {
    let contract = decls
        .ids
        .trait_methods
        .get(&(trait_name.to_string(), method.to_string()));
    match (decls.ids.slots.get(slot), contract) {
        (Some(slot), Some(method)) => CallTarget::Slot {
            slot: *slot,
            method: *method,
            receiver,
            slot_span,
        },
        _ => CallTarget::Unresolvable,
    }
}

/// 具体型のメンバーの本体。名前で絞って一意でなければ呼び出し自体が曖昧として
/// 落ちているので、候補が1つのときだけ引ける(`from_type` と同じ規則)。
fn concrete_target(type_name: &str, method: &str, decls: &Decls) -> Option<hir::CallableId> {
    match decls
        .ids
        .methods
        .get(&(type_name.to_string(), method.to_string()))
    {
        Some(found) if found.len() == 1 => Some(found[0]),
        _ => None,
    }
}

/// 等価比較。`nil` は反対側の optional 性を文脈にする(design.md 決定7)。
/// 結果はどちらにせよ `bool`。
fn equality(lhs: &Expr, rhs: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Lowered {
    let left_nil = matches!(lhs.kind, ExprKind::Nil);
    let right_nil = matches!(rhs.kind, ExprKind::Nil);
    // 両辺 `nil` は nominal な optional 型を決められない(design.md 決定7)
    if left_nil && right_nil {
        out.push(format!(
            "{}: `==` の両辺が `nil` なので optional の型が決まりません",
            cx.ctx
        ));
        return poison();
    }
    if left_nil || right_nil {
        let (value, bare) = if left_nil { (rhs, lhs) } else { (lhs, rhs) };
        // 片側が `nil` なら、もう片側の型がそのまま optional 性の文脈になる
        let checked = synth(value, cx, locals, out);
        let ty = checked.outcome.ty().cloned();
        if let Some(ty) = &ty
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
        // `nil` 側は相手の型を期待型にして下ろす。相手が optional でなければ
        // 型を与えられないが、その理由は既に上で報告している
        let site = Site::What("`==` の `nil` 側");
        let bare = match &ty {
            Some(ty) if ty.optional => walk(bare, Some((ty, &site)), cx, locals, out),
            _ => synth(bare, cx, locals, out),
        };
        let (lhs, rhs) = if left_nil {
            (bare.id, checked.id)
        } else {
            (checked.id, bare.id)
        };
        return typed(plain("bool"), hir::ExprKind::Eq { lhs, rhs });
    }
    let left = synth(lhs, cx, locals, out);
    let right = synth(rhs, cx, locals, out);
    if let (Some(left), Some(right)) = (left.outcome.ty(), right.outcome.ty())
        && left != right
    {
        out.push(format!(
            "{}: `==` の両辺は同じ型である必要がありますが、`{left}` と `{right}` です",
            cx.ctx
        ));
    }
    typed(
        plain("bool"),
        hir::ExprKind::Eq {
            lhs: left.id,
            rhs: right.id,
        },
    )
}

/// `T? ?? T` は `T` を返す。右辺が枝を抜けるなら中身の型との照合は要らない
/// (design.md 決定7)。
fn coalesce(lhs: &Expr, rhs: &Expr, cx: &Cx, locals: &mut Locals, out: &mut Out) -> Lowered {
    let ctx = cx.ctx;
    // 左辺が裸の `nil` なら、右辺の非 optional な型がそのまま結果になる
    if matches!(lhs.kind, ExprKind::Nil) {
        let right = synth(rhs, cx, locals, out);
        let ty = match &right.outcome {
            Outcome::Typed(ty) if ty.optional => {
                out.push(format!(
                    "{ctx}: `??` の右辺には非 optional の値が必要ですが、`{ty}` です"
                ));
                return poison();
            }
            Outcome::Typed(ty) => ty.clone(),
            // 右辺が値を産まないなら、この式は右辺を評価するだけで終わる。
            // 使われない `nil` に型を与える必要はない
            Outcome::Diverges => return diverged(right.id),
            Outcome::Poisoned => return poison(),
        };
        // 左辺の `nil` は結果型の optional 版。右辺が借用ならそれは optional な
        // 参照なので、注釈で書いたときと同じ理由で断る(tasks 2.3)。
        // `optional_of` が `&T?` を作れる唯一の経路がここ
        let optional = optional_of(&ty);
        if ty.reference.is_some() {
            out.push(format!(
                "{ctx}: `??` の左辺{}",
                optional_reference(&optional)
            ));
            return poison();
        }
        let site = Site::What("`??` の左辺");
        let bare = walk(lhs, Some((&optional, &site)), cx, locals, out);
        return typed(
            ty,
            hir::ExprKind::Coalesce {
                lhs: bare.id,
                rhs: right.id,
            },
        );
    }
    let before = out.count();
    let left = synth(lhs, cx, locals, out);
    let ty = match &left.outcome {
        Outcome::Typed(ty) => ty.clone(),
        other => {
            if matches!(other, Outcome::Poisoned) {
                out.explain(before, format!("{ctx}: `??` の左辺の型が決まりません"));
            }
            synth(rhs, cx, locals, out);
            // 左辺が値を産まないなら右辺も走らない
            return match other {
                Outcome::Diverges => diverged(left.id),
                _ => poison(),
            };
        }
    };
    if !ty.optional {
        out.push(format!(
            "{ctx}: `??` の左辺は optional である必要がありますが、`{ty}` です"
        ));
        synth(rhs, cx, locals, out);
        return poison();
    }
    let inner = KnownType {
        reference: ty.reference,
        kind: ty.kind,
        optional: false,
    };
    let site = Site::What("`??` の右辺");
    let right = walk(rhs, Some((&inner, &site)), cx, locals, out);
    let kind = hir::ExprKind::Coalesce {
        lhs: left.id,
        rhs: right.id,
    };
    match right.outcome {
        // 右辺が値を産まずに枝を終えても、続く経路の値は左辺の中身
        Outcome::Typed(_) | Outcome::Diverges => typed(inner, kind),
        Outcome::Poisoned => lowered(Outcome::Poisoned, kind),
    }
}

/// 後置 `?` を付けた同じ形。
fn optional_of(ty: &KnownType) -> KnownType {
    KnownType {
        reference: ty.reference,
        kind: ty.kind.clone(),
        optional: true,
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
) -> Lowered {
    let before = out.count();
    let (checked, matched) = matched_enum(subject, cx, locals, out);
    check_arms(arms, matched.as_deref(), cx.decls, cx.ctx, out);

    let mut result = want(expected).cloned();
    let mut poisoned = false;
    // arm が1つも無い(空 enum の網羅的な match)ときも、値は産まれない
    let mut continues = false;
    let mut lowered_arms = Vec::with_capacity(arms.len());
    let mut resolvable = true;
    for arm in arms {
        let (mut inner, pattern) = arm_locals(arm, cx.decls, locals, out);
        // guard は payload を見られるが、本体へ束縛を漏らさないよう複製で検査する
        let guard = arm.guard.as_ref().map(|guard| {
            let mut guard_locals = inner.clone();
            let site = Site::What("arm の guard");
            let before = out.count();
            let checked = walk(
                guard,
                Some((&plain("bool"), &site)),
                cx,
                &mut guard_locals,
                out,
            );
            // guard が偽になりうるかは網羅性に効くので、実行前に `bool` を確定する
            if matches!(checked.outcome, Outcome::Poisoned) {
                out.explain(
                    before,
                    format!("{}: arm の guard の型が決まりません", cx.ctx),
                );
            }
            checked.id
        });
        let what = format!("arm `{}` の値", arm.pattern.label());
        let site = Site::What(&what);
        let body = match &result {
            Some(result) => walk(&arm.body, Some((result, &site)), cx, &mut inner, out),
            None => synth(&arm.body, cx, &mut inner, out),
        };
        match body.outcome {
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
        match pattern {
            Some(pattern) => lowered_arms.push(hir::MatchArm {
                pattern,
                guard,
                body: body.id,
                span: arm.span,
            }),
            // 宣言に無い variant を指す arm。`check_arms` が報告済み
            None => resolvable = false,
        }
    }

    let kind = if resolvable {
        hir::ExprKind::Match {
            subject: checked.id,
            arms: lowered_arms,
        }
    } else {
        hir::ExprKind::Poison
    };
    match want(expected) {
        // 全 arm が抜けるなら合流点も抜ける。空 enum を 0 arm で網羅した
        // `match` もここ。宛先があっても値は産まれない(design.md 決定5)
        _ if !continues => lowered(Outcome::Diverges, kind),
        // 期待型のある位置は arm ごとに照合済みなので、式全体では二度言わない
        Some(expected) => typed(expected.clone(), kind),
        None if poisoned => lowered(Outcome::Poisoned, kind),
        None => match result {
            Some(result) => typed(result, kind),
            None => {
                out.explain(
                    before,
                    format!("{}: `match` の結果型が決まりません", cx.ctx),
                );
                lowered(Outcome::Poisoned, kind)
            }
        },
    }
}

/// `match` の対象の enum。既知の非 optional な enum のときだけ名前を返す。
///
/// 型不明の対象を実行時へ委ねると arm の所属・網羅性・結果型の基準を決められない
/// ので、ここで診断する(design.md 決定3)。
fn matched_enum(
    subject: &Expr,
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> (Checked, Option<String>) {
    let before = out.count();
    let checked = synth(subject, cx, locals, out);
    let ty = match &checked.outcome {
        Outcome::Typed(ty) => ty.clone(),
        // 対象が抜けるなら arm へ入らない
        Outcome::Diverges => return (checked, None),
        Outcome::Poisoned => {
            out.explain(
                before,
                format!("{}: `match` の対象の型が決まりません", cx.ctx),
            );
            return (checked, None);
        }
    };
    let matched = match ty.name() {
        Some(name) if !ty.optional && cx.decls.enums.contains_key(name) => Some(name.to_string()),
        _ => {
            out.push(format!(
                "{}: `match` の対象は非 optional な enum である必要がありますが、`{ty}` です",
                cx.ctx
            ));
            None
        }
    };
    (checked, matched)
}

/// `Head` で始まる式。`with` と条件式は本体の値を産み、ループは `unit`
/// (design.md 決定5)。
#[allow(clippy::too_many_arguments)]
fn head_expr(
    expected: Expect,
    head: &Head,
    body: &Expr,
    orelse: Option<&Expr>,
    span: Span,
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> Lowered {
    match head {
        Head::Ambient(binders) => {
            // 提供値は外側で評価される
            let mut provisions = Vec::with_capacity(binders.len());
            let mut resolvable = true;
            for binder in binders {
                let (ty, value) = match binder {
                    Provision::Type { type_name, .. } => (Some(plain(type_name)), None),
                    Provision::Value { value, .. } => {
                        let checked = synth(value, cx, locals, out);
                        (checked.outcome.ty().cloned(), Some(checked.id))
                    }
                };
                let implementation = check_provision(binder, ty.as_ref(), cx, out);
                match (cx.decls.ids.slots.get(binder.slot()), implementation) {
                    (Some(slot), Some(implementation)) => provisions.push(hir::Provision {
                        slot: *slot,
                        implementation,
                        value,
                        span,
                    }),
                    // スロットでない名前、あるいは契約を満たさない提供。
                    // `check_provision` と requirement 側が報告する
                    _ => resolvable = false,
                }
            }
            // 本体では内側の束縛が勝つ。スロットでない名前は requirement 側が
            // 報告するので、ここでは型不明の値にする
            let mut inner = locals.clone();
            for binder in binders {
                let slot = binder.slot();
                let binding = match cx.decls.slots.trait_of(slot) {
                    Some(trait_name) => Binding::Slot(trait_name.to_string()),
                    None => {
                        let local = alloc_local(slot, None, span, cx.decls, out);
                        Binding::Value(None, local)
                    }
                };
                inner.insert(slot.to_string(), binding);
            }
            let body = walk(body, expected, cx, &mut inner, out);
            let kind = if resolvable {
                hir::ExprKind::With {
                    provisions,
                    body: body.id,
                }
            } else {
                hir::ExprKind::Poison
            };
            lowered(body.outcome, kind)
        }
        Head::If(condition) | Head::Elif(condition) => {
            let site = Site::What("条件");
            let condition = walk(condition, Some((&plain("bool"), &site)), cx, locals, out);
            // `else` で終わらない連鎖はどの枝の値も使わないので `unit` を産む。
            // 使われない値に期待型を課さないため、枝へ配るのもそのときだけ
            // (design.md 決定5)
            let valued = has_else(orelse);
            let branch = if valued { expected } else { None };
            let taken = walk(body, branch, cx, &mut locals.clone(), out);
            let Some(orelse) = orelse else {
                return produces_unit(hir::ExprKind::If {
                    cond: condition.id,
                    then: taken.id,
                    orelse: None,
                });
            };
            let other = walk(orelse, branch, cx, &mut locals.clone(), out);
            let kind = hir::ExprKind::If {
                cond: condition.id,
                then: taken.id,
                orelse: Some(other.id),
            };
            if valued {
                lowered(merge(expected, taken.outcome, other.outcome), kind)
            } else {
                produces_unit(kind)
            }
        }
        Head::While(condition) => {
            let site = Site::What("条件");
            let condition = walk(condition, Some((&plain("bool"), &site)), cx, locals, out);
            let body = synth(body, cx, &mut locals.clone(), out);
            produces_unit(hir::ExprKind::While {
                cond: condition.id,
                body: body.id,
            })
        }
        Head::For { var, iter } => {
            let (iter, element) = iterated(iter, cx, locals, out);
            let mut inner = locals.clone();
            let local = alloc_local(var, element.as_ref(), span, cx.decls, out);
            inner.insert(var.clone(), Binding::Value(element, local));
            let body = synth(body, cx, &mut inner, out);
            produces_unit(hir::ExprKind::For {
                var: local,
                iter: iter.id,
                body: body.id,
            })
        }
        Head::Else => {
            let body = walk(body, expected, cx, &mut locals.clone(), out);
            lowered(body.outcome, hir::ExprKind::Block(vec![body.id]))
        }
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
fn iterated(
    iter: &Expr,
    cx: &Cx,
    locals: &mut Locals,
    out: &mut Out,
) -> (Checked, Option<KnownType>) {
    let before = out.count();
    let checked = synth(iter, cx, locals, out);
    let Outcome::Typed(ty) = &checked.outcome else {
        // 反復対象の型が決まらないとループ変数の型も決まらない
        if matches!(checked.outcome, Outcome::Poisoned) {
            out.explain(
                before,
                format!("{}: `for` の反復対象の型が決まりません", cx.ctx),
            );
        }
        return (checked, None);
    };
    let ty = ty.clone();
    let Some(element) = ty.element() else {
        out.push(format!(
            "{}: `for` の反復対象は配列である必要がありますが、`{ty}` です",
            cx.ctx
        ));
        return (checked, None);
    };
    if ty.optional {
        out.push(format!(
            "{}: optional な配列 `{ty}` はそのまま反復できません。先に `??` で展開してください",
            cx.ctx
        ));
        return (checked, None);
    }
    let element = element.clone();
    (checked, Some(element))
}

/// `with` の提供がスロットの契約を満たすか見て、満たすならその `impl` を返す。
/// `with db<Postgres>` は型名がそのまま分かるので常に、`with db(v)` は `v` の型が
/// 分かるときだけ見る(design.md 決定3)。
///
/// スロットでない名前は requirement 側が報告する。
fn check_provision(
    binder: &Provision,
    ty: Option<&KnownType>,
    cx: &Cx,
    out: &mut Out,
) -> Option<hir::TraitImplId> {
    let want = cx.decls.slots.trait_of(binder.slot())?;
    let Some(ty) = ty else {
        // 型の分からない提供を実行時へ回さない(design.md 決定3)
        out.push(format!(
            "{}: `{}` に提供する値の型が決まりません",
            cx.ctx,
            binder.slot()
        ));
        return None;
    };
    // 提供できるのは trait を実装した具体型そのものだけ。配列と optional は
    // 名前を持たないので、この時点で落ちる
    let implemented = (!ty.optional)
        .then(|| ty.name())
        .flatten()
        .and_then(|name| {
            cx.decls
                .ids
                .trait_impls
                .get(&(name.to_string(), want.to_string()))
        })
        .copied();
    if implemented.is_none() {
        out.push(format!(
            "{}: `{ty}` は `{want}` を実装していないので `{}` に提供できません",
            cx.ctx,
            binder.slot()
        ));
    }
    implemented
}

// ---------------------------------------------------------------------------
// 宣言の索引を引くだけのヘルパ
// ---------------------------------------------------------------------------

/// arm 本体から見えるローカル。pattern の名前は対応する宣言 payload 型を持ち、
/// 同名の外側束縛をこの arm の間だけ隠す。`_` は名前を作らない
/// (design.md 決定5)。
fn arm_locals(
    arm: &MatchArm,
    decls: &Decls,
    locals: &Locals,
    out: &mut Out,
) -> (Locals, Option<hir::Pattern>) {
    let mut inner = locals.clone();
    // `_` は variant も payload も晒さないので、外側のローカルがそのまま見える
    let MatchPattern::Variant {
        enum_name,
        variant,
        bindings,
    } = &arm.pattern
    else {
        return (inner, Some(hir::Pattern::CatchAll));
    };
    let payload: Vec<Option<KnownType>> = {
        let declared = payload_of(enum_name, variant, decls).unwrap_or(&[]);
        (0..bindings.len())
            .map(|n| declared.get(n).cloned())
            .collect()
    };
    let mut bound = Vec::with_capacity(bindings.len());
    for (binding, ty) in bindings.iter().zip(payload) {
        match binding {
            PatternBinding::Bind(name) => {
                // 個数が合わないときは `check_arms` が診断済み。型は付けずに束縛だけ作る
                let local = alloc_local(name, ty.as_ref(), arm.span, decls, out);
                inner.insert(name.clone(), Binding::Value(ty, local));
                bound.push(Some(local));
            }
            // `_` は値を捨てるので名前を作らない
            PatternBinding::Discard => bound.push(None),
        }
    }
    let pattern = declared_variant(enum_name, variant, decls)
        .and_then(|canonical| decls.ids.variants.get(canonical))
        .map(|variant| hir::Pattern::Variant {
            variant: *variant,
            bindings: bound,
        });
    (inner, pattern)
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
        Some(Binding::Value(..)) => None,
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
    match (sig.receiver.is_some(), dot) {
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

    /// 診断だけを見る。下ろした HIR は捨てる
    fn diagnostics(src: &str) -> Vec<Diag> {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        check_and_lower(&program).err().unwrap_or_default()
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

    // ---- 組み込みの clone(tasks 6.5) ----

    /// `clone()` は所有を産む。`&T` からは借用先の所有の形へ
    #[test]
    fn cloneは所有の値を産む() {
        let src = "struct User { id: int, name: str }
fn copy_of(u: &User -> User) { u.clone() }
fn main(-> int) { let u = User { id = 1, name = \"a\" }
 copy_of(u).id + u.clone().id }
";
        assert_eq!(errors(src), Vec::<String>::new());
    }

    /// 排他借用は複製できない(design.md 決定4)
    #[test]
    fn 排他借用はcloneできない() {
        assert_eq!(
            only(
                "struct User { id: int }
fn copy_of(u: &mut User -> User) { u.clone() }
"
            ),
            "copy_of: 排他借用 `&mut User` は clone できません"
        );
    }

    /// optional と配列も複製できる。結果の形は元のまま
    #[test]
    fn optionalと配列もcloneできる() {
        let src = "struct User { id: int }
fn main(-> int) { let xs = [User { id = 1 }]
 let o: User? = User { id = 2 }
 let ys = xs.clone()
 let p = o.clone()
 0 }
";
        assert_eq!(errors(src), Vec::<String>::new());
    }

    /// 宣言された `clone` があればそちらが勝つ。組み込みは引っ込む
    #[test]
    fn 宣言されたcloneが組み込みより優先する() {
        assert_eq!(
            only(
                "struct User { id: int }
impl User { fn clone(&self -> int) { self.id } }
fn main(-> User) { let u = User { id = 1 }
 u.clone() }
"
            ),
            "main: 戻り値は `User` ですが、`int` を返しています"
        );
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
        let e = only("enum Rank { Gold }\nstruct User { rank: Rank\nrank: Rank }\n");
        assert!(e.contains("struct `User`"), "{e}");
        assert!(e.contains("`rank`"), "{e}");
    }

    #[test]
    fn 相異なる宣言フィールドは診断を出さない() {
        assert!(errors("enum Rank { Gold }\nstruct User { id: int\nrank: Rank }\n").is_empty());
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
                 struct Users {}\n\
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

    /// guard 自身の型を決められない原因も、実行時へ委ねずに実行前に報告する
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
            "trait Clock { fn now(self -> int) }\neffect int: Clock\n",
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
                                   struct User { profile: Profile\nindirect manager: User? }\n";

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
            (
                "fn find(id: int -> User?) { nil }",
                "レシーバは `self` ですが、レシーバ無し",
            ),
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

    // ---- 宣言の下ろし ----

    /// 下ろした HIR。診断があれば失敗させる
    fn lowered(src: &str) -> hir::Program {
        let program = parse::parse(&join(lex(src).unwrap())).expect("パースできるはず");
        check_and_lower(&program).expect("診断なしで下がるはず")
    }

    /// 宣言だけの小さなプログラムを丸ごと固定する。宣言種ごとの ID の振り方と
    /// 参照の解決が一目で読める形でここに書いてある
    #[test]
    fn 全宣言種の下ろしを固定する() {
        let dumped = lowered(
            "trait Clock { fn now(self -> int) }\n\
             struct Frozen { at: int }\n\
             enum Rank { Bronze Gold }\n\
             effect clock: Clock\n\
             impl Clock for Frozen { fn now(self -> int) { self.at } }\n\
             impl Frozen { fn make(-> Frozen) { Frozen { at = 0 } } }\n\
             fn rank(r: Rank -> Rank) { r }\n\
             test \"t\" { assert true }\n",
        )
        .dump();
        assert_eq!(
            dumped,
            "struct#0 Frozen { at#0: int }\n\
             enum#0 Rank { Bronze#0, Gold#1 }\n\
             trait#0 Clock\n\
             \x20 method#0 now(self) -> int\n\
             slot#0 clock: Clock\n\
             impl#0 Clock for Frozen\n\
             \x20 now -> callable#0\n\
             callable#0 impl#0 now(self) -> int\n\
             \x20 local#0 self: Frozen\n\
             \x20 expr#0 : Frozen = local#0\n\
             \x20 expr#1 : int = field #0 .at\n\
             \x20 root [#1]\n\
             callable#1 inherent Frozen make() -> Frozen\n\
             \x20 expr#0 : int = int 0\n\
             \x20 expr#1 : Frozen = struct-lit Frozen { at=#0 }\n\
             \x20 root [#1]\n\
             callable#2 fn rank(Rank) -> Rank\n\
             \x20 local#0 r: Rank\n\
             \x20 expr#0 : Rank = local#0\n\
             \x20 root [#0]\n\
             test#0 \"t\"\n\
             \x20 expr#0 : bool = bool true\n\
             \x20 expr#1 : unit = assert #0\n\
             \x20 root [#1]\n",
            "{dumped}"
        );
    }

    /// 正典プログラムの宣言。個数と、名前ではなく ID で結ばれた参照を見る
    #[test]
    fn 正典の宣言はidで結ばれる() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let program = lowered(&src);

        // 宣言順に詰まっているので、個数が変わればここが落ちる
        assert_eq!(program.structs.len(), 5, "{}", program.dump());
        assert_eq!(program.enums.len(), 1);
        assert_eq!(program.traits.len(), 2);
        assert_eq!(program.slots.len(), 2);
        assert_eq!(program.trait_impls.len(), 4);

        // スロットは trait 名ではなく `TraitId` を持つ
        for (_, slot) in program.slots.iter() {
            let trait_name = &program.traits[slot.trait_].name;
            assert!(
                trait_name.ends_with("Database") || trait_name.ends_with("Clock"),
                "{trait_name}"
            );
        }

        // trait 実装は契約メソッドから本体へ直接つながっている
        for (id, impl_) in program.trait_impls.iter() {
            let contract = &program.traits[impl_.trait_];
            assert_eq!(
                impl_.methods.len(),
                contract.methods.len(),
                "impl#{} は契約を全部埋める",
                crate::hir::Id::index(id)
            );
            for method in &contract.methods {
                let callable = program
                    .implementation_of(id, *method)
                    .expect("契約メソッドに本体がある");
                assert_eq!(
                    program.callables[callable].name,
                    program.trait_methods[*method].name
                );
            }
        }

        // struct のフィールドは所属 struct を指し、型は宣言を指す
        let user = program
            .fields
            .iter()
            .find(|(_, f)| f.name == "user")
            .expect("正典に `user` がある");
        assert_eq!(program.structs[user.1.owner].name, "InMemoryDb");
        assert_eq!(program.show_type(&user.1.ty), "User");
    }

    /// 受理される式の形すべてが HIR の形へ下がること。1つでも `Poison` が
    /// 残れば `check_and_lower` が落ちるので、下がったことは成功が示す。
    /// ここで数えるのは「どの形も一度は通った」ことだけ(tasks 3.9)
    #[test]
    fn 受理される全ての式の形が下がる() {
        let program = lowered(
            "trait Clock { fn now(self -> int)\n fn zero(-> int) }\n\
             struct Frozen { t: int }\n\
             struct Empty {}\n\
             enum Lookup { Found(int) Missing }\n\
             effect clock: Clock\n\
             impl Clock for Frozen {\n\
             \x20 fn now(self -> int) { self.t }\n\
             \x20 fn zero(-> int) { 0 }\n\
             }\n\
             impl Frozen { fn at(t: int -> Frozen) { Frozen { t = t } } }\n\
             fn pick(n: int -> Lookup) { Lookup::Found(n) }\n\
             fn sink(n: int -> int) { n }\n\
             fn all(-> int) {\n\
             \x20 let f = Frozen::at(1)\n\
             \x20 let e = Empty\n\
             \x20 let m = Missing\n\
             \x20 let text = \"x\"\n\
             \x20 let flag = true\n\
             \x20 let maybe: Frozen? = nil\n\
             \x20 let xs = [f]\n\
             \x20 let t = maybe.?t ?? -1\n\
             \x20 f.t = f.t + 2 - 1 * 1 / 1\n\
             \x20 let mut_target = 0\n\
             \x20 mut_target = 1\n\
             \x20 assert flag == true\n\
             \x20 while false { sink(0) }\n\
             \x20 for x in xs { sink(x.t) }\n\
             \x20 if flag { sink(1) } else { sink(2) }\n\
             \x20 let branch = if flag: 1 else: 2\n\
             \x20 let found = match pick(1) {\n\
             \x20   Lookup::Found(n) if n == 1: n\n\
             \x20   _: 0\n\
             \x20 }\n\
             \x20 with clock(f) {\n\
             \x20   let via_value = clock.now()\n\
             \x20   let via_type = clock::zero()\n\
             \x20   sink(via_value + via_type)\n\
             \x20 }\n\
             \x20 with clock<Frozen> { sink(clock::zero()) }\n\
             \x20 let shown = { f.now() }\n\
             \x20 let _unused = [e, Empty]\n\
             \x20 sink(t + branch + found + shown + mut_target + m_count(m) + text_len(text))\n\
             \x20 return 0\n\
             }\n\
             fn m_count(l: Lookup -> int) { match l { Lookup::Found(n): n\n Lookup::Missing: 0 } }\n\
             fn text_len(s: str -> int) { if s == \"x\": 1 else: 0 }\n\
             fn main() { assert true }\n",
        );

        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (_, callable) in program.callables.iter() {
            for (_, expr) in callable.body.exprs() {
                seen.insert(kind_label(&expr.kind));
            }
        }
        for form in [
            "int",
            "str",
            "bool",
            "nil",
            "local",
            "unit-struct",
            "variant",
            "field",
            "optional-field",
            "struct-lit",
            "array",
            "let",
            "assign-local",
            "assign-field",
            "neg",
            "arith",
            "eq",
            "coalesce",
            "return",
            "assert",
            "block",
            "if",
            "while",
            "for",
            "with",
            "match",
            "direct",
            "method",
            "associated",
            "slot",
            "ctor",
        ] {
            assert!(
                seen.contains(form),
                "`{form}` の形が下がっていない: {seen:?}"
            );
        }
        assert!(!seen.contains("poison"), "{seen:?}");
    }

    /// 下ろした形の名前。どの形も一度は通ったことを数えるためだけに使う
    fn kind_label(kind: &hir::ExprKind) -> &'static str {
        use hir::ExprKind::*;
        match kind {
            Int(_) => "int",
            Str(_) => "str",
            Bool(_) => "bool",
            Nil => "nil",
            Local(_) => "local",
            UnitStruct(_) => "unit-struct",
            Variant(_) => "variant",
            Field { optional: true, .. } => "optional-field",
            Field { .. } => "field",
            StructLit { .. } => "struct-lit",
            Array(_) => "array",
            Let { .. } => "let",
            AssignLocal { .. } => "assign-local",
            AssignField { .. } => "assign-field",
            Access { .. } => "access",
            Clone(_) => "clone",
            Neg(_) => "neg",
            Arith { .. } => "arith",
            Eq { .. } => "eq",
            Coalesce { .. } => "coalesce",
            Return(_) => "return",
            Assert(_) => "assert",
            Block(_) => "block",
            If { .. } => "if",
            While { .. } => "while",
            For { .. } => "for",
            With { .. } => "with",
            Match { .. } => "match",
            Call(hir::Call::Direct { .. }) => "direct",
            Call(hir::Call::Method { .. }) => "method",
            Call(hir::Call::Associated { .. }) => "associated",
            Call(hir::Call::Slot { .. }) => "slot",
            Call(hir::Call::Ctor { .. }) => "ctor",
            Poison => "poison",
        }
    }

    /// 下ろしが失敗したときの診断。文言・並び・位置と、位置がどのファイルの
    /// ものかが、モジュールを跨いでも変わらないこと(tasks 4.5)
    #[test]
    fn 下ろしの失敗はモジュールを跨いでも同じ診断を出す() {
        let loaded = crate::module::load_files(&[
            (
                "main.rd",
                "use dep::{User, mark}\nfn main() { let m = mark(1) }\n",
            ),
            (
                "dep.rd",
                "struct User { id: int }\n\
                 fn mark(u: User -> User) { u.nope }\n",
            ),
        ])
        .expect("ロードできる");
        let diagnostics =
            check_and_lower(&loaded.program).expect_err("どちらのモジュールにも誤りがある");

        let shown: Vec<&str> = diagnostics.iter().map(|d| d.msg.as_str()).collect();
        assert_eq!(
            shown,
            [
                "dep::mark: `dep::User` にフィールド `nope` はありません",
                "main::main: `dep::mark` の第 1 引数は `dep::User` ですが、`int` を渡しています",
            ],
            "宣言順に出る"
        );
        // 位置はそれぞれ自分のファイルを指す。`main.rd` が src 0
        let sources: Vec<u32> = diagnostics
            .iter()
            .map(|d| d.span.expect("実行前の診断は位置を持つ").src)
            .collect();
        assert_eq!(sources, [1, 0], "{diagnostics:?}");
    }

    /// 別のモジュールから見た同じ宣言は、正準名が同じなので同じ ID になる。
    /// 名前で引き直す段が無くなるのはこれが成り立つから
    #[test]
    fn 同じ宣言を複数モジュールから見ても一つのidになる() {
        let loaded = crate::module::load_files(&[
            (
                "main.rd",
                "use dep::{User, mark}\n\
                 fn main() { let u: User = User { id = 1 }\n let m = mark(u) }\n",
            ),
            (
                "dep.rd",
                "struct User { id: int }\n\
                 fn mark(u: User -> User) { u }\n",
            ),
        ])
        .expect("ロードできる");
        let program = check_and_lower(&loaded.program).expect("診断なしで下がるはず");

        assert_eq!(program.structs.len(), 1, "宣言は1つだけ");
        let user = hir::Type {
            reference: None,
            kind: hir::TypeKind::Struct(program.structs.ids().next().unwrap()),
            optional: false,
        };
        // 注釈・struct リテラル・引数・戻り値のすべてが同じ ID を指す
        let mark = program.free_callable("dep::mark").expect("`mark` がある");
        assert_eq!(program.callables[mark].ret, user);
        let param = program.callables[mark].params[0];
        assert_eq!(program.callables[mark].body.local(param).ty, Some(user));
    }

    /// 宣言されていない型を注釈に書くと、その型の値がどの宣言のものか誰にも
    /// 分からない。HIR へ下ろせないので実行前に止める(design.md 決定7)
    #[test]
    fn 宣言されていない型の注釈を報告する() {
        let e = only("fn f(x: Nope -> Nope) { x }\nfn main() { assert true }\n");
        assert!(e.contains("型 `Nope` は宣言されていません"), "{e}");
    }

    #[test]
    fn traitでないスロット宣言と_structでない実装先を報告する() {
        let e = only("effect db: Nope\nfn main() { assert true }\n");
        assert!(
            e.contains("effect `db`: `Nope` は trait ではありません"),
            "{e}"
        );
        let e = only("impl Nope { fn f(-> int) { 1 } }\nfn main() { assert true }\n");
        assert!(
            e.contains("impl Nope: `Nope` は struct ではありません"),
            "{e}"
        );
    }

    // ---- 正典 ----

    #[test]
    fn 正典プログラムは診断を出さない() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let errors = errors(&src);
        assert!(errors.is_empty(), "{errors:?}");
    }

    /// 正典の `self.user` が静的に検査されていること。
    #[test]
    fn 正典のループ本体は要素型で検査される() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let broken = src.replace("if self.user.id == id", "if self.user.nope == id");
        assert_ne!(broken, src, "正典の find 本体が変わったらここも直す");

        let e = only(&broken);
        assert!(e.contains("`User`"), "{e}");
        assert!(e.contains("`nope`"), "{e}");
    }

    // ---- 18. 所有と借用の型(tasks 2.1〜2.6) ----

    /// 所有の宣言だけで書いた小さなプログラム。配列・文字列・struct・enum・
    /// optional が下がった形と、Copy の分類までを1枚で固定する
    #[test]
    fn 所有の宣言の下ろしを固定する() {
        let dumped = lowered(
            "struct Tag { text: str }\n\
             struct Bag { tags: [Tag]\nlead: Tag?\nsize: int }\n\
             enum Rank { Bronze Gold }\n\
             enum Lookup { Found(Tag) Missing }\n\
             fn main() { assert true }\n",
        )
        .dump();
        assert_eq!(
            dumped,
            "struct#0 Tag { text#0: str }\n\
             struct#1 Bag { tags#1: [Tag], lead#2: Tag?, size#3: int }\n\
             enum#0 Rank { Bronze#0, Gold#1 }\n\
             enum#1 Lookup { Found#2(Tag), Missing#3 }\n\
             callable#0 fn main() -> unit\n\
             \x20 expr#0 : bool = bool true\n\
             \x20 expr#1 : unit = assert #0\n\
             \x20 root [#1]\n",
            "{dumped}"
        );
    }

    /// Copy はこの版では意図的に小さい(design.md 決定4)。
    /// 引数の宣言型をそのまま分類にかけて、所有の複合値が入らないことを見る
    #[test]
    fn copyの分類はスカラーとfieldless_enumと共有参照だけ() {
        let program = lowered(
            "struct User { id: int }\n\
             enum Rank { Bronze Gold }\n\
             enum Lookup { Found(User) Missing }\n\
             fn probe(a: int, b: bool, c: unit, d: str, e: Rank, f: Lookup, g: User,\n\
             \x20 h: [int], i: int?, j: User?, k: &User, l: &mut User) { assert true }\n\
             fn main() { assert true }\n",
        );
        let probe = program.free_callable("probe").expect("`probe` がある");
        let callable = &program.callables[probe];
        let copies: Vec<bool> = callable
            .params
            .iter()
            .map(|p| {
                let ty = callable.body.local(*p).ty.as_ref().expect("注釈がある");
                program.is_copy(ty)
            })
            .collect();
        assert_eq!(
            copies,
            [
                true,  // int
                true,  // bool
                true,  // unit
                false, // str は所有の複合値
                true,  // payload を持たない enum
                false, // payload enum
                false, // struct
                false, // 配列
                true,  // Copy を包む optional
                false, // 非 Copy を包む optional
                true,  // `&T` は複製できる能力
                false, // `&mut T` は排他なので複製できない
            ],
            "{copies:?}"
        );
    }

    /// 参照が書けるのはローカル・引数・戻り値・射影(tasks 2.3)。
    /// 所有と2つの借用が別の静的型として下がることまで見る
    #[test]
    fn 参照はローカル引数戻り値射影に書ける() {
        let dumped = lowered(
            "struct User { name: str }\n\
             fn look(u: &User -> &str) { &u.name }\n\
             fn edit(u: &mut User) { assert true }\n\
             impl User {\n\
             \x20 fn peek(&self -> str) { self.name }\n\
             \x20 fn touch(&mut self) { assert true }\n\
             \x20 fn eat(self -> str) { move self.name }\n\
             }\n\
             fn main() { let u = User { name = \"a\" }\n let view = &u\n look(view)\n assert true }\n",
        )
        .dump();
        for expected in [
            "callable#0 fn look(&User) -> &str",
            "callable#1 fn edit(&mut User) -> unit",
            "callable#2 inherent User peek(&self) -> str",
            "callable#3 inherent User touch(&mut self) -> unit",
            "callable#4 inherent User eat(self) -> str",
            "local#0 self: &User",
            "local#0 self: &mut User",
            "local#0 self: User",
            "local#1 view: &User",
            // `&u.name` は射影全体に付くので `&str`
            "expr#2 : &str = &#1",
            // `move self.name` は所有をそのまま運ぶ
            "expr#2 : str = move #1",
        ] {
            assert!(dumped.contains(expected), "{expected} が無い:\n{dumped}");
        }
    }

    /// 束縛の可変性は下ろしまで届く。`let` と `let mut` は別の宣言なので、
    /// 所有権解析が読む前に HIR がそれを持っている(tasks 2.1)
    #[test]
    fn 束縛の可変性は下ろしに残る() {
        let dumped = lowered("fn main() { let mut n = 1\n let k = 2\n assert n == k }\n").dump();
        assert!(dumped.contains("local#0 mut n: int"), "{dumped}");
        assert!(dumped.contains("local#1 k: int"), "{dumped}");
    }

    /// 期待型は借用でもそのまま枝の中へ配られる(tasks 2.2)
    #[test]
    fn 期待型は借用のまま枝へ配られる() {
        const DECL: &str = "struct User { name: str }\n";
        let errors = errors(&format!(
            "{DECL}fn pick(a: &User, b: &User, c: bool -> &User) {{ if c: a\n else: b }}\n\
             fn main() {{ assert true }}\n"
        ));
        assert!(errors.is_empty(), "{errors:?}");
        // 片方の枝だけ所有なら、その枝を指して落ちる
        let e = only(&format!(
            "{DECL}fn pick(a: &User, b: User, c: bool -> &User) {{ if c: a\n else: b }}\n\
             fn main() {{ assert true }}\n"
        ));
        assert_eq!(e, "pick: 戻り値は `&User` ですが、`User` を返しています");
    }

    /// 所有 `T`・共有 `&T`・排他 `&mut T` は別の静的型
    /// (ownership-and-borrowing spec)。宛先の照合はそのままで、緩むのは
    /// 「共有借用を受け取る位置」だけ(tasks 5.1)
    #[test]
    fn 所有と共有と排他は別の型() {
        const DECL: &str = "struct User { name: str }\n\
                            fn shared(u: &User) { assert true }\n\
                            fn owned(u: User) { assert true }\n";
        // 共有借用を所有の引数へは渡せない
        let e = only(&format!(
            "{DECL}fn main() {{ let u = User {{ name = \"a\" }}\n owned(&u) }}\n"
        ));
        assert_eq!(
            e,
            "main: `owned` の第 1 引数は `User` ですが、`&User` を渡しています"
        );
        // 注釈の位置は緩まない。自動借用が入るのは呼び出しの引数とレシーバだけ
        let e = only(&format!(
            "{DECL}fn main() {{ let u = User {{ name = \"a\" }}\n let r: &User = u\n assert true }}\n"
        ));
        assert_eq!(e, "main: `r` の初期化子は `&User` ですが、`User` です");
    }

    /// 共有借用を受け取る引数は、所有の場所からも `&mut` からも埋まる
    /// (tasks 5.1、function-signature-type-checking「Shared parameter is concise」)
    #[test]
    fn 共有借用の引数は自動で借りる() {
        const DECL: &str = "struct User { name: str }\n\
                            fn shared(u: &User -> int) { 1 }\n\
                            fn make(-> User) { User { name = \"a\" } }\n";
        for arg in ["u", "&u", "&mut u", "make()"] {
            let src =
                format!("{DECL}fn main() {{ let mut u = make()\n assert shared({arg}) == 1 }}\n");
            let errors = errors(&src);
            assert!(errors.is_empty(), "{arg}: {errors:?}");
        }
    }

    /// 共有借用の位置でも、要素型が期待型から降りてくる形は従来どおり通る。
    /// 空の配列リテラルは宛先が要素型を与えなければ型を出せない
    #[test]
    fn 共有借用の配列引数も要素型が届く() {
        const DECL: &str = "fn total(xs: &[int] -> int) { 0 }\n";
        for arg in ["[]", "[1, 2]", "xs", "&xs"] {
            let src = format!("{DECL}fn main() {{ let xs = [1]\n assert total({arg}) == 0 }}\n");
            let errors = errors(&src);
            assert!(errors.is_empty(), "{arg}: {errors:?}");
        }
        let e = only(&format!(
            "{DECL}fn main() {{ assert total([\"a\"]) == 0 }}\n"
        ));
        assert_eq!(e, "main: 配列の第 1 要素は `int` ですが、`str` です");
    }

    /// 共有借用の位置に `move` を書くと、値を失うのに渡るのは借用だけになる。
    /// 自動借用があるので修飾そのものが要らない(design.md 決定2)
    #[test]
    fn 共有借用の位置へのmoveを断る() {
        const DECL: &str = "struct User { name: str }\n\
                            impl User { fn look(&self -> int) { 1 } }\n\
                            fn shared(u: &User -> int) { 1 }\n\
                            fn make(-> User) { User { name = \"a\" } }\n";
        for body in ["assert shared(move u) == 1", "assert move u.look() == 1"] {
            let src = format!("{DECL}fn main() {{ let u = make()\n {body} }}\n");
            let found = diagnostics(&src);
            assert_eq!(found.len(), 1, "{body}: {found:?}");
            assert_eq!(
                found[0].msg, "main: 共有借用を受け取る位置に `move` は掛けられません",
                "{body}"
            );
            assert_eq!(
                found[0].help.as_deref(),
                Some("`move` を外してください。共有借用は自動で挿さります")
            );
        }
    }

    /// 挿した共有借用は HIR に残る。所有の場所のときだけで、一時値には挿さない
    #[test]
    fn 自動の共有借用はhirに残る() {
        const DECL: &str = "struct User { name: str }\n\
                            fn shared(u: &User -> int) { 1 }\n\
                            fn make(-> User) { User { name = \"a\" } }\n";
        let dumped = lowered(&format!(
            "{DECL}fn main() {{ let u = make()\n assert shared(u) == 1 }}\n"
        ))
        .dump();
        assert!(dumped.contains(": &User = &#"), "{dumped}");
        let dumped = lowered(&format!(
            "{DECL}fn main() {{ assert shared(make()) == 1 }}\n"
        ))
        .dump();
        assert!(!dumped.contains("= &#"), "一時値には挿さない: {dumped}");
    }

    /// 参照を所有の中に置く形は全部断る(tasks 2.3)
    #[test]
    fn 集約の中の参照を報告する() {
        const USER: &str = "struct User { name: str }\n";
        for (decl, expected) in [
            (
                "struct Holder { r: &User }\n",
                "struct `Holder` のフィールド `r`には参照型を書けません。集約の中に借用を置くのはこの版では未対応です",
            ),
            (
                "struct Holder { rs: [&User] }\n",
                "struct `Holder` のフィールド `rs`の配列要素には参照型を書けません。集約の中に借用を置くのはこの版では未対応です",
            ),
            (
                "enum Held { One(&mut User) }\n",
                "enum `Held` の variant `One` の第 1 payloadには参照型を書けません。集約の中に借用を置くのはこの版では未対応です",
            ),
            (
                "fn take(rs: [&User]) { assert true }\n",
                "take の引数 `rs`の配列要素には参照型を書けません。集約の中に借用を置くのはこの版では未対応です",
            ),
            (
                "fn give(-> [&User]) { [] }\n",
                "give の戻り値の配列要素には参照型を書けません。集約の中に借用を置くのはこの版では未対応です",
            ),
        ] {
            let errors = errors(&format!("{USER}{decl}fn main() {{ assert true }}\n"));
            assert!(errors.iter().any(|e| e == expected), "{decl}: {errors:?}");
        }
    }

    /// optional な参照はこの版では作れない。注釈でも式でも、注釈に書けない
    /// 経路(`nil ?? 借用` が導く `&T?`)でも同じ理由で断る(tasks 2.3)。
    ///
    /// ここが閉じていないと、`hir::Type` が約束している
    /// 「`reference` と `optional` は両立しない」が後段で破れる
    #[test]
    fn optionalな参照を報告する() {
        const USER: &str = "struct User { name: str }\n";
        for (src, expected) in [
            (
                format!(
                    "{USER}fn take(u: &User?) {{ assert true }}\nfn main() {{ assert true }}\n"
                ),
                "take の引数 `u`の型 `&User?` は optional な参照です。参照に後置 `?` は付けられません",
            ),
            (
                format!("{USER}fn take(-> &mut User?) {{ nil }}\nfn main() {{ assert true }}\n"),
                "take の戻り値の型 `&mut User?` は optional な参照です。参照に後置 `?` は付けられません",
            ),
            (
                format!("{USER}fn main() {{ let v: &mut User? = nil }}\n"),
                "main: 局所束縛 `v`の型 `&mut User?` は optional な参照です。参照に後置 `?` は付けられません",
            ),
            (
                format!("{USER}fn main(u: User?) {{ let v = &u\n assert true }}\n"),
                "main: optional な値 `User?` への参照は作れません。先に `??` で展開してください",
            ),
            // 注釈を通らない唯一の経路。`nil ?? &u` の左辺は `&User?` になる
            (
                format!(
                    "{USER}fn pick(u: &User -> &User) {{ nil ?? u }}\nfn main() {{ assert true }}\n"
                ),
                "pick: `??` の左辺 `&User?` は optional な参照です。参照に後置 `?` は付けられません",
            ),
            (
                format!(
                    "{USER}fn pick(u: &mut User -> &mut User) {{ nil ?? u }}\nfn main() {{ assert true }}\n"
                ),
                "pick: `??` の左辺 `&mut User?` は optional な参照です。参照に後置 `?` は付けられません",
            ),
        ] {
            assert_eq!(only(&src), expected, "{src}");
        }
    }

    /// 参照を所有の中に置く形は、契約・実装・注釈のどの署名でも同じ規則で断る。
    /// `check_type_shape` を呼ぶ位置に抜けがないことを固定する(tasks 2.3)
    #[test]
    fn 全ての署名位置で入れ子の参照を報告する() {
        const USER: &str = "struct User { name: str }\nstruct Store { n: int }\n";
        for (decl, expected) in [
            (
                "trait Peek { fn peek(&self, us: [&User] -> int) }\n".to_string(),
                "trait Peek::peek の引数 `us`の配列要素には参照型を書けません。集約の中に借用を置くのはこの版では未対応です",
            ),
            (
                "impl Store { fn peek(&self, us: [&mut User] -> int) { 1 } }\n".to_string(),
                "impl Store::peek の引数 `us`の配列要素には参照型を書けません。集約の中に借用を置くのはこの版では未対応です",
            ),
            (
                "fn peek(-> &User?) { nil }\n".to_string(),
                "peek の戻り値の型 `&User?` は optional な参照です。参照に後置 `?` は付けられません",
            ),
            (
                "fn peek() { let xs: [&User] = []\n assert true }\n".to_string(),
                "peek: 局所束縛 `xs`の配列要素には参照型を書けません。集約の中に借用を置くのはこの版では未対応です",
            ),
        ] {
            let errors = errors(&format!("{USER}{decl}fn main() {{ assert true }}\n"));
            assert!(errors.iter().any(|e| e == expected), "{decl}: {errors:?}");
        }
    }

    /// trait のレシーバは所有モードまで含めて契約(method-call-type-checking spec)
    #[test]
    fn 契約と食い違うレシーバのモードを報告する() {
        const CONTRACT: &str = "struct Store { n: int }\n\
                                trait Peek { fn peek(&self -> int) }\n";
        for (method, expected) in [
            (
                "fn peek(self -> int) { 1 }",
                "impl Peek for Store: `peek` のレシーバは `&self` ですが、`self` を宣言しています",
            ),
            (
                "fn peek(&mut self -> int) { 1 }",
                "impl Peek for Store: `peek` のレシーバは `&self` ですが、`&mut self` を宣言しています",
            ),
            (
                "fn peek(-> int) { 1 }",
                "impl Peek for Store: `peek` のレシーバは `&self` ですが、レシーバ無し を宣言しています",
            ),
        ] {
            let errors = errors(&format!(
                "{CONTRACT}impl Peek for Store {{ {method} }}\nfn main() {{ assert true }}\n"
            ));
            assert!(errors.iter().any(|e| e == expected), "{method}: {errors:?}");
        }
        // 宣言どおりなら診断は出ない
        let errors = errors(&format!(
            "{CONTRACT}impl Peek for Store {{ fn peek(&self -> int) {{ self.n }} }}\n\
             fn main() {{ assert true }}\n"
        ));
        assert!(errors.is_empty(), "{errors:?}");
    }

    /// レシーバの所有モードは呼び出し地点でも契約(tasks 5.2)
    const RECEIVERS: &str = "struct User { n: int }\n\
                             struct Account { user: User }\n\
                             impl User {\n\
                             \x20 fn look(&self -> int) { self.n }\n\
                             \x20 fn bump(&mut self -> int) { self.n = self.n + 1\n self.n }\n\
                             \x20 fn finish(self -> int) { self.n }\n\
                             }\n\
                             fn user(-> User) { User { n = 1 } }\n\
                             fn account(-> Account) { Account { user = user() } }\n";

    /// `&self` は修飾なしで借りる。`&mut` からも共有として再借用できる
    #[test]
    fn 共有レシーバは修飾なしで借りる() {
        for body in [
            "let u = user()\n assert u.look() == 1",
            "let u = user()\n assert (&u).look() == 1",
            "let mut u = user()\n let r = &mut u\n assert r.look() == 1",
            "assert user().look() == 1",
        ] {
            let errors = errors(&format!("{RECEIVERS}fn main() {{ {body} }}\n"));
            assert!(errors.is_empty(), "{body}: {errors:?}");
        }
    }

    /// `&mut self` と消費 `self` は修飾が要る。修飾を書けば通る
    #[test]
    fn 排他と消費のレシーバは修飾で見える() {
        for body in [
            "let mut u = user()\n assert &mut u.bump() == 2",
            "let u = user()\n assert move u.finish() == 1",
            "let mut a = account()\n assert &mut a.user.bump() == 2",
        ] {
            let errors = errors(&format!("{RECEIVERS}fn main() {{ {body} }}\n"));
            assert!(errors.is_empty(), "{body}: {errors:?}");
        }
    }

    /// 修飾を書かない排他レシーバと、借用越しの消費レシーバは断る
    #[test]
    fn レシーバの所有モードの食い違いを報告する() {
        for (body, expected, help) in [
            (
                "let mut u = user()\n assert u.bump() == 2",
                "main: `User::bump` のレシーバは `&mut User` ですが、`User` を渡しています",
                Some("`&mut` を付けて排他借用を渡してください"),
            ),
            (
                "let mut u = user()\n assert &u.bump() == 2",
                "main: `User::bump` のレシーバは `&mut User` ですが、`&User` を渡しています",
                None,
            ),
            (
                "let u = user()\n assert &u.finish() == 1",
                "main: `User::finish` のレシーバは `User` ですが、`&User` を渡しています",
                None,
            ),
            (
                "let mut a = account()\n assert a.user.bump() == 2",
                "main: `User::bump` のレシーバは `&mut User` ですが、`User` を渡しています",
                Some("`&mut` を付けて排他借用を渡してください"),
            ),
        ] {
            let src = format!("{RECEIVERS}fn main() {{ {body} }}\n");
            let found = diagnostics(&src);
            assert_eq!(found.len(), 1, "{body}: {found:?}");
            assert_eq!(found[0].msg, expected, "{body}");
            assert_eq!(found[0].help.as_deref(), help, "{body}");
        }
    }

    /// 後置の修飾はレシーバの場所を指す。`&a.user.bump()` の診断が指すのは
    /// `&a.user` で、`a` でも呼び出し全体でもない(design.md 決定2)
    #[test]
    fn レシーバ修飾は場所を指す() {
        let src = format!(
            "{RECEIVERS}fn main() {{ let mut a = account()\n assert &a.user.bump() == 2 }}\n"
        );
        assert_eq!(spanned(&src, "のレシーバは"), "&a.user");
    }

    /// 所有の内包に `indirect` の無い循環があれば、循環の辺を related で示して落とす
    /// (design.md 決定10、tasks 2.5)
    #[test]
    fn 所有の循環を報告する() {
        for (decl, main_msg, edges) in [
            (
                "struct Node { next: Node }\n",
                "型 `Node` の所有が循環しています。値の大きさが決まりません",
                vec!["`Node` のフィールド `next` が `Node` を直接持っています"],
            ),
            (
                "struct Left { r: Right }\nstruct Right { l: Left }\n",
                "型 `Left` の所有が循環しています。値の大きさが決まりません",
                vec![
                    "`Left` のフィールド `r` が `Right` を直接持っています",
                    "`Right` のフィールド `l` が `Left` を直接持っています",
                ],
            ),
            (
                "struct Node { next: Node? }\n",
                "型 `Node` の所有が循環しています。値の大きさが決まりません",
                vec!["`Node` のフィールド `next` が `Node` を直接持っています"],
            ),
            (
                "struct Node { kids: [Node] }\n",
                "型 `Node` の所有が循環しています。値の大きさが決まりません",
                vec!["`Node` のフィールド `kids` が `Node` を直接持っています"],
            ),
            (
                "enum List { Cons(int, List) Empty }\n",
                "型 `List` の所有が循環しています。値の大きさが決まりません",
                vec!["`List` の variant `Cons` の第 2 payload が `List` を直接持っています"],
            ),
        ] {
            let diagnostics = diagnostics(&format!("{decl}fn main() {{ assert true }}\n"));
            let cycle = diagnostics
                .iter()
                .find(|d| d.msg == main_msg)
                .unwrap_or_else(|| panic!("{decl}: {diagnostics:?}"));
            assert!(cycle.span.is_some(), "{decl}: 循環の診断は宣言を指す");
            let related: Vec<&str> = cycle.related.iter().map(|d| d.msg.as_str()).collect();
            assert_eq!(related, edges, "{decl}");
            assert!(
                cycle.related.iter().all(|d| d.span.is_some()),
                "{decl}: 辺も位置を持つ"
            );
        }
    }

    /// `indirect` が1本でもあれば層が切れるので受理する。同じ形が
    /// `indirect` 無しなら落ちることまで見て、効いているのが修飾だと示す
    #[test]
    fn indirectは所有の循環を切る() {
        for decl in [
            "struct Node { indirect next: Node? }\n",
            "enum List { Cons(int, indirect List) Empty }\n",
            "struct Left { indirect r: Right }\nstruct Right { l: Left }\n",
        ] {
            let src = format!("{decl}fn main() {{ assert true }}\n");
            let accepted = errors(&src);
            assert!(accepted.is_empty(), "{decl}: {accepted:?}");
            let without = src.replace("indirect ", "");
            assert_ne!(without, src);
            assert!(
                errors(&without).iter().any(|e| e.contains("所有が循環")),
                "{decl}: `indirect` を外せば落ちる"
            );
        }
    }

    /// `indirect` と参照の注釈はモジュールを跨いでも同じ宣言を指す。
    /// 正準名まで解決してから内包グラフを作るので、他モジュールの型を
    /// 間接に持つ再帰も1つの ID で閉じる
    #[test]
    fn indirectと参照はモジュールを跨いでも同じ宣言を指す() {
        let loaded = crate::module::load_files(&[
            (
                "main.rd",
                "use dep\n\
                 fn head(n: &dep::Node -> &int) { &n.id }\n\
                 fn main() { assert true }\n",
            ),
            ("dep.rd", "struct Node { id: int\nindirect next: Node? }\n"),
        ])
        .expect("ロードできる");
        let program = check_and_lower(&loaded.program).expect("診断なしで下がるはず");

        assert_eq!(program.structs.len(), 1, "宣言は1つだけ");
        let node = program.structs.ids().next().unwrap();
        let next = program.structs[node].fields[1];
        assert!(program.fields[next].indirect, "`indirect` が下がっている");
        assert_eq!(
            program.fields[next].ty,
            hir::Type {
                reference: None,
                kind: hir::TypeKind::Struct(node),
                optional: true,
            },
            "間接でも見た目の型は `Node?` のまま"
        );

        // 別モジュールの型を名指した `&dep::Node` も同じ ID を指す
        let head = program.free_callable("main::head").expect("`head` がある");
        let param = program.callables[head].params[0];
        assert_eq!(
            program.callables[head].body.local(param).ty,
            Some(hir::Type {
                reference: Some(hir::RefKind::Shared),
                kind: hir::TypeKind::Struct(node),
                optional: false,
            })
        );
        assert_eq!(
            program.show_type(&program.callables[head].ret),
            "&int",
            "戻り値の綴りも借用を出す"
        );
    }
}
