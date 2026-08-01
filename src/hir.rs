//! 型付き・参照解決済みの中間表現(HIR)。
//!
//! 型検査を通ったプログラムの意味だけを持つ。構文の区別(`elif` の畳み方、
//! 裸の名前とパスの綴り、注釈の有無)は残さず、逆に検査中しか存在しなかった
//! 事実 — 全ての式の具体型、呼び出し先の宣言、フィールドと variant の所属、
//! 局所束縛の同一性 — を1度だけ確定した形で持つ(design.md 決定1)。
//!
//! ここを処理系の境界にすると、要求解析・評価・後段の C 下ろしは AST へ
//! 戻らずに済む。名前による再解決が消えるので、曖昧性もそこでは起きない。
//!
//! # ID
//!
//! 宣言・フィールド・variant・局所束縛・式はすべて `u32` の dense な index で
//! 指す。ID は**それを作った `Program` の中でだけ**意味を持ち、宣言順に振られる
//! (build をまたいで安定ではない)。種類ごとに別の newtype なので、`StructId` で
//! enum の arena を引くことは型の時点で書けない(design.md リスク「Dense IDs
//! overflow or are mixed across kinds」)。
//!
//! # 名前
//!
//! 各 arena の要素は正準表示名と宣言 span を持つ。診断と要求解析の出力は
//! そこから名前を引くだけで、意味の照合には使わない。**意味を持つ型参照に
//! 文字列は入らない**(`Type` を見れば分かる)。

/// 所有モードは構文と意味で同じ3択なので、構文木の定義をそのまま使う。
/// HIR が捨てるのは「どう書かれたか」であって「何を要求したか」ではない
pub use crate::ast::{AccessMode, ReceiverMode};
use crate::lex::Span;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::marker::PhantomData;
use std::ops::Index;

// ---------------------------------------------------------------------------
// ID と arena
// ---------------------------------------------------------------------------

/// arena の index になれるもの。`Arena` を種類ごとに別の型にするためだけの trait。
pub trait Id: Copy + Eq + Ord + std::fmt::Debug {
    fn from_index(index: usize) -> Self;
    fn index(self) -> usize;
}

/// ID newtype をまとめて宣言する。`u32` への変換はここ1箇所で検査する
/// (dense index が `u32` を超えたら確保の時点で落ちる)。
macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => { $(
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(u32);

        impl Id for $name {
            fn from_index(index: usize) -> Self {
                match u32::try_from(index) {
                    Ok(raw) => Self(raw),
                    Err(_) => panic!(concat!(stringify!($name), " の個数が u32 を超えました")),
                }
            }

            fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    )+ };
}

ids! {
    /// 本体を持つ実行単位。トップレベル関数・inherent メソッド・trait 実装
    /// メソッドを1つの arena に入れる(design.md 決定3)
    CallableId,
    StructId,
    /// struct の宣言フィールド。所属 struct を持つので単独で引ける
    FieldId,
    EnumId,
    VariantId,
    TraitId,
    /// trait が宣言するメソッド。契約なので本体を持たない。`CallableId` とは
    /// 交換できない(design.md 決定3)
    TraitMethodId,
    /// `impl Trait for Struct` 1つ
    TraitImplId,
    SlotId,
    TestId,
    /// callable / test の中で一意な局所束縛。引数・`self`・`let`・ループ変数・
    /// match payload のすべてがこれになる(design.md 決定5)
    LocalId,
    /// 1つの本体の式 arena の中で一意な式
    ExprId,
}

/// 宣言順に詰めた列。`I` でだけ引ける。
#[derive(Debug)]
pub struct Arena<I: Id, T> {
    items: Vec<T>,
    /// `I` を型の上でだけ固定する。値は持たない
    marker: PhantomData<fn() -> I>,
}

impl<I: Id, T> Default for Arena<I, T> {
    fn default() -> Self {
        Arena {
            items: Vec::new(),
            marker: PhantomData,
        }
    }
}

impl<I: Id, T> Arena<I, T> {
    /// 末尾に足して、その ID を返す。ID は確保順 = 宣言順
    pub fn alloc(&mut self, item: T) -> I {
        let id = I::from_index(self.items.len());
        self.items.push(item);
        id
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// ID と要素の組を宣言順に。出力の並びは常にこれで決める
    pub fn iter(&self) -> impl Iterator<Item = (I, &T)> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| (I::from_index(index), item))
    }

    pub fn ids(&self) -> impl Iterator<Item = I> + use<I, T> {
        (0..self.items.len()).map(I::from_index)
    }

    /// 書き換え。宣言パスで shell を作ってから本体を埋めるのに使う
    pub fn get_mut(&mut self, id: I) -> &mut T {
        let index = id.index();
        debug_assert!(index < self.items.len(), "{id:?} はこの arena の外です");
        &mut self.items[index]
    }
}

impl<I: Id, T> Index<I> for Arena<I, T> {
    type Output = T;

    fn index(&self, id: I) -> &T {
        let index = id.index();
        debug_assert!(index < self.items.len(), "{id:?} はこの arena の外です");
        &self.items[index]
    }
}

// ---------------------------------------------------------------------------
// 型
// ---------------------------------------------------------------------------

