//! 構文木。
//!
//! 値ベースなので文と式を分けない。すべて `Expr`。
//! トップレベルだけは宣言しか置けないので `Item` を分けている。

use crate::lex::Span;

#[derive(Debug)]
pub struct Program {
    pub items: Vec<Item>,
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

/// `fn find(id: UserId -> User?)` — 戻り値の `->` は括弧の内側にある
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

#[derive(Debug)]
pub struct Type {
    pub name: String,
    /// `User?` の後置 `?`
    pub optional: bool,
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
    /// 変数、あるいはスロット名
    Ident(String),
    /// `Postgres::new` — 2要素以上のパス
    Path(Vec<String>),
    /// `u.rank`
    Field(Box<Expr>, String),
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
    /// `Head ':' expr` — 文法の4産出のうち2つがこれ
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

impl Expr {
    /// `Head::Ambient` の要素を「提供」として読む。`db(pg)` → `("db", [pg])`。
    ///
    /// 提供の形を知っているのはこの1箇所だけにする(要求推論と評価の両方が使う)。
    pub fn as_provision(&self) -> Option<(&str, &[Expr])> {
        let ExprKind::Call(callee, args) = &self.kind else {
            return None;
        };
        let ExprKind::Ident(slot) = &callee.kind else {
            return None;
        };
        Some((slot, args))
    }
}

#[derive(Debug)]
pub enum Head {
    If(Box<Expr>),
    Elif(Box<Expr>),
    Else,
    For { var: String, iter: Box<Expr> },
    While(Box<Expr>),
    /// `db(pg), clock(sys)` — ambient 束縛の導入。各要素は呼び出し形
    Ambient(Vec<Expr>),
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
