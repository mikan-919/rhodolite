//! 再帰下降パーサ。
//!
//! 優先順位は関数の入れ子で表す。低い段が高い段を呼んでから自分の演算子を探す:
//!
//!   assign → coalesce → equality → additive → multiplicative → unary → postfix → primary
//!
//! Head は `if` / `for` / `while` / `with` のキーワードから始まる。
//! ブロック形はそのまま `{}`、一行形だけ `:` で本体を区切る。

use crate::ast::*;
use crate::diag::Diag;
use crate::lex::{Span, Tok, Token};

type PResult<T> = Result<T, Diag>;

pub fn parse(tokens: &[Token]) -> PResult<Program> {
    let mut p = Parser {
        toks: tokens,
        pos: 0,
        no_struct: false,
    };
    p.program()
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    /// Head の条件式を読んでいる間だけ真。`while c { }` の `c { }` を
    /// struct リテラルと解釈しないようにする(Rust と同じ回避)。
    no_struct: bool,
}

// ---------------------------------------------------------------------------
// トークン列の上を歩く道具
// ---------------------------------------------------------------------------

impl<'a> Parser<'a> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn line(&self) -> u32 {
        self.toks[self.pos].line
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn bump(&mut self) -> &'a Token {
        let t = &self.toks[self.pos];
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok, what: &str) -> PResult<&'a Token> {
        if self.at(t) {
            Ok(self.bump())
        } else {
            Err(self.err(&format!("{} が必要です (実際は {:?})", what, self.peek())))
        }
    }

    fn expect_ident(&mut self, what: &str) -> PResult<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            // `use` はトップレベル接頭部でだけキーワードとして扱う。
            // 既存プログラムの `fn use()` / `use(x)` は引き続き識別子として読める。
            Tok::Use => {
                self.bump();
                Ok("use".to_string())
            }
            Tok::As => {
                self.bump();
                Ok("as".to_string())
            }
            other => Err(self.err(&format!("{} が必要です (実際は {:?})", what, other))),
        }
    }

    /// 現在のトークンを主 span に取る。読み進めた位置がそのまま原因の位置になる。
    fn err(&self, msg: &str) -> Diag {
        Diag::at(self.span(), msg)
    }

    fn skip_newlines(&mut self) {
        while self.at(&Tok::Newline) {
            self.bump();
        }
    }

    fn to(&self, start: Span) -> Span {
        let end = self.toks[self.pos.saturating_sub(1)].span.end;
        Span {
            src: start.src,
            start: start.start,
            end,
        }
    }
}

// ---------------------------------------------------------------------------
// トップレベル
// ---------------------------------------------------------------------------

impl<'a> Parser<'a> {
    fn program(&mut self) -> PResult<Program> {
        let mut uses = Vec::new();
        let mut items = Vec::new();
        self.skip_newlines();
        while self.at(&Tok::Use) {
            uses.push(self.use_decl()?);
            self.skip_newlines();
        }
        loop {
            self.skip_newlines();
            if self.at(&Tok::Eof) {
                break;
            }
            if self.at(&Tok::Use) {
                return Err(self.err("`use` はモジュール先頭の接頭部にだけ書けます"));
            }
            items.push(self.item()?);
        }
        Ok(Program { uses, items })
    }

    fn use_decl(&mut self) -> PResult<UseDecl> {
        let start = self.span();
        self.expect(&Tok::Use, "`use`")?;
        let first = self.expect_ident("モジュール名")?;
        if first == "super" || first == "crate" {
            return Err(self.err("`use` にはソースルート基準の絶対モジュールパスが必要です"));
        }
        let mut path = vec![first];

        while self.eat(&Tok::ColonColon) {
            if self.eat(&Tok::LBrace) {
                let mut members = Vec::new();
                self.skip_newlines();
                if self.at(&Tok::RBrace) {
                    return Err(self.err("`use` の選択リストは空にできません"));
                }
                loop {
                    let name = self.expect_ident("選択するメンバー名")?;
                    let alias = if self.eat(&Tok::As) {
                        Some(self.expect_ident("別名")?)
                    } else {
                        None
                    };
                    members.push(UseMember { name, alias });
                    self.skip_newlines();
                    if !self.eat(&Tok::Comma) {
                        self.expect(&Tok::RBrace, "`}`")?;
                        break;
                    }
                    self.skip_newlines();
                    if self.eat(&Tok::RBrace) {
                        break;
                    }
                }
                return Ok(UseDecl {
                    path,
                    alias: None,
                    members: Some(members),
                    span: self.to(start),
                });
            }
            path.push(self.expect_ident("モジュールパスの続き")?);
        }

        let alias = if self.eat(&Tok::As) {
            Some(self.expect_ident("別名")?)
        } else {
            None
        };
        Ok(UseDecl {
            path,
            alias,
            members: None,
            span: self.to(start),
        })
    }

