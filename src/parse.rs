//! 再帰下降パーサ。
//!
//! 優先順位は関数の入れ子で表す。低い段が高い段を呼んでから自分の演算子を探す:
//!
//!   assign → coalesce → equality → additive → multiplicative → unary → postfix → primary
//!
//! `Head` に専用の構文カテゴリはない。CONTEXT.md の定義どおり
//! 「`:` の左にあるもの」なので、式を読んでから次が `:` かどうかで判明する。
//! キーワードで始まる `if`/`for`/`while` だけ先に分岐する。

use crate::ast::*;
use crate::lex::{Span, Tok, Token};

#[derive(Debug)]
pub struct ParseError {
    pub msg: String,
    pub line: u32,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}行目: {}", self.line, self.msg)
    }
}

type PResult<T> = Result<T, ParseError>;

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
            other => Err(self.err(&format!("{} が必要です (実際は {:?})", what, other))),
        }
    }

    fn err(&self, msg: &str) -> ParseError {
        ParseError {
            msg: msg.to_string(),
            line: self.line(),
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(&Tok::Newline) {
            self.bump();
        }
    }

    fn to(&self, start: Span) -> Span {
        let end = self.toks[self.pos.saturating_sub(1)].span.end;
        Span {
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
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&Tok::Eof) {
                break;
            }
            items.push(self.item()?);
        }
        Ok(Program { items })
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

            // `effect db: Database` — 関数を1つも宣言しない。契約と役割を分けて書く
            Tok::Effect => {
                self.bump();
                let slot = self.expect_ident("スロット名")?;
                self.expect(&Tok::Colon, "`:`")?;
                let trait_name = self.expect_ident("trait 名")?;
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
                        return Err(self.err(&format!("テスト名の文字列が必要です (実際は {:?})", other)));
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
                "trait / effect / fn / test のいずれかが必要です (実際は {:?})",
                other
            ))),
        }
    }

    /// `fn find(id: UserId -> User?)` — 戻り値の `->` は括弧の内側にある。
    /// `fn now(-> Time)` のように引数ゼロで戻り値だけ、も書ける。
    fn sig(&mut self) -> PResult<Sig> {
        let start = self.span();
        let name = self.expect_ident("関数名")?;
        self.expect(&Tok::LParen, "`(`")?;

        let mut params = Vec::new();
        let mut ret = None;

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
            params,
            ret,
            span: self.to(start),
        })
    }

    fn ty(&mut self) -> PResult<Type> {
        let name = self.expect_ident("型名")?;
        let optional = self.eat(&Tok::Question);
        Ok(Type { name, optional })
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

        // ここから先は普通の式。ただし `:` が続けば ambient の Head だったと判明する
        let first = self.expr()?;
        if self.at(&Tok::Comma) || self.at(&Tok::Colon) {
            let mut binders = vec![first];
            while self.eat(&Tok::Comma) {
                binders.push(self.expr()?);
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
        Ok(first)
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
            Some(Box::new(self.if_tail(Head::Elif(Box::new(cond)), elif_start)?))
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

    /// `':' 単純式` または `':' '{' ... '}'`。
    ///
    /// 非ブレースの本体は Head と同一物理行に限る。これが dangling else と
    /// goto-fail を構文レベルで殺している規則なので、ここで検査する。
    fn head_body(&mut self) -> PResult<Expr> {
        let colon = self.expect(&Tok::Colon, "`:`")?;

        if self.at(&Tok::LBrace) {
            let start = self.span();
            let body = self.block()?;
            return Ok(Expr {
                kind: ExprKind::Block(body),
                span: self.to(start),
            });
        }

        if self.line() != colon.line {
            return Err(self.err(
                "`:` の後ろに `{` なしで改行することはできません。同じ行に書くか `{}` で囲んでください",
            ));
        }
        self.stmt()
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

    /// 後置。`.field` / `(args)` / `::name` を左から積む
    fn postfix(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut e = self.primary()?;
        loop {
            if self.eat(&Tok::Dot) {
                let name = self.expect_ident("フィールド名かメソッド名")?;
                e = Expr {
                    kind: ExprKind::Field(Box::new(e), name),
                    span: self.to(start),
                };
            } else if self.at(&Tok::LParen) {
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

            Tok::Ident(name) => {
                self.bump();
                // `Circle { r = 1.0 }` — struct 生成。`=` は「束縛」で let と一貫。
                // `:` を Head 専用に保つための選択(ADR / docs/grammar.md)。
                if self.at(&Tok::LBrace) && !self.no_struct {
                    self.bump();
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
                    ExprKind::StructLit { name, fields }
                } else {
                    ExprKind::Ident(name)
                }
            }

            Tok::Let => {
                self.bump();
                let name = self.expect_ident("変数名")?;
                self.expect(&Tok::Eq, "`=`")?;
                let value = self.expr()?;
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
        assert_eq!(p.items.len(), 9);
    }

    #[test]
    fn 契約とスロット宣言() {
        let p = ok("trait Database {\n  fn find(id: UserId -> User?)\n}\neffect db: Database\n");
        assert!(matches!(&p.items[0], Item::Trait { methods, .. } if methods.len() == 1));
        assert!(
            matches!(&p.items[1], Item::Effect { slot, trait_name, .. }
                     if slot == "db" && trait_name == "Database")
        );
    }

    #[test]
    fn 戻り値の矢印は括弧の内側() {
        let p = ok("fn now(-> Time) {\n  1\n}\n");
        let Item::Fn { sig, .. } = &p.items[0] else {
            panic!()
        };
        assert!(sig.params.is_empty());
        assert_eq!(sig.ret.as_ref().unwrap().name, "Time");
    }

    #[test]
    fn ambient_の提供は_head_産出になる() {
        let p = ok("fn main() {\n  db(pg), clock(sys): {\n    handle(id)\n  }\n}\n");
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!()
        };
        let ExprKind::Head {
            head: Head::Ambient(binders),
            ..
        } = &body[0].kind
        else {
            panic!("ambient head として読めていない: {:?}", body[0].kind)
        };
        assert_eq!(binders.len(), 2);
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
    fn 一行に二つの式は書けない() {
        let e = parse_src("fn f() {\n  a() b()\n}\n").unwrap_err();
        assert!(e.msg.contains("1行に2つ"), "{}", e.msg);
    }

    #[test]
    fn struct生成は等号() {
        ok("fn f() {\n  Circle { r = 1, g = 2 }\n}\n");
    }

    #[test]
    fn 条件位置ではstruct生成と読まない() {
        // `while c { }` の `c { }` を struct リテラルにしてしまうと `:` が来ずに壊れる
        let e = parse_src("fn f() {\n  while c { x() }\n}\n").unwrap_err();
        assert!(e.msg.contains('`'), "{}", e.msg);
    }
}
