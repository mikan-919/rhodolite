//! 構文木。
//!
//! 値ベースなので文と式を分けない。すべて `Expr`。
//! トップレベルだけは宣言しか置けないので `Item` を分けている。

use crate::lex::Span;

#[derive(Debug)]
pub struct Program {
    pub uses: Vec<UseDecl>,
    pub items: Vec<Item>,
}

/// `use data::database` / `use data::database::{Database, db}`
#[derive(Debug, Clone)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub members: Option<Vec<UseMember>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct UseMember {
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug)]
pub enum Item {
    /// `trait Database { fn find(...) fn save(...) }` — 契約
    Trait {
        name: String,
        methods: Vec<Sig>,
        span: Span,
    },
    /// `struct User { rank: Rank }` — フィールドの宣言
    Struct {
        name: String,
        fields: Vec<(String, Type)>,
        span: Span,
    },
    /// `enum Lookup { Found(User) Skipped }` — variant ごとに0個以上の
    /// positional payload を持つ有限個の値。明示値は持たない(design.md 決定1)
    Enum {
        name: String,
        variants: Vec<EnumVariant>,
        span: Span,
    },
    /// `impl Database for Postgres { ... }` — ハンドラの正体。専用構文は持たない。
    /// `impl Postgres { ... }`(trait 無し)も書ける。`Postgres::new` はそこに置く
    Impl {
        trait_name: Option<String>,
        type_name: String,
        methods: Vec<(Sig, Vec<Expr>)>,
        span: Span,
    },
    /// `effect db: Database` — スロット宣言。関数を1つも宣言しない
    Effect {
        slot: String,
        trait_name: String,
        span: Span,
    },
    Fn {
        sig: Sig,
        body: Vec<Expr>,
        span: Span,
    },
    Test {
        name: String,
        body: Vec<Expr>,
        span: Span,
    },
}

/// `Found(User, str)` / `Skipped` — 宣言された variant。
///
/// payload 無しは長さ0の `payload` として同じ形に載せる。fieldless を別の変種に
/// しないのは、どの層でも0要素と1要素以上を同じ走査で扱うため(design.md 決定2)
#[derive(Debug)]
pub struct EnumVariant {
    pub name: String,
    /// 宣言順の payload 型
    pub payload: Vec<Type>,
}

impl Item {
    /// 宣言全体の範囲。宣言そのものを指す診断はここを使う。
    pub fn span(&self) -> Span {
        match self {
            Item::Trait { span, .. }
            | Item::Struct { span, .. }
            | Item::Enum { span, .. }
            | Item::Impl { span, .. }
            | Item::Effect { span, .. }
            | Item::Fn { span, .. }
            | Item::Test { span, .. } => *span,
        }
    }
}

/// `fn find(id: int -> User?)` — 戻り値の `->` は括弧の内側にある
#[derive(Debug)]
pub struct Sig {
    pub name: String,
    /// 第一引数が `self` か。トレイトのメソッドと関連関数の区別はこれ一つ。
    /// 暗黙にしないのは、`Postgres::new` のようにレシーバを取らないものと
    /// 見た目で区別できなくなるため
    pub has_self: bool,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

/// 型注釈。形は再帰的で、後置 `?` は**外側の**型に付く。
/// これで `[User]?`(optional な配列)と `[User?]`(optional な要素の配列)を
/// 言い分けられる(design.md 決定1)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Type {
    pub kind: TypeKind,
    /// `User?` / `[User]?` の後置 `?`
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Named(String),
    /// `[T]`
    Array(Box<Type>),
}