    fn item(&mut self) -> PResult<Item> {
        let start = self.span();
        match self.peek() {
            Tok::Trait => {
                self.bump();
                let name = self.expect_ident("trait 名")?;
                self.expect(&Tok::LBrace, "`{`")?;
                let mut methods = Vec::new();
                loop {
                    self.skip_newlines();
                    if self.eat(&Tok::RBrace) {
                        break;
                    }
                    self.expect(&Tok::Fn, "`fn`")?;
                    methods.push(self.sig()?);
                }
                Ok(Item::Trait {
                    name,
                    methods,
                    span: self.to(start),
                })
            }

            // `struct User { rank: Rank }`。`struct Gold {}` のようにフィールド0個も書ける
            Tok::Struct => {
                self.bump();
                let name = self.expect_ident("struct 名")?;
                self.expect(&Tok::LBrace, "`{`")?;
                let mut fields = Vec::new();
                loop {
                    self.skip_newlines();
                    if self.eat(&Tok::RBrace) {
                        break;
                    }
                    let fname = self.expect_ident("フィールド名")?;
                    self.expect(&Tok::Colon, "`:`")?;
                    fields.push((fname, self.ty()?));
                    self.eat(&Tok::Comma);
                }
                Ok(Item::Struct {
                    name,
                    fields,
                    span: self.to(start),
                })
            }

            // `enum Lookup { Found(User) Skipped }`。payload は関数引数と同じ
            // positional な型の並び。`enum Never {}` のように variant 0個も書ける
            Tok::Enum => {
                self.bump();
                let name = self.expect_ident("enum 名")?;
                self.expect(&Tok::LBrace, "`{`")?;
                let mut variants = Vec::new();
                loop {
                    self.skip_newlines();
                    if self.eat(&Tok::RBrace) {
                        break;
                    }
                    let vname = self.expect_ident("variant 名")?;
                    let mut payload = Vec::new();
                    if self.eat(&Tok::LParen) {
                        while !self.at(&Tok::RParen) {
                            payload.push(self.ty()?);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                        self.expect(&Tok::RParen, "`)` または payload の型")?;
                    }
                    variants.push(EnumVariant {
                        name: vname,
                        payload,
                    });
                    self.eat(&Tok::Comma);
                }
                Ok(Item::Enum {
                    name,
                    variants,
                    span: self.to(start),
                })
            }

            // `impl Database for Postgres { ... }` / `impl Postgres { ... }`
            // ハンドラに専用構文は無い(CONTEXT.md「ハンドラ」)。ただの impl。
            Tok::Impl => {
                self.bump();
                let first = self.name_path("trait 名または型名")?;
                let (trait_name, type_name) = if self.eat(&Tok::For) {
                    (Some(first), self.name_path("型名")?)
                } else {
                    (None, first)
                };
                self.expect(&Tok::LBrace, "`{`")?;
                let mut methods = Vec::new();
                loop {
                    self.skip_newlines();
                    if self.eat(&Tok::RBrace) {
                        break;
                    }
                    self.expect(&Tok::Fn, "`fn`")?;
                    let sig = self.sig()?;
                    let body = self.block()?;
                    methods.push((sig, body));
                }
                Ok(Item::Impl {
                    trait_name,
                    type_name,
                    methods,
                    span: self.to(start),
                })
            }

            // `effect db: Database` — 関数を1つも宣言しない。契約と役割を分けて書く
            Tok::Effect => {
                self.bump();
                let slot = self.expect_ident("スロット名")?;
                self.expect(&Tok::Colon, "`:`")?;
                let trait_name = self.name_path("trait 名")?;
                Ok(Item::Effect {
                    slot,
                    trait_name,
                    span: self.to(start),
                })
            }

            Tok::Fn => {
                self.bump();
                let sig = self.sig()?;
                let body = self.block()?;
                Ok(Item::Fn {
                    sig,
                    body,
                    span: self.to(start),
                })
            }

            Tok::Test => {
                self.bump();
                let name = match self.peek().clone() {
                    Tok::Str(s) => {
                        self.bump();
                        s
                    }
                    other => {
                        return Err(
                            self.err(&format!("テスト名の文字列が必要です (実際は {:?})", other))
                        );
                    }
                };
                let body = self.block()?;
                Ok(Item::Test {
                    name,
                    body,
                    span: self.to(start),
                })
            }

            other => Err(self.err(&format!(
                "trait / struct / enum / impl / effect / fn / test のいずれかが必要です (実際は {:?})",
                other
            ))),
        }
    }

    fn name_path(&mut self, what: &str) -> PResult<String> {
        let mut parts = vec![self.expect_ident(what)?];
        while self.eat(&Tok::ColonColon) {
            parts.push(self.expect_ident("パスの続き")?);
        }
        Ok(parts.join("::"))
    }

    /// `fn find(id: int -> User?)` — 戻り値の `->` は括弧の内側にある。
    /// `fn now(-> int)` のように引数ゼロで戻り値だけ、も書ける。
    fn sig(&mut self) -> PResult<Sig> {
        let start = self.span();
        let name = self.expect_ident("関数名")?;
        self.expect(&Tok::LParen, "`(`")?;

        let mut params = Vec::new();
        let mut ret = None;

        // `fn save(self, u: User -> unit)` — self は型を書かない。
        // これがある/ないだけがメソッドと関連関数の区別
        let has_self = self.eat(&Tok::SelfKw);
        if has_self {
            self.eat(&Tok::Comma);
        }

        if !self.at(&Tok::RParen) {
            if !self.at(&Tok::Arrow) {
                loop {
                    let pname = self.expect_ident("引数名")?;
                    self.expect(&Tok::Colon, "`:`")?;
                    let ty = self.ty()?;
                    params.push(Param { name: pname, ty });
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
            }
            if self.eat(&Tok::Arrow) {
                ret = Some(self.ty()?);
            }
        }

        self.expect(&Tok::RParen, "`)`")?;
        Ok(Sig {
            name,
            has_self,
            params,
            ret,
            span: self.to(start),
        })
    }

    /// `User` / `[User]` / `[User?]?`。後置 `?` は直前の完成した型に付く
    /// ので、`[T]?` と `[T?]` は別物(design.md 決定2)。
    fn ty(&mut self) -> PResult<Type> {
        let kind = if self.eat(&Tok::LBracket) {
            let element = self.ty()?;
            self.expect(&Tok::RBracket, "`]`")?;
            TypeKind::Array(Box::new(element))
        } else {
            TypeKind::Named(self.name_path("型名")?)
        };
        let optional = self.eat(&Tok::Question);
        Ok(Type { kind, optional })
    }
}

// ---------------------------------------------------------------------------
// ブロックと文位置
// ---------------------------------------------------------------------------

impl<'a> Parser<'a> {
    /// `{ expr* }`。ブロックの値は最後の式(ここでは構文木を作るだけ)。
    fn block(&mut self) -> PResult<Vec<Expr>> {
        self.expect(&Tok::LBrace, "`{`")?;
        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                break;
            }
            if self.at(&Tok::Eof) {
                return Err(self.err("`}` が見つからないままファイルが終わりました"));
            }
            body.push(self.stmt()?);

            // 文の区切りは改行か `}`。`;` は言語に存在しない
            if !self.at(&Tok::Newline) && !self.at(&Tok::RBrace) {
                return Err(self.err(&format!(
                    "1行に2つの式は書けません (次は {:?})",
                    self.peek()
                )));
            }
        }
        Ok(body)
    }