/// 組み込みのスカラー型。ユーザー宣言はこの名前を名乗れない。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Builtin {
    Int,
    Bool,
    Str,
    Unit,
}

impl Builtin {
    pub fn spelling(self) -> &'static str {
        match self {
            Builtin::Int => "int",
            Builtin::Bool => "bool",
            Builtin::Str => "str",
            Builtin::Unit => "unit",
        }
    }
}

/// 借用の強さ。所有値は `None` で表すので、ここには借用の2つしか無い
/// (design.md 決定3)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefKind {
    /// `&T` — 共有借用。Copy な能力
    Shared,
    /// `&mut T` — 排他借用。Copy ではない
    Mutable,
}

impl RefKind {
    pub fn spelling(self) -> &'static str {
        match self {
            RefKind::Shared => "&",
            RefKind::Mutable => "&mut ",
        }
    }
}

/// 値の型。同一性は形と後置 `?` と借用の強さの一致だけ(nominal, design.md 決定2)。
/// 所有 `T`・共有 `&T`・排他 `&mut T` は別の静的型(design.md 決定3)。
///
/// trait とスロットは値の型ではないのでここに現れない。制御が続かないことは
/// 型ではなく `ExprResult::Diverges` で言う。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Type {
    /// `&T` / `&mut T`。所有値は `None`。参照は借用の nominal な形なので
    /// `TypeKind` の変種ではなく、完成した所有形の外側に1つだけ付く
    /// (design.md 決定3)
    pub reference: Option<RefKind>,
    pub kind: TypeKind,
    /// `User?` / `[User]?` の後置 `?`。完成した型に1 bit 付くだけなので
    /// `[T]?` と `[T?]` を言い分けられる。
    ///
    /// optional が付くのは**所有の形**だけ。検査を通った型では
    /// `reference.is_some()` と `optional` は両立しない(design.md 決定3)
    pub optional: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TypeKind {
    Builtin(Builtin),
    Struct(StructId),
    Enum(EnumId),
    Array(Box<Type>),
    /// 型注釈が宣言されていない名前を指していた型。`ExprKind::Poison` と同じで
    /// **診断を伴うときだけ**存在する(`Program::poisoned` が検査する)
    Poison,
}

impl Type {
    pub fn builtin(builtin: Builtin) -> Self {
        Type {
            reference: None,
            kind: TypeKind::Builtin(builtin),
            optional: false,
        }
    }

    pub fn unit() -> Self {
        Type::builtin(Builtin::Unit)
    }
}

/// 式の結果(design.md 決定4)。
///
/// `Diverges` は値を産まずに制御が抜ける式。`Poison` は型検査が具体型を
/// 出せなかった式で、**診断を1件以上伴うときだけ**存在する。`Program` が
/// 外へ渡るのは診断が空のときだけなので、渡った HIR に `Poison` は無い
/// (`Program::poisoned` がそれを検査する)。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ExprResult {
    Value(Type),
    Diverges,
    Poison,
}

