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
    /// `enum Rank { Bronze Gold }` — データを持たない有限個の値。
    /// variant は payload も明示値も持たない(design.md 決定1)
    Enum {
        name: String,
        variants: Vec<String>,
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