impl Type {
    /// 名前の葉。配列なら `None`
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            TypeKind::Named(name) => Some(name),
            TypeKind::Array(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum ExprKind {
    Int(i64),
    Str(String),
    Bool(bool),
    /// `User?` の空の側。`db.find(id) ?? return false` の左辺が取りうる値
    Nil,
    /// 変数、あるいはスロット名
    Ident(String),
    /// `Postgres::new` — 2要素以上のパス
    Path(Vec<String>),
    /// `u.rank`
    Field(Box<Expr>, String),
    /// `u.?rank` — nil を伝播する読み取り専用のフィールド射影
    OptionalField(Box<Expr>, String),
    /// `f(a, b)`。`db.save(u)` は `Call(Field(db, "save"), [u])`
    Call(Box<Expr>, Vec<Expr>),
    Array(Vec<Expr>),
    /// `Circle { r = 1.0 }`
    StructLit {
        name: String,
        fields: Vec<(String, Expr)>,
    },
    Let {
        name: String,
        value: Box<Expr>,
    },
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    Unary(UnOp, Box<Expr>),
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Return(Option<Box<Expr>>),
    Assert(Box<Expr>),
    /// `{ ... }` — 第二級。値は最後の式
    Block(Vec<Expr>),
    /// `match rank { Rank::Gold: "gold"  _: "other" }` — enum の variant ごとの
    /// 分岐。選ばれた arm の値がこの式の値になる(design.md 決定1)
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// `Head block` または `Head ':' 単純式`
    Head {
        head: Head,
        body: Box<Expr>,
        /// `if`/`elif` にだけ付く後続。`elif` は入れ子の `Head` として畳まれる。
        ///
        /// 値ベースにした以上 `if`/`elif`/`else` は兄弟文でいられない
        /// (`let x = if c: a else: b` が値を持つため)。産出は増やさず、
        /// 「`elif`/`else` は文の先頭に来られない」という整形式規則で扱う。
        orelse: Option<Box<Expr>>,
    },
}

/// `Rank::Gold: "gold"` / `Lookup::Found(user) if n > 0 { ... }` / `_: "other"`
/// — pattern と、任意の boolean guard と、既存 Head と同じ形の本体。
#[derive(Debug)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    /// `Enum::Variant(payload) if condition` の `condition`。arm が選ばれるかを
    /// 決めるので pattern ではなく arm が持つ。`_` には付けられない
    /// (design.md 決定1)。payload の束縛は本体と同じくここからも見える
    pub guard: Option<Box<Expr>>,
    pub body: Expr,
    pub span: Span,
}

/// arm 全体の pattern。限定 variant か catch-all のどちらかで、
/// payload の束縛を持てるのは前者だけ(design.md 決定1)。
#[derive(Debug)]
pub enum MatchPattern {
    /// 限定を必須にすることで arm 単体から所属 enum が分かる
    Variant {
        /// 書かれたままの enum パス。module loader が正準名へ書き換える
        enum_name: String,
        /// その enum に属する短い variant 名
        variant: String,
        /// 宣言 payload と同じ個数の平坦な pattern 要素。fieldless なら空
        /// (design.md 決定3)
        bindings: Vec<PatternBinding>,
    },
    /// `_`。先行する限定 arm が拾わなかった全 variant を受け、名前を導入しない
    CatchAll,
}

impl MatchPattern {
    /// 診断で arm を指すときの綴り。書かれたままの形をそのまま返す
    pub fn label(&self) -> String {
        match self {
            MatchPattern::Variant {
                enum_name, variant, ..
            } => format!("{enum_name}::{variant}"),
            MatchPattern::CatchAll => "_".to_string(),
        }
    }
}

/// arm pattern の1要素。入れ子も literal も無く、名前か `_` だけ。
#[derive(Debug)]
pub enum PatternBinding {
    /// 識別子。対応する宣言 payload 型を持ち、その arm 本体だけで見える
    Bind(String),
    /// `_`。値を捨て、ローカル名を導入しない
    Discard,
}

#[derive(Debug)]
pub enum Provision {
    /// `with db<Postgres> { ... }` — 実装型だけを選ぶ。
    Type { slot: String, type_name: String },
    /// `with db(store) { ... }` — 値の具体型と実体を置く。
    Value { slot: String, value: Expr },
}

impl Provision {
    pub fn slot(&self) -> &str {
        match self {
            Provision::Type { slot, .. } | Provision::Value { slot, .. } => slot,
        }
    }
}

#[derive(Debug)]
pub enum Head {
    If(Box<Expr>),
    Elif(Box<Expr>),
    Else,
    For {
        var: String,
        iter: Box<Expr>,
    },
    While(Box<Expr>),
    /// `with db(pg), clock<SystemClock>` — ambient 束縛の導入
    Ambient(Vec<Provision>),
}

#[derive(Debug, Clone, Copy)]
pub enum UnOp {
    Neg,
}

#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    /// `??`
    Coalesce,
}