impl ExprResult {
    pub fn ty(&self) -> Option<&Type> {
        match self {
            ExprResult::Value(ty) => Some(ty),
            ExprResult::Diverges | ExprResult::Poison => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 宣言
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct StructDecl {
    pub name: String,
    /// 宣言順のフィールド。名前で引くのは表示のときだけ
    pub fields: Vec<FieldId>,
    pub span: Span,
}

#[derive(Debug)]
pub struct FieldDecl {
    pub name: String,
    pub owner: StructId,
    pub ty: Type,
    /// `indirect next: Node?` — 所有エッジを間接化する。値の見た目の型は `ty` の
    /// ままで、型の中身の並びからは外れるので再帰の層が切れる(design.md 決定10)
    pub indirect: bool,
    pub span: Span,
}

#[derive(Debug)]
pub struct EnumDecl {
    pub name: String,
    /// 宣言順の variant。網羅性の診断はこの順で読む
    pub variants: Vec<VariantId>,
    pub span: Span,
}

#[derive(Debug)]
pub struct VariantDecl {
    pub name: String,
    pub owner: EnumId,
    /// 宣言順の payload。fieldless は空
    pub payload: Vec<PayloadDecl>,
    pub span: Span,
}

/// `Cons(int, indirect List)` — variant の payload 1位置。
#[derive(Debug)]
pub struct PayloadDecl {
    pub ty: Type,
    /// `indirect`。`FieldDecl::indirect` と同じ意味(design.md 決定10)
    pub indirect: bool,
}

#[derive(Debug)]
pub struct TraitDecl {
    pub name: String,
    pub methods: Vec<TraitMethodId>,
    pub span: Span,
}

/// trait の契約1つ。本体は持たない(実装が `CallableId` を持つ)。
#[derive(Debug)]
pub struct TraitMethodDecl {
    pub name: String,
    pub owner: TraitId,
    /// 解決済みのレシーバ。取らないなら `None`(design.md 決定2)
    pub receiver: Option<ReceiverMode>,
    pub params: Vec<Type>,
    /// 実効戻り値型。注釈が無ければ `unit`
    pub ret: Type,
    pub span: Span,
}

#[derive(Debug)]
pub struct SlotDecl {
    pub name: String,
    pub trait_: TraitId,
    pub span: Span,
}

/// `impl Trait for Struct` 1つ。契約のメソッドから実装本体への表を持つので、
/// スロット呼び出しは提供された実装からメソッドを1手で引ける(design.md 決定6)。
#[derive(Debug)]
pub struct TraitImplDecl {
    pub trait_: TraitId,
    pub type_: StructId,
    pub methods: BTreeMap<TraitMethodId, CallableId>,
    pub span: Span,
}

/// 本体の同一性。宣言順の並びと、要求解析・評価器の本体参照に使う。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum BodyId {
    Callable(CallableId),
    Test(TestId),
}

/// 本体の持ち主。診断と要求解析の表示名はここから決まる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallableOwner {
    /// トップレベル `fn`
    Free,
    /// `impl Type { ... }`(trait 無し)
    Inherent(StructId),
    TraitImpl(TraitImplId),
}

#[derive(Debug)]
pub struct Callable {
    /// 正準表示名。トップレベルは関数名、メソッドは短いメソッド名
    pub name: String,
    pub owner: CallableOwner,
    /// 解決済みのレシーバ。`self` / `&self` / `&mut self` を言い分ける。
    /// レシーバの局所束縛の型はこのモードから決まる(design.md 決定2)
    pub receiver: Option<ReceiverMode>,
    /// `self` を含まない宣言順の引数。`self` は `Body::receiver`
    pub params: Vec<LocalId>,
    pub ret: Type,
    pub body: Body,
    pub span: Span,
}

#[derive(Debug)]
pub struct TestDecl {
    pub name: String,
    pub body: Body,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// 本体
// ---------------------------------------------------------------------------

/// 1つの実行単位の中身。式と局所束縛の arena をここに閉じるので、`ExprId` と
/// `LocalId` は本体ごとに 0 から振り直される(design.md 決定4)。
#[derive(Debug, Default)]
pub struct Body {
    exprs: Arena<ExprId, Expr>,
    locals: Arena<LocalId, LocalDecl>,
    /// `self` があればその局所束縛
    pub receiver: Option<LocalId>,
    /// 本体の式の列。値になるのは最後の式だけ
    pub root: Vec<ExprId>,
}

/// 局所束縛。名前は診断のためだけに持つ。
#[derive(Debug)]
pub struct LocalDecl {
    pub name: String,
    /// 初期化子が値を産まない `let`(`let x = return 1`)だけ `None`。
    /// そのローカルは読めない(読めば検査が落ちる)
    pub ty: Option<Type>,
    /// `let mut` で宣言されたか。引数・`self`・ループ変数・match payload は
    /// 構文上 `mut` を持てないので偽(design.md 決定2)
    pub mutable: bool,
    pub span: Span,
}

impl Body {
    pub fn alloc_expr(&mut self, expr: Expr) -> ExprId {
        self.exprs.alloc(expr)
    }

    pub fn alloc_local(&mut self, local: LocalDecl) -> LocalId {
        self.locals.alloc(local)
    }

    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id]
    }

    pub fn local(&self, id: LocalId) -> &LocalDecl {
        &self.locals[id]
    }

    pub fn exprs(&self) -> impl Iterator<Item = (ExprId, &Expr)> {
        self.exprs.iter()
    }

    pub fn locals(&self) -> impl Iterator<Item = (LocalId, &LocalDecl)> {
        self.locals.iter()
    }

    /// 場所として書けている式か。局所束縛とその非 optional なフィールド射影だけ
    /// が場所で、呼び出しや構築の結果は一時値(design.md 決定5)。
    ///
    /// 所有権解析の `place_of` は同じ形から射影の列まで組み立てる。あちらは
    /// `.?` も optional 射影の場所として畳むが、ここは「共有借用を挿してよいか」
    /// だけを決める。`.?` の結果は必ず optional で、optional な参照は作れない
    /// ので、挿せる位置がそもそも無い(design.md 決定3)
    pub fn is_place(&self, id: ExprId) -> bool {
        match &self.expr(id).kind {
            ExprKind::Local(_) => true,
            ExprKind::Field {
                recv,
                optional: false,
                ..
            } => self.is_place(*recv),
            _ => false,
        }
    }
}

#[derive(Debug)]
pub struct Expr {
    pub result: ExprResult,
    pub kind: ExprKind,
    /// 元ソースの位置。実行時診断も型診断もここを指す
    pub span: Span,
}