    /// 文位置。Head で始まるかもしれない。
    fn stmt(&mut self) -> PResult<Expr> {
        let start = self.span();

        // `if` は後続の `elif`/`else` を吸い込んで1つの式になる
        if self.at(&Tok::If) {
            self.bump();
            let cond = self.cond()?;
            return self.if_tail(Head::If(Box::new(cond)), start);
        }
        if self.at(&Tok::Elif) || self.at(&Tok::Else) {
            return Err(self.err("対応する `if` がありません"));
        }

        // `with db(value), clock(value) { ... }` — ambient の提供。
        if self.at(&Tok::With) {
            self.bump();
            let mut binders = vec![self.provision()?];
            while self.eat(&Tok::Comma) {
                binders.push(self.provision()?);
            }
            let body = self.head_body()?;
            return Ok(Expr {
                kind: ExprKind::Head {
                    head: Head::Ambient(binders),
                    body: Box::new(body),
                    orelse: None,
                },
                span: self.to(start),
            });
        }

        // キーワードで始まる Head
        let head = match self.peek() {
            Tok::While => {
                self.bump();
                Some(Head::While(Box::new(self.cond()?)))
            }
            Tok::For => {
                self.bump();
                let var = self.expect_ident("ループ変数")?;
                self.expect(&Tok::In, "`in`")?;
                Some(Head::For {
                    var,
                    iter: Box::new(self.cond()?),
                })
            }
            _ => None,
        };
        if let Some(head) = head {
            let body = self.head_body()?;
            return Ok(Expr {
                kind: ExprKind::Head {
                    head,
                    body: Box::new(body),
                    orelse: None,
                },
                span: self.to(start),
            });
        }

        self.expr()
    }

    /// `db<Type>` または `db(value)`。`with` が先にあるため普通の式とは衝突しない。
    fn provision(&mut self) -> PResult<Provision> {
        let slot = self.name_path("スロット名")?;

        if self.eat(&Tok::Less) {
            let type_name = self.name_path("実装型")?;
            self.expect(&Tok::Greater, "`>`")?;
            return Ok(Provision::Type { slot, type_name });
        }

        self.expect(&Tok::LParen, "`<型>` または `(値)`")?;
        let saved = std::mem::replace(&mut self.no_struct, false);
        let value = self.expr();
        self.no_struct = saved;
        let value = value?;
        self.expect(&Tok::RParen, "`)`")?;
        Ok(Provision::Value { slot, value })
    }

    /// `if`/`elif` の本体と、後続の `elif`/`else` を読む。
    ///
    /// 直前の改行は行継続規則が既に落としている(`elif`/`else` は式を始められない)。
    /// なので本体を読み終えた地点で、次のトークンがそのまま `elif`/`else` になる。
    fn if_tail(&mut self, head: Head, start: Span) -> PResult<Expr> {
        let body = self.head_body()?;

        let orelse = if self.at(&Tok::Elif) {
            let elif_start = self.span();
            self.bump();
            let cond = self.cond()?;
            Some(Box::new(
                self.if_tail(Head::Elif(Box::new(cond)), elif_start)?,
            ))
        } else if self.at(&Tok::Else) {
            let else_start = self.span();
            self.bump();
            let else_body = self.head_body()?;
            Some(Box::new(Expr {
                kind: ExprKind::Head {
                    head: Head::Else,
                    body: Box::new(else_body),
                    orelse: None,
                },
                span: self.to(else_start),
            }))
        } else {
            None
        };

        Ok(Expr {
            kind: ExprKind::Head {
                head,
                body: Box::new(body),
                orelse,
            },
            span: self.to(start),
        })
    }

    /// Head の条件式。ここでは struct リテラルを認めない
    /// (`while c { }` の `c { }` を struct 生成と読まないため)。
    fn cond(&mut self) -> PResult<Expr> {
        let saved = self.no_struct;
        self.no_struct = true;
        let e = self.expr();
        self.no_struct = saved;
        e
    }

    /// `'{' ... '}'`、または `':' 単純式`。
    ///
    /// 非ブレースの本体は Head と同一物理行に限る。これが dangling else と
    /// goto-fail を構文レベルで殺している規則なので、ここで検査する。
    fn head_body(&mut self) -> PResult<Expr> {
        if self.at(&Tok::LBrace) {
            let start = self.span();
            let body = self.block()?;
            return Ok(Expr {
                kind: ExprKind::Block(body),
                span: self.to(start),
            });
        }

        let colon = self.expect(&Tok::Colon, "`:` または `{`")?;

        if self.at(&Tok::LBrace) {
            return Err(self.err("ブロックの前に `:` は要りません"));
        }

        if self.line() != colon.line {
            return Err(self.err(
                "`:` の後ろに `{` なしで改行することはできません。同じ行に書くか `{}` で囲んでください",
            ));
        }
        self.stmt()
    }

    /// `match` の arm 列。区切りはブロックと同じ改行で、カンマは無い。
    /// 本体は既存 Head と同じ `: 単純式` か `{ ... }`(design.md 決定1)。
    fn match_arms(&mut self) -> PResult<Vec<MatchArm>> {
        self.expect(&Tok::LBrace, "`{`")?;
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                break;
            }
            if self.at(&Tok::Eof) {
                return Err(self.err("`}` が見つからないままファイルが終わりました"));
            }

            let start = self.span();
            let pattern = self.match_pattern()?;
            let body = self.head_body()?;
            arms.push(MatchArm {
                pattern,
                body,
                span: self.to(start),
            });

            if !self.at(&Tok::Newline) && !self.at(&Tok::RBrace) {
                return Err(self.err(&format!(
                    "1行に2つの arm は書けません (次は {:?})",
                    self.peek()
                )));
            }
        }
        Ok(arms)
    }

    /// arm 全体の pattern。限定 `Enum::Variant` に payload 要素が続く形だけ。
    fn match_pattern(&mut self) -> PResult<MatchPattern> {
        let path = self.name_path("arm の `Enum::Variant`")?;
        let Some((enum_name, variant)) = path.rsplit_once("::") else {
            return Err(self.err(&format!(
                "arm には `Enum::Variant` の形が必要です (実際は `{path}`)"
            )));
        };
        // `Lookup::Found(user, _)` — 宣言 payload と同じ個数の平坦な要素。
        // 個数と型の照合は宣言表を持つ型検査の側にある(design.md 決定3)
        let mut bindings = Vec::new();
        if self.eat(&Tok::LParen) {
            while !self.at(&Tok::RParen) {
                let name = self.expect_ident("payload の束縛名または `_`")?;
                bindings.push(if name == "_" {
                    PatternBinding::Discard
                } else {
                    PatternBinding::Bind(name)
                });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RParen, "`)` または payload の束縛名")?;
        }
        Ok(MatchPattern::Variant {
            enum_name: enum_name.to_string(),
            variant: variant.to_string(),
            bindings,
        })
    }
}

// ---------------------------------------------------------------------------
// 式。優先順位は関数の入れ子で表す
// ---------------------------------------------------------------------------

impl<'a> Parser<'a> {
    fn expr(&mut self) -> PResult<Expr> {
        self.assign()
    }

    fn assign(&mut self) -> PResult<Expr> {
        let start = self.span();
        let lhs = self.coalesce()?;
        if self.eat(&Tok::Eq) {
            if matches!(&lhs.kind, ExprKind::OptionalField(_, _)) {
                return Err(self.err("optional field access `.?` は読み取り専用です"));
            }
            let value = self.assign()?; // 右結合
            return Ok(Expr {
                kind: ExprKind::Assign {
                    target: Box::new(lhs),
                    value: Box::new(value),
                },
                span: self.to(start),
            });
        }
        Ok(lhs)
    }