#[derive(Debug)]
pub enum ExprKind {
    Int(i64),
    Str(String),
    Bool(bool),
    Nil,
    Local(LocalId),
    /// フィールド0個の struct は名前だけで値になる
    UnitStruct(StructId),
    /// payload 0個の variant は名前だけで値になる
    Variant(VariantId),
    /// `u.rank` / `u.?rank`。所属は `FieldId` が持つ
    Field {
        recv: ExprId,
        field: FieldId,
        /// `.?` なら真。レシーバの `nil` をそのまま伝播する
        optional: bool,
    },
    StructLit {
        struct_: StructId,
        /// 宣言順のフィールドと値。評価順はソース順なので下ろした順を保つ
        fields: Vec<(FieldId, ExprId)>,
    },
    Array(Vec<ExprId>),
    Let {
        local: LocalId,
        value: ExprId,
    },
    AssignLocal {
        local: LocalId,
        value: ExprId,
    },
    AssignField {
        recv: ExprId,
        field: FieldId,
        value: ExprId,
    },
    /// `&place` / `&mut place` / `move place` — 解決済みの所有権修飾。
    /// 結果型は `place` の型にモードを掛けたもの(design.md 決定3)
    Access {
        mode: AccessMode,
        place: ExprId,
    },
    /// `value.clone()` — 検査器が知っている構造的な深い複製。所有を産むので
    /// レシーバは共有借用のまま残る(design.md 決定4)
    Clone(ExprId),
    Neg(ExprId),
    Arith {
        op: ArithOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Eq {
        lhs: ExprId,
        rhs: ExprId,
    },
    /// `??`。右辺は左辺が `nil` のときだけ走る
    Coalesce {
        lhs: ExprId,
        rhs: ExprId,
    },
    Return(Option<ExprId>),
    Assert(ExprId),
    Block(Vec<ExprId>),
    If {
        cond: ExprId,
        then: ExprId,
        orelse: Option<ExprId>,
    },
    While {
        cond: ExprId,
        body: ExprId,
    },
    For {
        var: LocalId,
        iter: ExprId,
        body: ExprId,
    },
    /// `with db(store), clock<SystemClock> { ... }`
    With {
        provisions: Vec<Provision>,
        body: ExprId,
    },
    Match {
        subject: ExprId,
        arms: Vec<MatchArm>,
    },
    Call(Call),
    /// 型検査が具体型を出せなかった式。診断を伴うときだけ存在する
    Poison,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl ArithOp {
    pub fn spelling(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
        }
    }
}

/// `with` の提供1つ。実装型は静的に決まっているので `TraitImplId` を持つ。
/// 実体を置く提供だけ値の式を持つ(design.md 決定6)。
#[derive(Debug)]
pub struct Provision {
    pub slot: SlotId,
    pub implementation: TraitImplId,
    /// `with db(store)` の `store`。`with db<Postgres>` は `None`
    pub value: Option<ExprId>,
    pub span: Span,
}

#[derive(Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<ExprId>,
    pub body: ExprId,
    pub span: Span,
}

#[derive(Debug)]
pub enum Pattern {
    Variant {
        variant: VariantId,
        /// 宣言 payload と同じ個数。`_` は `None`
        bindings: Vec<Option<LocalId>>,
    },
    CatchAll,
}

/// 既に選ばれている呼び出し。名前で引き直す形は無い(design.md 決定6)。
#[derive(Debug)]
pub enum Call {
    /// `stamp(u)` — トップレベル関数
    Direct {
        callable: CallableId,
        args: Vec<ExprId>,
    },
    /// `store.get(id)` — 具体型のメソッド
    Method {
        callable: CallableId,
        recv: ExprId,
        args: Vec<ExprId>,
    },
    /// `Postgres::new(url)` — レシーバを取らない関連関数
    Associated {
        callable: CallableId,
        args: Vec<ExprId>,
    },
    /// `db.save(u)` / `db::make()` — スロット経由。実行する本体は、その場の
    /// 提供が持つ実装から `method` で引く
    Slot {
        slot: SlotId,
        method: TraitMethodId,
        receiver: SlotReceiver,
        /// スロットを名指している部分(`db.save` / `db::make`)の位置。
        /// 呼び出し全体より狭い。提供忘れの診断は使用地点としてここを指す
        slot_span: Span,
        args: Vec<ExprId>,
    },
    /// `Lookup::Found(user)` — enum の構築
    Ctor {
        variant: VariantId,
        args: Vec<ExprId>,
    },
}

/// スロット呼び出しがスロットに要求する強さ。`.` は実体、`::` は型だけ。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotReceiver {
    Value,
    Type,
}

// ---------------------------------------------------------------------------
// プログラム
// ---------------------------------------------------------------------------

/// 型検査を通ったプログラム全体。arena はすべて宣言順。
#[derive(Debug, Default)]
pub struct Program {
    pub structs: Arena<StructId, StructDecl>,
    pub fields: Arena<FieldId, FieldDecl>,
    pub enums: Arena<EnumId, EnumDecl>,
    pub variants: Arena<VariantId, VariantDecl>,
    pub traits: Arena<TraitId, TraitDecl>,
    pub trait_methods: Arena<TraitMethodId, TraitMethodDecl>,
    pub trait_impls: Arena<TraitImplId, TraitImplDecl>,
    pub slots: Arena<SlotId, SlotDecl>,
    pub callables: Arena<CallableId, Callable>,
    pub tests: Arena<TestId, TestDecl>,
    /// 宣言順の本体。要求の一覧の並びは宣言順なので、arena が種類ごとに
    /// 分かれていても元の順を復元できるようここに持つ
    pub bodies: Vec<BodyId>,
}

impl Program {
    /// 名前で引けるトップレベル関数。エントリの解決だけに使う
    pub fn free_callable(&self, name: &str) -> Option<CallableId> {
        self.callables
            .iter()
            .find(|(_, c)| c.owner == CallableOwner::Free && c.name == name)
            .map(|(id, _)| id)
    }

    /// 診断に出す型の綴り。ID を表示の直前で名前へ戻す唯一の場所
    pub fn show_type(&self, ty: &Type) -> String {
        let mut out = match &ty.kind {
            TypeKind::Builtin(builtin) => builtin.spelling().to_string(),
            TypeKind::Struct(id) => self.structs[*id].name.clone(),
            TypeKind::Enum(id) => self.enums[*id].name.clone(),
            TypeKind::Array(element) => format!("[{}]", self.show_type(element)),
            TypeKind::Poison => "?".to_string(),
        };
        if ty.optional {
            out.push('?');
        }
        match ty.reference {
            Some(kind) => format!("{}{out}", kind.spelling()),
            None => out,
        }
    }

    /// 暗黙に複製してよい型か(design.md 決定4)。
    ///
    /// Copy はこの版では意図的に小さい。スカラー・payload を持たない enum・
    /// 共有借用だけで、`&mut T` と所有の複合値(struct・payload enum・`str`・
    /// 配列・非 Copy を包む optional)は入らない。`copy struct` の opt-in は
    /// 後の capability に残してある
    pub fn is_copy(&self, ty: &Type) -> bool {
        match ty.reference {
            // `&T` は複製できる能力、`&mut T` は排他なので複製できない
            Some(RefKind::Shared) => return true,
            Some(RefKind::Mutable) => return false,
            None => {}
        }
        match &ty.kind {
            TypeKind::Builtin(Builtin::Int | Builtin::Bool | Builtin::Unit) => true,
            TypeKind::Builtin(Builtin::Str) => false,
            TypeKind::Struct(_) => false,
            // payload を1つでも持つ enum は所有の複合値
            TypeKind::Enum(id) => self.enums[*id]
                .variants
                .iter()
                .all(|v| self.variants[*v].payload.is_empty()),
            TypeKind::Array(_) => false,
            TypeKind::Poison => false,
        }
    }

    pub fn body(&self, id: BodyId) -> &Body {
        match id {
            BodyId::Callable(id) => &self.callables[id].body,
            BodyId::Test(id) => &self.tests[id].body,
        }
    }

    /// 要求解析と診断が使う本体の表示名。
    ///
    /// `impl` のメソッドは trait ではなく**実装先の型**で綴る。同じ契約の別の
    /// 実装は別の本体なので、具体的な呼び出し先を指すときは型で言い分ける
    /// (契約そのものを指す綴りは `show_trait_method`)。
    pub fn show_callable(&self, id: CallableId) -> String {
        let callable = &self.callables[id];
        let owner = match callable.owner {
            CallableOwner::Free => return callable.name.clone(),
            CallableOwner::Inherent(type_) => type_,
            CallableOwner::TraitImpl(impl_) => self.trait_impls[impl_].type_,
        };
        format!("impl {}::{}", self.structs[owner].name, callable.name)
    }

    /// 契約メソッドの表示名。スロット経由の呼び出しはこれを指す
    pub fn show_trait_method(&self, id: TraitMethodId) -> String {
        let method = &self.trait_methods[id];
        format!("impl {}::{}", self.traits[method.owner].name, method.name)
    }

    /// 本体の表示名。
    pub fn show_body(&self, id: BodyId) -> String {
        match id {
            BodyId::Callable(id) => self.show_callable(id),
            BodyId::Test(id) => format!("test {:?}", self.tests[id].name),
        }
    }

    /// `impl Trait for Type` が実装する契約メソッドの本体。
    pub fn implementation_of(
        &self,
        impl_: TraitImplId,
        method: TraitMethodId,
    ) -> Option<CallableId> {
        self.trait_impls[impl_].methods.get(&method).copied()
    }

    /// 型検査が成功したのに残っている未解決。空でなければ検査器の不具合
    /// (design.md 決定7)。最初に見つけた1件の span を返す。
    ///
    /// 見るのは `Poison` な式・結果型・宣言型の3つ。呼び出し先とフィールドと
    /// 提供は ID しか持てない形なので、構築できた時点で解決済み
    pub fn poisoned(&self) -> Option<Span> {
        for (_, field) in self.fields.iter() {
            if poisoned_type(&field.ty) {
                return Some(field.span);
            }
        }
        for (_, variant) in self.variants.iter() {
            if variant.payload.iter().any(|p| poisoned_type(&p.ty)) {
                return Some(variant.span);
            }
        }
        for (_, method) in self.trait_methods.iter() {
            if method.params.iter().any(poisoned_type) || poisoned_type(&method.ret) {
                return Some(method.span);
            }
        }
        for (_, callable) in self.callables.iter() {
            if poisoned_type(&callable.ret) {
                return Some(callable.span);
            }
        }
        let bodies = self
            .callables
            .iter()
            .map(|(_, c)| &c.body)
            .chain(self.tests.iter().map(|(_, t)| &t.body));
        for body in bodies {
            for (_, local) in body.locals() {
                if local.ty.as_ref().is_some_and(poisoned_type) {
                    return Some(local.span);
                }
            }
            for (_, expr) in body.exprs() {
                if matches!(expr.kind, ExprKind::Poison)
                    || expr.result == ExprResult::Poison
                    || expr.result.ty().is_some_and(poisoned_type)
                {
                    return Some(expr.span);
                }
            }
        }
        None
    }
}