    fn coalesce(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut lhs = self.equality()?;
        while self.eat(&Tok::Coalesce) {
            let rhs = self.equality()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op: BinOp::Coalesce,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: self.to(start),
            };
        }
        Ok(lhs)
    }

    fn equality(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut lhs = self.additive()?;
        while self.eat(&Tok::EqEq) {
            let rhs = self.additive()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op: BinOp::Eq,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: self.to(start),
            };
        }
        Ok(lhs)
    }

    fn additive(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut lhs = self.multiplicative()?;
        loop {
            let op = if self.eat(&Tok::Plus) {
                BinOp::Add
            } else if self.eat(&Tok::Minus) {
                BinOp::Sub
            } else {
                break;
            };
            let rhs = self.multiplicative()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: self.to(start),
            };
        }
        Ok(lhs)
    }

    fn multiplicative(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut lhs = self.unary()?;
        loop {
            let op = if self.eat(&Tok::Star) {
                BinOp::Mul
            } else if self.eat(&Tok::Slash) {
                BinOp::Div
            } else {
                break;
            };
            let rhs = self.unary()?;
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span: self.to(start),
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let start = self.span();
        if self.eat(&Tok::Minus) {
            let e = self.unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Neg, Box::new(e)),
                span: self.to(start),
            });
        }
        self.postfix()
    }

    /// 後置。`.field` / `.?field` / `(args)` / `::name` を左から積む
    fn postfix(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut e = self.primary()?;
        loop {
            if self.eat(&Tok::Dot) {
                let optional = self.eat(&Tok::Question);
                let name = self.expect_ident("フィールド名かメソッド名")?;
                e = Expr {
                    kind: if optional {
                        ExprKind::OptionalField(Box::new(e), name)
                    } else {
                        ExprKind::Field(Box::new(e), name)
                    },
                    span: self.to(start),
                };
            } else if self.at(&Tok::LParen) {
                if matches!(&e.kind, ExprKind::OptionalField(_, _)) {
                    return Err(self.err("optional method call `.?method(...)` は未対応です"));
                }
                let args = self.args()?;
                e = Expr {
                    kind: ExprKind::Call(Box::new(e), args),
                    span: self.to(start),
                };
            } else if self.eat(&Tok::ColonColon) {
                let name = self.expect_ident("パスの続き")?;
                // `A::b::c` は1つの Path に畳む
                let mut segs = match e.kind {
                    ExprKind::Ident(s) => vec![s],
                    ExprKind::Path(v) => v,
                    _ => return Err(self.err("`::` の左には名前が必要です")),
                };
                segs.push(name);
                e = Expr {
                    kind: ExprKind::Path(segs),
                    span: self.to(start),
                };
            } else if self.at(&Tok::LBrace) && !self.no_struct {
                let name = match e.kind {
                    ExprKind::Ident(name) => name,
                    ExprKind::Path(parts) => parts.join("::"),
                    _ => break,
                };
                let fields = self.struct_fields()?;
                e = Expr {
                    kind: ExprKind::StructLit { name, fields },
                    span: self.to(start),
                };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn args(&mut self) -> PResult<Vec<Expr>> {
        self.expect(&Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        if !self.at(&Tok::RParen) {
            loop {
                // 括弧の中では struct リテラルを禁じる理由がない
                let saved = std::mem::replace(&mut self.no_struct, false);
                let a = self.expr();
                self.no_struct = saved;
                args.push(a?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        self.expect(&Tok::RParen, "`)`")?;
        Ok(args)
    }

    fn struct_fields(&mut self) -> PResult<Vec<(String, Expr)>> {
        self.expect(&Tok::LBrace, "`{`")?;
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                break;
            }
            let fname = self.expect_ident("フィールド名")?;
            self.expect(&Tok::Eq, "`=`")?;
            let value = self.expr()?;
            fields.push((fname, value));
            self.skip_newlines();
            if !self.eat(&Tok::Comma) {
                self.skip_newlines();
                self.expect(&Tok::RBrace, "`}`")?;
                break;
            }
        }
        Ok(fields)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let start = self.span();

        let kind = match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                ExprKind::Int(n)
            }
            Tok::Str(s) => {
                self.bump();
                ExprKind::Str(s)
            }
            Tok::True => {
                self.bump();
                ExprKind::Bool(true)
            }
            Tok::False => {
                self.bump();
                ExprKind::Bool(false)
            }
            Tok::Nil => {
                self.bump();
                ExprKind::Nil
            }

            // `self` は式としてはただの名前。新しい産出も値も足さない。
            // 束縛されるのはメソッドを呼んだときだけ(eval)
            Tok::SelfKw => {
                self.bump();
                ExprKind::Ident("self".to_string())
            }

            Tok::Ident(name) => {
                self.bump();
                ExprKind::Ident(name)
            }
            Tok::Use => {
                self.bump();
                ExprKind::Ident("use".to_string())
            }
            Tok::As => {
                self.bump();
                ExprKind::Ident("as".to_string())
            }

            Tok::Let => {
                self.bump();
                let name = self.expect_ident("変数名")?;
                self.expect(&Tok::Eq, "`=`")?;
                // 値は `stmt` で読む。値ベースなので Head も値を産む
                // (`let r = with db(replica) { collect() }` — CONTEXT.md「第二級ブロック」)。
                // `expr` で読むと `:` が let の外に残り、`let` 全体が
                // ambient の binder として読まれてしまう
                let value = self.stmt()?;
                ExprKind::Let {
                    name,
                    value: Box::new(value),
                }
            }

            Tok::Return => {
                self.bump();
                // 値なしの `return` は行末・`}`・`)` の手前でしか現れない
                let has_value = !matches!(
                    self.peek(),
                    Tok::Newline | Tok::RBrace | Tok::RParen | Tok::Eof
                );
                let v = if has_value {
                    Some(Box::new(self.expr()?))
                } else {
                    None
                };
                ExprKind::Return(v)
            }

            Tok::Assert => {
                self.bump();
                ExprKind::Assert(Box::new(self.expr()?))
            }

            Tok::LParen => {
                self.bump();
                let saved = std::mem::replace(&mut self.no_struct, false);
                let e = self.expr();
                self.no_struct = saved;
                let e = e?;
                self.expect(&Tok::RParen, "`)`")?;
                return Ok(e);
            }

            Tok::LBracket => {
                self.bump();
                let mut items = Vec::new();
                if !self.at(&Tok::RBracket) {
                    loop {
                        self.skip_newlines();
                        items.push(self.expr()?);
                        self.skip_newlines();
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&Tok::RBracket, "`]`")?;
                ExprKind::Array(items)
            }

            // 裸のブロック。第二級だが値は産む
            Tok::LBrace => {
                let body = self.block()?;
                ExprKind::Block(body)
            }

            // `match rank { Rank::Gold: "gold" }` — 選ばれた arm の値を産む式。
            // 文位置専用にしないのは、値ベースで引数にも渡せる必要があるため
            Tok::Match => {
                self.bump();
                let subject = self.cond()?;
                let arms = self.match_arms()?;
                ExprKind::Match {
                    subject: Box::new(subject),
                    arms,
                }
            }

            other => {
                return Err(self.err(&format!("式が必要です (実際は {:?})", other)));
            }
        };

        Ok(Expr {
            kind,
            span: self.to(start),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::{join, lex};

    fn parse_src(src: &str) -> PResult<Program> {
        parse(&join(lex(src).unwrap()))
    }

    fn ok(src: &str) -> Program {
        match parse_src(src) {
            Ok(p) => p,
            Err(e) => panic!("パースに失敗: {e}\n--- ソース ---\n{src}"),
        }
    }

    /// v1 の的そのもの。これが通らなくなったら何かを壊している。
    #[test]
    fn 正典プログラムが通る() {
        let src = std::fs::read_to_string("examples/canonical.rd").unwrap();
        let p = ok(&src);

        // 個数ではなく、宣言の種類が全部読めていることを見る
        let mut kinds = std::collections::BTreeSet::new();
        for i in &p.items {
            kinds.insert(match i {
                Item::Trait { .. } => "trait",
                Item::Struct { .. } => "struct",
                Item::Enum { .. } => "enum",
                Item::Impl { .. } => "impl",
                Item::Effect { .. } => "effect",
                Item::Fn { .. } => "fn",
                Item::Test { .. } => "test",
            });
        }
        assert_eq!(
            kinds,
            ["effect", "enum", "fn", "impl", "struct", "test", "trait"]
                .into_iter()
                .collect()
        );
    }

    /// 宣言 variant を (名前, payload 型の綴り) で取り出す
    fn variants(src: &str) -> Vec<(String, Vec<String>)> {
        let p = ok(src);
        let Item::Enum { variants, .. } = &p.items[0] else {
            panic!("enum ではない: {:?}", p.items[0])
        };
        variants
            .iter()
            .map(|v| {
                let payload = v.payload.iter().map(show_type).collect();
                (v.name.clone(), payload)
            })
            .collect()
    }

    fn show_type(ty: &Type) -> String {
        let base = match &ty.kind {
            TypeKind::Named(name) => name.clone(),
            TypeKind::Array(element) => format!("[{}]", show_type(element)),
        };
        if ty.optional {
            format!("{base}?")
        } else {
            base
        }
    }

    #[test]
    fn enum宣言はpayloadを持たないvariantを並べる() {
        // 改行区切りでも1行でも同じ形に読める
        for src in [
            "enum Rank {\n  Bronze\n  Gold\n}\n",
            "enum Rank { Bronze Gold }\n",
            "enum Rank { Bronze, Gold }\n",
        ] {
            let p = ok(src);
            let Item::Enum { name, .. } = &p.items[0] else {
                panic!("enum ではない: {:?}", p.items[0])
            };
            assert_eq!(name, "Rank");
            assert_eq!(
                variants(src),
                vec![
                    ("Bronze".to_string(), Vec::<String>::new()),
                    ("Gold".to_string(), Vec::new()),
                ]
            );
        }
    }

    #[test]
    fn variantはpayload型を宣言順に並べる() {
        assert_eq!(
            variants("enum Lookup {\n  Found(User)\n  Missing(str, int)\n  Skipped\n}\n"),
            vec![
                ("Found".to_string(), vec!["User".to_string()]),
                (
                    "Missing".to_string(),
                    vec!["str".to_string(), "int".to_string()]
                ),
                ("Skipped".to_string(), Vec::new()),
            ]
        );
    }

    #[test]
    fn payloadは型注釈の形をそのまま取る() {
        assert_eq!(
            variants("enum Box { One([User?]?) }\n"),
            vec![("One".to_string(), vec!["[User?]?".to_string()])]
        );
    }

    #[test]
    fn 空のpayload括弧はfieldlessと同じ形になる() {
        assert_eq!(
            variants("enum Lookup { Skipped() }\n"),
            vec![("Skipped".to_string(), Vec::<String>::new())]
        );
    }

    #[test]
    fn payloadの閉じ括弧が無いと落ちる() {
        let e = parse_src("enum Lookup { Found(User }\n").unwrap_err();
        assert!(e.msg.contains("`)`"), "{}", e.msg);
    }

    #[test]
    fn payloadに型でないものは書けない() {
        let e = parse_src("enum Lookup { Found(1) }\n").unwrap_err();
        assert!(e.msg.contains("型名"), "{}", e.msg);
    }

    #[test]
    fn 空のenumも書ける() {
        let p = ok("enum Never {}\n");
        assert!(matches!(&p.items[0], Item::Enum { variants, .. } if variants.is_empty()));
    }

    #[test]
    fn enumのvariantは値を持てない() {
        let e = parse_src("enum Rank { Bronze = 1 }\n").unwrap_err();
        assert!(e.msg.contains("variant 名"), "{}", e.msg);
    }

    // ---- match ----

    /// arm の pattern を書かれたままの綴りで取り出す。`_` は `"_"`
    fn arms(src: &str) -> Vec<String> {
        let p = ok(src);
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!()
        };
        let ExprKind::Let { value, .. } = &body[0].kind else {
            panic!("let ではない: {:?}", body[0].kind)
        };
        let ExprKind::Match { arms, .. } = &value.kind else {
            panic!("match ではない: {:?}", value.kind)
        };
        arms.iter().map(|a| a.pattern.label()).collect()
    }

    #[test]
    fn matchは限定variantのarmを改行で並べる() {
        // 一行形とブロック形が混ざっても、arm の並びは同じ
        let expected = vec!["Rank::Bronze".to_string(), "Rank::Gold".to_string()];
        assert_eq!(
            arms(
                "fn f(r: Rank) {\n\
                  \x20 let label = match r {\n\
                  \x20   Rank::Bronze: \"bronze\"\n\
                  \x20   Rank::Gold {\n\
                  \x20     audit()\n\
                  \x20     \"gold\"\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n"
            ),
            expected
        );
        // 修飾された enum パスも1つの名前として保つ
        assert_eq!(
            arms("fn f(r: Rank) {\n let x = match r { dep::Rank::Gold: 1 }\n}\n"),
            vec!["dep::Rank::Gold".to_string()]
        );
    }

    #[test]
    fn 空のmatchも書ける() {
        assert!(arms("fn f(n: Never) {\n let x = match n { }\n}\n").is_empty());
    }

    #[test]
    fn matchの対象の直後のbraceはarm列になる() {
        // `match c { }` の `{ }` を `c` の struct リテラルにはしない
        ok("fn f(r: Rank) {\n match r { Rank::Gold: 1 }\n}\n");
    }

    /// arm を (`Enum::Variant`, pattern 要素の綴り) で取り出す。`_` は `"_"`
    fn arm_patterns(src: &str) -> Vec<(String, Vec<String>)> {
        let p = ok(src);
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!()
        };
        let ExprKind::Let { value, .. } = &body[0].kind else {
            panic!("let ではない: {:?}", body[0].kind)
        };
        let ExprKind::Match { arms, .. } = &value.kind else {
            panic!("match ではない: {:?}", value.kind)
        };
        arms.iter()
            .map(|a| match &a.pattern {
                MatchPattern::Variant {
                    enum_name,
                    variant,
                    bindings,
                } => {
                    let bindings = bindings
                        .iter()
                        .map(|b| match b {
                            PatternBinding::Bind(name) => name.clone(),
                            PatternBinding::Discard => "_".to_string(),
                        })
                        .collect();
                    (format!("{enum_name}::{variant}"), bindings)
                }
                MatchPattern::CatchAll => ("_".to_string(), Vec::new()),
            })
            .collect()
    }

    #[test]
    fn armのpatternは識別子とアンダースコアを並べる() {
        assert_eq!(
            arm_patterns(
                "fn f(l: Lookup) {\n\
                  \x20 let x = match l {\n\
                  \x20   Lookup::Found(user): 1\n\
                  \x20   Lookup::Missing(reason, _): 2\n\
                  \x20   Lookup::Skipped: 3\n\
                  \x20 }\n\
                  }\n"
            ),
            vec![
                ("Lookup::Found".to_string(), vec!["user".to_string()]),
                (
                    "Lookup::Missing".to_string(),
                    vec!["reason".to_string(), "_".to_string()]
                ),
                ("Lookup::Skipped".to_string(), Vec::new()),
            ]
        );
    }

    #[test]
    fn armのpatternの閉じ括弧が無いと落ちる() {
        let e =
            parse_src("fn f(l: Lookup) {\n match l { Lookup::Found(user: 1 }\n}\n").unwrap_err();
        assert!(e.msg.contains("`)`"), "{}", e.msg);
    }

    #[test]
    fn 裸のarmパスを受けない() {
        let e = parse_src("fn f(r: Rank) {\n match r { Gold: 1 }\n}\n").unwrap_err();
        assert!(e.msg.contains("`Enum::Variant`"), "{}", e.msg);
    }

    #[test]
    fn armの本体にもコロンと改行の規則が効く() {
        let e = parse_src("fn f(r: Rank) {\n match r {\n Rank::Gold:\n 1\n }\n}\n").unwrap_err();
        assert!(e.msg.contains("改行"), "{}", e.msg);

        let e = parse_src("fn f(r: Rank) {\n match r { Rank::Gold: 1 Rank::Bronze: 2 }\n}\n")
            .unwrap_err();
        assert!(e.msg.contains("1行に2つ"), "{}", e.msg);
    }

    #[test]
    fn 契約とスロット宣言() {
        let p = ok("trait Database {\n  fn find(id: int -> User?)\n}\neffect db: Database\n");
        assert!(matches!(&p.items[0], Item::Trait { methods, .. } if methods.len() == 1));
        assert!(matches!(&p.items[1], Item::Effect { slot, trait_name, .. }
                     if slot == "db" && trait_name == "Database"));
    }

    /// 値ベースなので Head も値を産む。`let` の右辺に来られること
    /// (CONTEXT.md「第二級ブロック」の `let r = with db(replica) { collect() }`)
    #[test]
    fn letの右辺にheadが来られる() {
        let p = ok("fn main() {\n  let r = with db(replica) { 1 }\n}\n");
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!()
        };
        let ExprKind::Let { value, .. } = &body[0].kind else {
            panic!("let ではない: {:?}", body[0].kind)
        };
        assert!(
            matches!(&value.kind, ExprKind::Head { head: Head::Ambient(bs), .. } if bs.len() == 1),
            "右辺が Head になっていない: {:?}",
            value.kind
        );
    }

    #[test]
    fn optional_field_accessと連鎖を読む() {
        for src in [
            "fn f(u: User?) { u.?profile.?name }\n",
            "fn f(u: User?) {\n u\n .?profile\n .?name\n}\n",
        ] {
            let p = ok(src);
            let Item::Fn { body, .. } = &p.items[0] else {
                panic!()
            };
            let ExprKind::OptionalField(profile, name) = &body[0].kind else {
                panic!("外側が optional field ではない: {:?}", body[0].kind)
            };
            assert_eq!(name, "name");
            assert!(
                matches!(&profile.kind, ExprKind::OptionalField(_, field) if field == "profile")
            );
        }
    }

    #[test]
    fn optional_field_accessは代入とメソッド呼び出しに使えない() {
        let assignment = parse_src("fn f(u: User?) { u.?name = \"x\" }\n").unwrap_err();
        assert!(assignment.msg.contains("読み取り専用"), "{assignment}");

        let call = parse_src("fn f(u: User?) { u.?save() }\n").unwrap_err();
        assert!(call.msg.contains("optional method"), "{call}");
    }

    /// `self` の有無だけがメソッドと関連関数の区別
    #[test]
    fn selfは第一引数として書く() {
        let p = ok("impl Database for Postgres {\n\
                    \x20 fn save(self, u: User -> unit) {\n\
                    \x20   1\n\
                    \x20 }\n\
                    \x20 fn new(url: str -> Postgres) {\n\
                    \x20   2\n\
                    \x20 }\n\
                    }\n");
        let Item::Impl {
            trait_name,
            type_name,
            methods,
            ..
        } = &p.items[0]
        else {
            panic!()
        };
        assert_eq!(trait_name.as_deref(), Some("Database"));
        assert_eq!(type_name, "Postgres");
        // self は params には入らない。has_self に出る
        assert!(methods[0].0.has_self);
        assert_eq!(methods[0].0.params.len(), 1);
        assert!(!methods[1].0.has_self);
        assert_eq!(methods[1].0.params.len(), 1);
    }

    #[test]
    fn 戻り値の矢印は括弧の内側() {
        let p = ok("fn now(-> int) {\n  1\n}\n");
        let Item::Fn { sig, .. } = &p.items[0] else {
            panic!()
        };
        assert!(sig.params.is_empty());
        assert_eq!(sig.ret.as_ref().unwrap().name(), Some("int"));
    }

    /// `[T]` を全ての型注釈位置で受け、後置 `?` の付き先を言い分ける
    #[test]
    fn 配列型を全ての型位置で読む() {
        fn named(name: &str, optional: bool) -> Type {
            Type {
                kind: TypeKind::Named(name.to_string()),
                optional,
            }
        }
        fn array(element: Type, optional: bool) -> Type {
            Type {
                kind: TypeKind::Array(Box::new(element)),
                optional,
            }
        }

        let p = ok("struct Store { users: [User]\ntags: [[str]?] }\n\
                    fn pick(xs: [User], ys: [User?]? -> [User]?) {\n 1\n}\n");

        let Item::Struct { fields, .. } = &p.items[0] else {
            panic!()
        };
        assert_eq!(fields[0].1, array(named("User", false), false));
        assert_eq!(
            fields[1].1,
            array(array(named("str", false), true), false),
            "入れ子の要素にも後置 `?` が付く"
        );

        let Item::Fn { sig, .. } = &p.items[1] else {
            panic!()
        };
        assert_eq!(sig.params[0].ty, array(named("User", false), false));
        assert_eq!(
            sig.params[1].ty,
            array(named("User", true), true),
            "`[T?]?` は optional な要素の optional な配列"
        );
        assert_eq!(sig.ret, Some(array(named("User", false), true)));
    }

    #[test]
    fn 閉じない配列型を報告する() {
        assert!(parse(&join(lex("fn f(xs: [User) { 1 }\n").unwrap())).is_err());
    }

    #[test]
    fn ブロック形headにコロンはいらない() {
        ok("fn f() {\n\
            \x20 if ready { one() }\n\
            \x20 for x in xs { use(x) }\n\
            \x20 while running { tick() }\n\
            }\n");
    }

    #[test]
    fn withはambient提供になる() {
        let p = ok("fn main() {\n\
                    \x20 with db(pg), clock(sys) {\n\
                    \x20   handle(id)\n\
                    \x20 }\n\
                    }\n");
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!()
        };
        let ExprKind::Head {
            head: Head::Ambient(binders),
            ..
        } = &body[0].kind
        else {
            panic!("with が ambient head になっていない: {:?}", body[0].kind)
        };
        assert_eq!(binders.len(), 2);
    }

    #[test]
    fn withは型だけを提供できる() {
        let p = ok("fn main() {\n  with db<Postgres> { db::new() }\n}\n");
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!()
        };
        let ExprKind::Head {
            head: Head::Ambient(binders),
            ..
        } = &body[0].kind
        else {
            panic!("with が ambient head になっていない: {:?}", body[0].kind)
        };
        assert!(matches!(
            binders.as_slice(),
            [Provision::Type { slot, type_name }]
                if slot == "db" && type_name == "Postgres"
        ));
    }

    #[test]
    fn head条件のstruct生成は括弧で囲める() {
        ok("fn f() {\n  if user == (User { id = 1 }) { yes() }\n}\n");
    }

    #[test]
    fn 一行フォームと結合() {
        ok("fn f() {\n  if a: x()\n  elif b: y()\n  else: z()\n}\n");
    }

    #[test]
    fn コロンの後ろで改行はできない() {
        // 行継続で改行が消えても、行番号で捕まえる
        let e = parse_src("fn f() {\n  if a:\n    x()\n}\n").unwrap_err();
        assert!(e.msg.contains("改行"), "{}", e.msg);
    }

    #[test]
    fn ブロックの前にコロンは書けない() {
        let e = parse_src("fn f() {\n  if ready: { go() }\n}\n").unwrap_err();
        assert!(e.msg.contains("要りません"), "{}", e.msg);
    }

    #[test]
    fn 旧ambient提供構文は受けない() {
        let e = parse_src("fn f() {\n  db(store): { go() }\n}\n").unwrap_err();
        assert!(e.msg.contains("1行に2つ"), "{}", e.msg);
    }

    #[test]
    fn 一行に二つの式は書けない() {
        let e = parse_src("fn f() {\n  a() b()\n}\n").unwrap_err();
        assert!(e.msg.contains("1行に2つ"), "{}", e.msg);
    }

    #[test]
    fn struct生成は等号() {
        ok("fn f() {\n  Circle { r = 1, g = 2 }\n}\n");
    }

    #[test]
    fn 条件直後のbraceはhead本体になる() {
        // `while c { }` の `{ }` を `c` の struct リテラルにはしない
        ok("fn f() {\n  while c { x() }\n}\n");
    }

    #[test]
    fn useは先頭でモジュールと選択メンバーを導入する() {
        let p = ok("use data::database as db_module\n\
             use services::{\n\
             \x20 users,\n\
             \x20 billing as payments,\n\
             }\n\
             fn main() { db_module::connect() }\n");
        assert_eq!(p.uses.len(), 2);
        assert_eq!(p.uses[0].path, ["data", "database"]);
        assert_eq!(p.uses[0].alias.as_deref(), Some("db_module"));
        let members = p.uses[1].members.as_ref().unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[1].alias.as_deref(), Some("payments"));
    }

    #[test]
    fn useの選択リストは空にできない() {
        let error = parse_src("use services::{}\nfn main() { 0 }\n").unwrap_err();
        assert!(error.msg.contains("空にできません"), "{}", error.msg);
    }

    #[test]
    fn useはトップレベル接頭部にだけ書ける() {
        let error = parse_src("fn first() { 1 }\nuse services\n").unwrap_err();
        assert!(error.msg.contains("先頭"), "{}", error.msg);
    }

    #[test]
    fn asはuse以外では既存どおり識別子として使える() {
        ok("fn as() { 1 }\nfn main() { as() }\n");
    }

    #[test]
    fn useに相対モジュールパスは書けない() {
        let error = parse_src("use super::services\nfn main() { 0 }\n").unwrap_err();
        assert!(error.msg.contains("絶対"), "{}", error.msg);
    }
}