/// 名前を解決できなかった型を含むか。配列は要素まで辿る
fn poisoned_type(ty: &Type) -> bool {
    match &ty.kind {
        TypeKind::Poison => true,
        TypeKind::Array(element) => poisoned_type(element),
        TypeKind::Builtin(_) | TypeKind::Struct(_) | TypeKind::Enum(_) => false,
    }
}

// ---------------------------------------------------------------------------
// 決定的な描画
// ---------------------------------------------------------------------------

impl Program {
    /// 宣言順に全 arena を書き出す。同じプログラムからは常に同じ文字列が出るので、
    /// 下ろしのスナップショットテストはこれを比べる(tasks 2.7 / 3.9)。
    ///
    /// span は載せない(バイト位置は無関係な編集で動くため)。
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (id, decl) in self.structs.iter() {
            let fields: Vec<String> = decl
                .fields
                .iter()
                .map(|f| {
                    let field = &self.fields[*f];
                    format!(
                        "{}{}#{}: {}",
                        if field.indirect { "indirect " } else { "" },
                        field.name,
                        f.index(),
                        self.show_type(&field.ty)
                    )
                })
                .collect();
            let _ = writeln!(
                out,
                "struct#{} {} {{ {} }}",
                id.index(),
                decl.name,
                fields.join(", ")
            );
        }
        for (id, decl) in self.enums.iter() {
            let variants: Vec<String> = decl
                .variants
                .iter()
                .map(|v| {
                    let variant = &self.variants[*v];
                    let payload: Vec<String> = variant
                        .payload
                        .iter()
                        .map(|p| {
                            format!(
                                "{}{}",
                                if p.indirect { "indirect " } else { "" },
                                self.show_type(&p.ty)
                            )
                        })
                        .collect();
                    if payload.is_empty() {
                        format!("{}#{}", variant.name, v.index())
                    } else {
                        format!("{}#{}({})", variant.name, v.index(), payload.join(", "))
                    }
                })
                .collect();
            let _ = writeln!(
                out,
                "enum#{} {} {{ {} }}",
                id.index(),
                decl.name,
                variants.join(", ")
            );
        }
        for (id, decl) in self.traits.iter() {
            let _ = writeln!(out, "trait#{} {}", id.index(), decl.name);
            for method in &decl.methods {
                let sig = &self.trait_methods[*method];
                let _ = writeln!(
                    out,
                    "  method#{} {}{}",
                    method.index(),
                    sig.name,
                    self.show_signature(sig.receiver, &sig.params, &sig.ret)
                );
            }
        }
        for (id, decl) in self.slots.iter() {
            let _ = writeln!(
                out,
                "slot#{} {}: {}",
                id.index(),
                decl.name,
                self.traits[decl.trait_].name
            );
        }
        for (id, decl) in self.trait_impls.iter() {
            let _ = writeln!(
                out,
                "impl#{} {} for {}",
                id.index(),
                self.traits[decl.trait_].name,
                self.structs[decl.type_].name
            );
            for (method, callable) in &decl.methods {
                let _ = writeln!(
                    out,
                    "  {} -> callable#{}",
                    self.trait_methods[*method].name,
                    callable.index()
                );
            }
        }
        for (id, decl) in self.callables.iter() {
            let owner = match decl.owner {
                CallableOwner::Free => "fn".to_string(),
                CallableOwner::Inherent(type_) => {
                    format!("inherent {}", self.structs[type_].name)
                }
                CallableOwner::TraitImpl(impl_) => format!("impl#{}", impl_.index()),
            };
            let params: Vec<Type> = decl
                .params
                .iter()
                .filter_map(|p| decl.body.local(*p).ty.clone())
                .collect();
            let _ = writeln!(
                out,
                "callable#{} {} {}{}",
                id.index(),
                owner,
                decl.name,
                self.show_signature(decl.receiver, &params, &decl.ret)
            );
            self.dump_body(&decl.body, &mut out);
        }
        for (id, decl) in self.tests.iter() {
            let _ = writeln!(out, "test#{} {:?}", id.index(), decl.name);
            self.dump_body(&decl.body, &mut out);
        }
        out
    }

    fn show_signature(
        &self,
        receiver: Option<ReceiverMode>,
        params: &[Type],
        ret: &Type,
    ) -> String {
        let mut shown: Vec<String> = Vec::new();
        if let Some(mode) = receiver {
            shown.push(
                match mode {
                    ReceiverMode::Owned => "self",
                    ReceiverMode::Shared => "&self",
                    ReceiverMode::Mutable => "&mut self",
                }
                .to_string(),
            );
        }
        shown.extend(params.iter().map(|t| self.show_type(t)));
        format!("({}) -> {}", shown.join(", "), self.show_type(ret))
    }

    fn dump_body(&self, body: &Body, out: &mut String) {
        for (id, local) in body.locals() {
            let ty = match &local.ty {
                Some(ty) => self.show_type(ty),
                None => "?".to_string(),
            };
            let _ = writeln!(
                out,
                "  local#{} {}{}: {}",
                id.index(),
                if local.mutable { "mut " } else { "" },
                local.name,
                ty
            );
        }
        for (id, expr) in body.exprs() {
            let result = match &expr.result {
                ExprResult::Value(ty) => self.show_type(ty),
                ExprResult::Diverges => "!".to_string(),
                ExprResult::Poison => "poison".to_string(),
            };
            let _ = writeln!(
                out,
                "  expr#{} : {} = {}",
                id.index(),
                result,
                self.show_kind(&expr.kind)
            );
        }
        let root: Vec<String> = body
            .root
            .iter()
            .map(|e| format!("#{}", e.index()))
            .collect();
        let _ = writeln!(out, "  root [{}]", root.join(" "));
    }

    fn show_kind(&self, kind: &ExprKind) -> String {
        let args = |ids: &[ExprId]| -> String {
            ids.iter()
                .map(|a| format!("#{}", a.index()))
                .collect::<Vec<_>>()
                .join(" ")
        };
        match kind {
            ExprKind::Int(n) => format!("int {n}"),
            ExprKind::Str(s) => format!("str {s:?}"),
            ExprKind::Bool(b) => format!("bool {b}"),
            ExprKind::Nil => "nil".to_string(),
            ExprKind::Local(local) => format!("local#{}", local.index()),
            ExprKind::UnitStruct(id) => format!("unit-struct {}", self.structs[*id].name),
            ExprKind::Variant(id) => format!("variant {}", self.variants[*id].name),
            ExprKind::Field {
                recv,
                field,
                optional,
            } => format!(
                "field{} #{} .{}",
                if *optional { "?" } else { "" },
                recv.index(),
                self.fields[*field].name
            ),
            ExprKind::StructLit { struct_, fields } => {
                let shown: Vec<String> = fields
                    .iter()
                    .map(|(f, v)| format!("{}=#{}", self.fields[*f].name, v.index()))
                    .collect();
                format!(
                    "struct-lit {} {{ {} }}",
                    self.structs[*struct_].name,
                    shown.join(", ")
                )
            }
            ExprKind::Array(items) => format!("array [{}]", args(items)),
            ExprKind::Let { local, value } => {
                format!("let local#{} = #{}", local.index(), value.index())
            }
            ExprKind::AssignLocal { local, value } => {
                format!("assign local#{} = #{}", local.index(), value.index())
            }
            ExprKind::AssignField { recv, field, value } => format!(
                "assign #{}.{} = #{}",
                recv.index(),
                self.fields[*field].name,
                value.index()
            ),
            ExprKind::Access { mode, place } => format!(
                "{}#{}",
                match mode {
                    AccessMode::Shared => "&",
                    AccessMode::Mutable => "&mut ",
                    AccessMode::Move => "move ",
                },
                place.index()
            ),
            ExprKind::Clone(inner) => format!("clone #{}", inner.index()),
            ExprKind::Neg(inner) => format!("neg #{}", inner.index()),
            ExprKind::Arith { op, lhs, rhs } => {
                format!("{} #{} #{}", op.spelling(), lhs.index(), rhs.index())
            }
            ExprKind::Eq { lhs, rhs } => format!("== #{} #{}", lhs.index(), rhs.index()),
            ExprKind::Coalesce { lhs, rhs } => format!("?? #{} #{}", lhs.index(), rhs.index()),
            ExprKind::Return(Some(value)) => format!("return #{}", value.index()),
            ExprKind::Return(None) => "return".to_string(),
            ExprKind::Assert(inner) => format!("assert #{}", inner.index()),
            ExprKind::Block(body) => format!("block [{}]", args(body)),
            ExprKind::If { cond, then, orelse } => match orelse {
                Some(orelse) => format!(
                    "if #{} then #{} else #{}",
                    cond.index(),
                    then.index(),
                    orelse.index()
                ),
                None => format!("if #{} then #{}", cond.index(), then.index()),
            },
            ExprKind::While { cond, body } => {
                format!("while #{} #{}", cond.index(), body.index())
            }
            ExprKind::For { var, iter, body } => format!(
                "for local#{} in #{} #{}",
                var.index(),
                iter.index(),
                body.index()
            ),
            ExprKind::With { provisions, body } => {
                let shown: Vec<String> = provisions
                    .iter()
                    .map(|p| {
                        let target = format!(
                            "{}=impl#{}",
                            self.slots[p.slot].name,
                            p.implementation.index()
                        );
                        match p.value {
                            Some(value) => format!("{target}(#{})", value.index()),
                            None => target,
                        }
                    })
                    .collect();
                format!("with {} #{}", shown.join(", "), body.index())
            }
            ExprKind::Match { subject, arms } => {
                let shown: Vec<String> = arms
                    .iter()
                    .map(|arm| {
                        let pattern = match &arm.pattern {
                            Pattern::Variant { variant, bindings } => {
                                let bound: Vec<String> = bindings
                                    .iter()
                                    .map(|b| match b {
                                        Some(local) => format!("local#{}", local.index()),
                                        None => "_".to_string(),
                                    })
                                    .collect();
                                if bound.is_empty() {
                                    self.variants[*variant].name.clone()
                                } else {
                                    format!(
                                        "{}({})",
                                        self.variants[*variant].name,
                                        bound.join(", ")
                                    )
                                }
                            }
                            Pattern::CatchAll => "_".to_string(),
                        };
                        match arm.guard {
                            Some(guard) => {
                                format!("{pattern} if #{} -> #{}", guard.index(), arm.body.index())
                            }
                            None => format!("{pattern} -> #{}", arm.body.index()),
                        }
                    })
                    .collect();
                format!("match #{} [{}]", subject.index(), shown.join(" | "))
            }
            ExprKind::Call(call) => match call {
                Call::Direct { callable, args: a } => {
                    format!("call callable#{}({})", callable.index(), args(a))
                }
                Call::Method {
                    callable,
                    recv,
                    args: a,
                } => format!(
                    "call #{}.callable#{}({})",
                    recv.index(),
                    callable.index(),
                    args(a)
                ),
                Call::Associated { callable, args: a } => {
                    format!("call ::callable#{}({})", callable.index(), args(a))
                }
                Call::Slot {
                    slot,
                    method,
                    receiver,
                    args: a,
                    ..
                } => format!(
                    "call slot {}{}{}({})",
                    self.slots[*slot].name,
                    match receiver {
                        SlotReceiver::Value => ".",
                        SlotReceiver::Type => "::",
                    },
                    self.trait_methods[*method].name,
                    args(a)
                ),
                Call::Ctor { variant, args: a } => {
                    format!("ctor {}({})", self.variants[*variant].name, args(a))
                }
            },
            ExprKind::Poison => "poison".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            src: 0,
            start: 0,
            end: 0,
        }
    }

    #[test]
    fn idは確保順に振られる() {
        let mut arena: Arena<StructId, StructDecl> = Arena::default();
        let first = arena.alloc(StructDecl {
            name: "A".to_string(),
            fields: Vec::new(),
            span: span(),
        });
        let second = arena.alloc(StructDecl {
            name: "B".to_string(),
            fields: Vec::new(),
            span: span(),
        });
        assert_eq!(first.index(), 0);
        assert_eq!(second.index(), 1);
        assert!(first < second, "宣言順が ID の順でもある");
        assert_eq!(arena[first].name, "A");
        assert_eq!(arena[second].name, "B");
    }

    /// 種類ごとに別の newtype なので、他の宣言種の ID で引くことは書けない。
    /// `Arena<StructId, _>[EnumId]` は型が合わずコンパイルできないので、
    /// ここでは同じ index の別種 ID が別の値であることだけを確かめる。
    #[test]
    fn 別の宣言種のidは混ざらない() {
        let struct_id = StructId::from_index(3);
        let enum_id = EnumId::from_index(3);
        assert_eq!(struct_id.index(), enum_id.index());
        assert_eq!(format!("{struct_id}"), "StructId(3)");
        assert_eq!(format!("{enum_id}"), "EnumId(3)");
    }

    #[test]
    fn 式とローカルのidは本体ごとに0から振り直される() {
        let mut program = Program::default();
        let mut body = Body::default();
        let local = body.alloc_local(LocalDecl {
            name: "n".to_string(),
            ty: Some(Type::builtin(Builtin::Int)),
            mutable: false,
            span: span(),
        });
        let value = body.alloc_expr(Expr {
            result: ExprResult::Value(Type::builtin(Builtin::Int)),
            kind: ExprKind::Local(local),
            span: span(),
        });
        body.root.push(value);
        program.callables.alloc(Callable {
            name: "main".to_string(),
            owner: CallableOwner::Free,
            receiver: None,
            params: vec![local],
            ret: Type::builtin(Builtin::Int),
            body,
            span: span(),
        });

        let mut other = Body::default();
        let first = other.alloc_expr(Expr {
            result: ExprResult::Value(Type::unit()),
            kind: ExprKind::Poison,
            span: span(),
        });
        assert_eq!(first.index(), 0, "本体が変われば ExprId は 0 から");
        other.root.push(first);
        program.tests.alloc(TestDecl {
            name: "t".to_string(),
            body: other,
            span: span(),
        });

        assert_eq!(program.free_callable("main").map(Id::index), Some(0));
        assert!(program.poisoned().is_some(), "Poison は検査で見つかる");
    }

    #[test]
    fn dumpは宣言順で決定的() {
        let mut program = Program::default();
        let point = program.structs.alloc(StructDecl {
            name: "Point".to_string(),
            fields: Vec::new(),
            span: span(),
        });
        let x = program.fields.alloc(FieldDecl {
            name: "x".to_string(),
            owner: point,
            ty: Type::builtin(Builtin::Int),
            indirect: false,
            span: span(),
        });
        program.structs.get_mut(point).fields.push(x);

        let mut body = Body::default();
        let literal = body.alloc_expr(Expr {
            result: ExprResult::Value(Type::builtin(Builtin::Int)),
            kind: ExprKind::Int(1),
            span: span(),
        });
        body.root.push(literal);
        program.callables.alloc(Callable {
            name: "main".to_string(),
            owner: CallableOwner::Free,
            receiver: None,
            params: Vec::new(),
            ret: Type::builtin(Builtin::Int),
            body,
            span: span(),
        });

        let dumped = program.dump();
        assert_eq!(dumped, program.dump(), "同じ Program からは同じ文字列");
        assert!(dumped.contains("struct#0 Point"), "{dumped}");
        assert!(dumped.contains("callable#0 fn main() -> int"), "{dumped}");
        assert!(dumped.contains("expr#0 : int = int 1"), "{dumped}");
    }
}
