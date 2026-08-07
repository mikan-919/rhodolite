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
            Tok::Pub => {
                self.bump();
                Ok("pub".to_string())
            }
            Tok::As => {
                self.bump();
                Ok("as".to_string())
            }
            // 所有権の綴りも宣言位置の外では識別子に戻す。`use`/`pub`/`as` と同じ
            Tok::Mut => {
                self.bump();
                Ok("mut".to_string())
            }
            Tok::Move => {
                self.bump();
                Ok("move".to_string())
            }
            Tok::Indirect => {
                self.bump();
                Ok("indirect".to_string())
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
        while self.at(&Tok::Use) || self.starts_pub_use() {
            uses.push(self.use_decl()?);
            self.skip_newlines();
        }
        loop {
            self.skip_newlines();
            if self.at(&Tok::Eof) {
                break;
            }
            if self.at(&Tok::Use) || self.starts_pub_use() {
                return Err(self.err("`use` はモジュール先頭の接頭部にだけ書けます"));
            }
            items.push(self.item()?);
        }
        Ok(Program { uses, items })
    }

    /// `pub` 単体は識別子に戻るので、`use` が続くときだけ宣言の始まりと見る。
    fn starts_pub_use(&self) -> bool {
        self.at(&Tok::Pub) && self.toks.get(self.pos + 1).map(|t| &t.tok) == Some(&Tok::Use)
    }

    fn use_decl(&mut self) -> PResult<UseDecl> {
        let start = self.span();
        let public = self.eat(&Tok::Pub);
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
                    public,
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
            public,
            span: self.to(start),
        })
    }

    fn item(&mut self) -> PResult<Item> {
        let start = self.span();
        match self.peek() {
            Tok::Trait => {
                self.bump();
                let name = self.expect_ident("trait 名")?;
                let type_params = self.type_params()?;
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
                    type_params,
                    methods,
                    span: self.to(start),
                })
            }

            // `struct User { rank: Rank }`。`struct Gold {}` のようにフィールド0個も書ける
            Tok::Struct => {
                self.bump();
                let name = self.expect_ident("struct 名")?;
                self.reject_type_params("struct")?;
                self.expect(&Tok::LBrace, "`{`")?;
                let mut fields = Vec::new();
                loop {
                    self.skip_newlines();
                    if self.eat(&Tok::RBrace) {
                        break;
                    }
                    let indirect = self.indirect_modifier();
                    let fname = self.expect_ident("フィールド名")?;
                    self.expect(&Tok::Colon, "`:`")?;
                    fields.push(FieldDecl {
                        name: fname,
                        ty: self.ty()?,
                        indirect,
                    });
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
                self.reject_type_params("enum")?;
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
                            let indirect = self.indirect_modifier();
                            payload.push(PayloadDecl {
                                ty: self.ty()?,
                                indirect,
                            });
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
                let type_params = self.type_params()?;
                let (trait_ref, target) = self.impl_head()?;
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
                    type_params,
                    trait_ref,
                    target,
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

            Tok::Unsafe => Err(self.err("`unsafe` はありません")),

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

    /// `<T, U>` — 宣言が導入する型パラメータ。無ければ空(MAP-Q1)。
    ///
    /// `<` `>` は `with slot<Type>` でしか使われず、比較演算子でもないので、
    /// キーワードで位置が固定されたここでは曖昧にならない
    fn type_params(&mut self) -> PResult<Vec<TypeParam>> {
        if !self.eat(&Tok::Less) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        loop {
            let start = self.span();
            let name = self.expect_ident("型パラメータ名")?;
            params.push(TypeParam {
                name,
                span: self.to(start),
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::Greater, "`>`")?;
        Ok(params)
    }

    /// 型パラメータを取れない宣言で `<` を見たら、そこで止める(MAP-Q1)。
    fn reject_type_params(&mut self, kind: &str) -> PResult<()> {
        if self.at(&Tok::Less) {
            return Err(self.err(&format!("{kind} には型パラメータを書けません")));
        }
        Ok(())
    }

    /// `Map<T> for [T]` / `Database for Postgres` / `Postgres` — `impl` の頭。
    ///
    /// trait 参照は識別子で始まり `for` で閉じる。trait を持たない `impl` の
    /// 対象型は型注釈の文法そのままなので、`impl [T]` も同じ道を通る
    fn impl_head(&mut self) -> PResult<(Option<TraitRef>, Type)> {
        if !matches!(self.peek(), Tok::Ident(_)) {
            return Ok((None, self.ty()?));
        }
        let name = self.name_path("trait 名または型名")?;
        let args = self.type_args()?;
        if self.eat(&Tok::For) {
            return Ok((Some(TraitRef { name, args }), self.ty()?));
        }
        if !args.is_empty() {
            return Err(self.err(
                "`impl` の対象型に型引数は書けません。trait を実装するなら `for` が必要です",
            ));
        }
        Ok((
            None,
            Type {
                mode: TypeMode::Owned,
                kind: TypeKind::Named(name),
                optional: false,
            },
        ))
    }

    /// `<T, [U]>` — trait 参照に渡す型引数。無ければ空
    fn type_args(&mut self) -> PResult<Vec<Type>> {
        if !self.eat(&Tok::Less) {
            return Ok(Vec::new());
        }
        let mut args = vec![self.ty()?];
        while self.eat(&Tok::Comma) {
            args.push(self.ty()?);
        }
        self.expect(&Tok::Greater, "`>`")?;
        Ok(args)
    }

    /// `fn find(id: int -> User?)` — 戻り値の `->` は括弧の内側にある。
    /// `fn now(-> int)` のように引数ゼロで戻り値だけ、も書ける。
    fn sig(&mut self) -> PResult<Sig> {
        let start = self.span();
        let name = self.expect_ident("関数名")?;
        // 自由関数・trait メソッド・impl メソッドはこの1本を共有するので、
        // `fn map<U>(...)` の構文は3箇所ぶん同時に入る
        let type_params = self.type_params()?;
        self.expect(&Tok::LParen, "`(`")?;

        // `fn save(self, u: User -> unit)` — self は型を書かない。
        // これがある/ないだけがメソッドと関連関数の区別。所有モードは
        // `self` / `&self` / `&mut self` の3つ(design.md 決定2)
        let receiver = self.receiver_mode()?;
        if receiver.is_some() {
            self.eat(&Tok::Comma);
        }

        let (params, ret) = self.fn_params_and_ret()?;

        self.expect(&Tok::RParen, "`)`")?;
        // 破棄はコンパイラが決めるので、利用者が書ける後始末フックは無い。
        // 対象はレシーバを取るものだけ。レシーバの無い自由関数は後始末フックに
        // なりようがないので従来どおり(deterministic-destruction)
        if receiver.is_some() && matches!(name.as_str(), "drop" | "deinit" | "finalize") {
            return Err(Diag::at(
                start,
                format!(
                    "`{name}` はユーザー定義のデストラクタになるため宣言できません。破棄はコンパイラが決めます"
                ),
            ));
        }
        Ok(Sig {
            name,
            type_params,
            receiver,
            params,
            ret,
            span: self.to(start),
        })
    }

    /// `(a: int, b: str -> R)` の中身、開き `(` とレシーバの後ろから閉じ `)` の
    /// 手前まで。名前付き関数の `sig()` と無名関数リテラルで共有する —
    /// 引数名・型注釈は必須で、期待型や本体からの推論はしない(CLO-Q1)。
    /// `->` を省くと戻り値は `unit` (`typecheck::effective_ret`)
    fn fn_params_and_ret(&mut self) -> PResult<(Vec<Param>, Option<Type>)> {
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
        Ok((params, ret))
    }

    /// 引数リストの先頭のレシーバ。`&` は引数名にはなれないので曖昧にならない。
    fn receiver_mode(&mut self) -> PResult<Option<ReceiverMode>> {
        if self.eat(&Tok::SelfKw) {
            return Ok(Some(ReceiverMode::Owned));
        }
        if !self.at(&Tok::Amp) {
            return Ok(None);
        }
        self.bump();
        let mode = if self.eat(&Tok::Mut) {
            ReceiverMode::Mutable
        } else {
            ReceiverMode::Shared
        };
        self.expect(&Tok::SelfKw, "`self`")?;
        Ok(Some(mode))
    }

    /// `indirect` は宣言位置の修飾。型が続かないなら普通の識別子に戻す。
    fn indirect_modifier(&mut self) -> bool {
        let next = self.toks.get(self.pos + 1).map(|t| &t.tok);
        if !self.at(&Tok::Indirect) || !matches!(next, Some(Tok::Ident(_) | Tok::LBracket)) {
            return false;
        }
        self.bump();
        true
    }

    /// `User` / `[User]` / `[User?]?` / `&User` / `&mut User`。後置 `?` は
    /// 直前の完成した型に付くので、`[T]?` と `[T?]` は別物(design.md 決定2)。
    fn ty(&mut self) -> PResult<Type> {
        let mode = if self.eat(&Tok::Amp) {
            if self.eat(&Tok::Mut) {
                TypeMode::Mutable
            } else {
                TypeMode::Shared
            }
        } else {
            TypeMode::Owned
        };
        match self.peek() {
            Tok::Star => {
                return Err(self.err("生ポインタ型はありません。`&T` か `&mut T` と書きます"));
            }
            Tok::Amp => return Err(self.err("参照の参照は書けません")),
            _ => {}
        }
        let kind = if self.eat(&Tok::LBracket) {
            let element = self.ty()?;
            self.expect(&Tok::RBracket, "`]`")?;
            TypeKind::Array(Box::new(element))
        } else if self.eat(&Tok::Fn) {
            // `fn(P1, P2 -> R)`。宣言の署名と違い、名前も本体も持たない
            self.expect(&Tok::LParen, "`(`")?;
            let mut params = Vec::new();
            while !self.at(&Tok::Arrow) {
                params.push(self.ty()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::Arrow, "`->`")?;
            let result = self.ty()?;
            self.expect(&Tok::RParen, "`)`")?;
            TypeKind::Callable {
                params,
                result: Box::new(result),
            }
        } else {
            TypeKind::Named(self.name_path("型名")?)
        };
        let optional = self.eat(&Tok::Question);
        Ok(Type {
            mode,
            kind,
            optional,
        })
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
    /// 限定 pattern には任意の `if 条件` が続けられ、本体は既存 Head と同じ
    /// `: 単純式` か `{ ... }`(design.md 決定1・2)。
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
            // `Enum::Variant(payload) if condition` — 本体の `{` を struct
            // literal と読まないよう、`if`/`while` と同じ cond で読む
            // (design.md 決定2)
            let guard = if self.at(&Tok::If) {
                if matches!(pattern, MatchPattern::CatchAll) {
                    return Err(self.err("`_` に `if` は付けられません"));
                }
                self.bump();
                Some(Box::new(self.cond()?))
            } else {
                None
            };
            let body = self.head_body()?;
            arms.push(MatchArm {
                pattern,
                guard,
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

    /// arm 全体の pattern。限定 `Enum::Variant` に payload 要素が続く形か、
    /// 残りの variant を全部受ける `_`(design.md 決定2)。
    /// 一意性と最後であることは arm span を持つ型検査の側で見る。
    fn match_pattern(&mut self) -> PResult<MatchPattern> {
        if self.eat(&Tok::Ident("_".to_string())) {
            // `_` は値を晒さないので payload を書く先が無い
            if self.at(&Tok::LParen) {
                return Err(self.err("`_` は payload を束縛できません"));
            }
            return Ok(MatchPattern::CatchAll);
        }

        let path = self.name_path("arm の `Enum::Variant` または `_`")?;
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
                // モードは対象全体に一つ。pattern ごとの部分的な move は無い
                // (design.md 決定7)。`move` / `mut` は他の位置と同じく、
                // 場所が続くときだけ修飾。続かなければただの束縛名
                if self.at(&Tok::Amp)
                    || (matches!(self.peek(), Tok::Move | Tok::Mut) && self.place_follows())
                {
                    return Err(self.err(
                        "pattern に所有権修飾は書けません。`match` の対象に `&mut` か `move` を付けます",
                    ));
                }
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
        if let Some(mode) = self.access_mode()? {
            return self.qualified_place(mode, start);
        }
        self.postfix()
    }

    /// `&` / `&mut` / `move` の接頭辞。所有権修飾は場所に付くので、単項演算子
    /// ではなくここで場所ごと読む(design.md 決定2)。
    ///
    /// `move` / `mut` は場所が続くときだけ修飾。続かなければ従来どおり識別子。
    fn access_mode(&mut self) -> PResult<Option<AccessMode>> {
        if self.eat(&Tok::Amp) {
            return Ok(Some(if self.eat(&Tok::Mut) {
                AccessMode::Mutable
            } else {
                AccessMode::Shared
            }));
        }
        if !matches!(self.peek(), Tok::Move | Tok::Mut) || !self.place_follows() {
            return Ok(None);
        }
        if self.eat(&Tok::Move) {
            return Ok(Some(AccessMode::Move));
        }
        Err(self.err("`mut` だけでは修飾になりません。`&mut place` か `let mut` と書きます"))
    }

    /// 次のトークンから場所(あるいは重ねた修飾)が始まるか。
    /// これが偽なら `move` / `mut` は従来どおりただの名前。
    ///
    /// ponytail: `(` を場所の始まりに入れられないので `move (u.name)` は
    /// `move` という名前の呼び出しに読める。`move` の識別子用法を捨てるか
    /// 修飾を予約語にするまでこの天井は残る。`&(u.name)` と `(move u.name)`
    /// は書けるので回避路はある
    fn place_follows(&self) -> bool {
        matches!(
            self.toks.get(self.pos + 1).map(|t| &t.tok),
            Some(Tok::Ident(_) | Tok::SelfKw | Tok::Amp | Tok::Move | Tok::Mut)
        )
    }

    /// 修飾された場所。末尾がメソッド呼び出しならレシーバに、そうでなければ
    /// 射影全体に付く。括弧は受けるが要らない(design.md 決定2)。
    fn qualified_place(&mut self, mode: AccessMode, start: Span) -> PResult<Expr> {
        if matches!(self.peek(), Tok::Amp | Tok::Move | Tok::Mut) {
            return Err(self.err("所有権修飾は重ねて書けません"));
        }
        let Expr { kind, span: inner } = self.postfix()?;
        let span = self.to(start);
        let to_inner = |end: u32| Span {
            src: start.src,
            start: start.start,
            end,
        };

        // `&mut account.user.rename(name)` — 修飾が付くのは `account.user`
        match kind {
            ExprKind::Call(callee, args) if matches!(callee.kind, ExprKind::Field(..)) => {
                let callee_span = callee.span;
                let ExprKind::Field(receiver, method) = callee.kind else {
                    unreachable!("直前の照合で Field と分かっている")
                };
                let place = Expr {
                    span: to_inner(receiver.span.end),
                    kind: ExprKind::Access {
                        mode,
                        place: receiver,
                    },
                };
                Ok(Expr {
                    kind: ExprKind::Call(
                        Box::new(Expr {
                            kind: ExprKind::Field(Box::new(place), method),
                            span: to_inner(callee_span.end),
                        }),
                        args,
                    ),
                    span,
                })
            }
            kind => Ok(Expr {
                kind: ExprKind::Access {
                    mode,
                    place: Box::new(Expr { kind, span: inner }),
                },
                span,
            }),
        }
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
            } else if self.eat(&Tok::LBracket) {
                // 角括弧の中では struct リテラルを禁じる理由がない
                let saved = std::mem::replace(&mut self.no_struct, false);
                let index = self.expr();
                self.no_struct = saved;
                let index = index?;
                self.expect(&Tok::RBracket, "`]`")?;
                e = Expr {
                    kind: ExprKind::Index(Box::new(e), Box::new(index)),
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
            Tok::Pub => {
                self.bump();
                ExprKind::Ident("pub".to_string())
            }
            Tok::As => {
                self.bump();
                ExprKind::Ident("as".to_string())
            }
            // 場所が続かなかった所有権の綴りは、従来どおりただの名前
            Tok::Mut => {
                self.bump();
                ExprKind::Ident("mut".to_string())
            }
            Tok::Move => {
                self.bump();
                ExprKind::Ident("move".to_string())
            }
            Tok::Indirect => {
                self.bump();
                ExprKind::Ident("indirect".to_string())
            }

            Tok::Let => {
                self.bump();
                // `let` は不変。可変にするのは `let mut` だけ
                let mutable = self.eat(&Tok::Mut);
                let name = self.expect_ident("変数名")?;
                // 型注釈は引数・フィールド・戻り値と同じ型文法を使う
                let annotation = if self.eat(&Tok::Colon) {
                    Some(self.ty()?)
                } else {
                    None
                };
                self.expect(&Tok::Eq, "`=`")?;
                // 値は `stmt` で読む。値ベースなので Head も値を産む
                // (`let r = with db(replica) { collect() }` — CONTEXT.md「第二級ブロック」)。
                // `expr` で読むと `:` が let の外に残り、`let` 全体が
                // ambient の binder として読まれてしまう
                let value = self.stmt()?;
                ExprKind::Let {
                    name,
                    mutable,
                    annotation,
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

            // `fn(x: int -> int) { x }` — 名前を省いた関数リテラル。式位置の
            // `fn` は必ずこれ。宣言位置の `fn` には名前が続き、型注釈位置の
            // `fn(...)` は `ty()` が読むので、先読みは要らない(design.md 決定2)
            Tok::Fn => {
                self.bump();
                self.expect(&Tok::LParen, "`(`")?;
                let (params, ret) = self.fn_params_and_ret()?;
                self.expect(&Tok::RParen, "`)`")?;
                let body = self.block()?;
                ExprKind::Closure { params, ret, body }
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

            // 安全性は実行前の検査で閉じるので、抜け道は用意しない
            Tok::Unsafe => {
                return Err(self.err("`unsafe` はありません"));
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
                let payload = v
                    .payload
                    .iter()
                    .map(|p| {
                        let ty = show_type(&p.ty);
                        if p.indirect {
                            format!("indirect {ty}")
                        } else {
                            ty
                        }
                    })
                    .collect();
                (v.name.clone(), payload)
            })
            .collect()
    }

    fn show_type(ty: &Type) -> String {
        let base = match &ty.kind {
            TypeKind::Named(name) => name.clone(),
            TypeKind::Array(element) => format!("[{}]", show_type(element)),
            TypeKind::Callable { params, result } => {
                let params: Vec<String> = params.iter().map(show_type).collect();
                format!(
                    "fn({}-> {})",
                    crate::hir::spelled_params(&params),
                    show_type(result)
                )
            }
        };
        let base = if ty.optional {
            format!("{base}?")
        } else {
            base
        };
        match ty.mode {
            TypeMode::Owned => base,
            TypeMode::Shared => format!("&{base}"),
            TypeMode::Mutable => format!("&mut {base}"),
        }
    }

    /// 所有権修飾の付き先が見えるところまで式を綴る。他の形は種別名だけ
    fn show_expr(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Ident(name) => name.clone(),
            ExprKind::Path(parts) => parts.join("::"),
            ExprKind::Field(recv, name) => format!("{}.{name}", show_expr(recv)),
            ExprKind::OptionalField(recv, name) => format!("{}.?{name}", show_expr(recv)),
            ExprKind::Call(callee, args) => {
                let args: Vec<String> = args.iter().map(show_expr).collect();
                format!("{}({})", show_expr(callee), args.join(", "))
            }
            ExprKind::Index(base, index) => {
                format!("{}[{}]", show_expr(base), show_expr(index))
            }
            ExprKind::Access { mode, place } => {
                let mode = match mode {
                    AccessMode::Shared => "&",
                    AccessMode::Mutable => "&mut ",
                    AccessMode::Move => "move ",
                };
                format!("({mode}{})", show_expr(place))
            }
            ExprKind::Binary { op, lhs, rhs } => {
                format!("({} {op:?} {})", show_expr(lhs), show_expr(rhs))
            }
            other => format!("{other:?}"),
        }
    }

    /// 関数本体の最初の式
    fn first(src: &str) -> String {
        let p = ok(src);
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!("fn ではない: {:?}", p.items[0])
        };
        show_expr(&body[0])
    }

    fn error(src: &str) -> String {
        match parse_src(src) {
            Ok(_) => panic!("通ってしまった:\n{src}"),
            Err(e) => e.msg,
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

    /// 引数の型の綴りを取り出す(tasks 1.3)
    fn param_types(src: &str) -> Vec<String> {
        let p = ok(src);
        let Item::Fn { sig, .. } = &p.items[0] else {
            panic!("fn ではない: {:?}", p.items[0])
        };
        sig.params.iter().map(|p| show_type(&p.ty)).collect()
    }

    #[test]
    fn callable型は引数位置に書ける() {
        assert_eq!(
            param_types("fn apply(f: fn(int -> int), value: int -> int) { f(value) }\n"),
            vec!["fn(int -> int)".to_string(), "int".to_string()]
        );
    }

    #[test]
    fn callable型は引数を0個でも複数でも取れる() {
        assert_eq!(
            param_types("fn run(a: fn(-> int), b: fn(&User, int -> bool) -> int) { 0 }\n"),
            vec![
                "fn(-> int)".to_string(),
                "fn(&User, int -> bool)".to_string()
            ]
        );
    }

    #[test]
    fn callable型の結果は省略できない() {
        let e = parse_src("fn apply(f: fn(int) -> int) { 0 }\n").unwrap_err();
        assert!(e.msg.contains("`->`"), "{}", e.msg);
    }

    #[test]
    fn 局所束縛にもcallable注釈を書ける() {
        let p = ok("fn main(-> int) { let f: fn(int -> int) = double\n f(1) }\n");
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!("fn ではない")
        };
        let ExprKind::Let { annotation, .. } = &body[0].kind else {
            panic!("let ではない: {:?}", body[0].kind)
        };
        assert_eq!(
            show_type(annotation.as_ref().expect("注釈がある")),
            "fn(int -> int)"
        );
    }

    /// 既存の直接呼び出しの綴りは何も変わらない(tasks 1.3)
    #[test]
    fn 直接呼び出しの綴りは変わらない() {
        assert_eq!(
            param_types("fn stamp(u: &mut User, at: int) { u.at = at }\n"),
            vec!["&mut User".to_string(), "int".to_string()]
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

    // ---- let の型注釈 ----

    /// 本体の最初の `let` の注釈を綴りで取り出す。注釈が無ければ `None`
    fn let_annotation(src: &str) -> Option<String> {
        let p = ok(src);
        let Item::Fn { body, .. } = &p.items[0] else {
            panic!("fn ではない: {:?}", p.items[0])
        };
        let ExprKind::Let { annotation, .. } = &body[0].kind else {
            panic!("let ではない: {:?}", body[0].kind)
        };
        annotation.as_ref().map(show_type)
    }

    #[test]
    fn letは型注釈を省略できる() {
        assert_eq!(let_annotation("fn f() {\n  let n = 1\n}\n"), None);
    }

    #[test]
    fn letの型注釈は名前付き型を取る() {
        assert_eq!(
            let_annotation("fn f() {\n  let n: int = 1\n}\n"),
            Some("int".to_string())
        );
    }

    /// 注釈は引数・フィールド・戻り値と同じ型文法なので、後置 `?` も
    /// 角括弧もそのまま読める
    #[test]
    fn letの型注釈はoptionalと配列の形をそのまま取る() {
        assert_eq!(
            let_annotation("fn f() {\n  let u: User? = nil\n}\n"),
            Some("User?".to_string())
        );
        assert_eq!(
            let_annotation("fn f() {\n  let us: [User] = []\n}\n"),
            Some("[User]".to_string())
        );
        assert_eq!(
            let_annotation("fn f() {\n  let us: [User?]? = nil\n}\n"),
            Some("[User?]?".to_string())
        );
    }

    #[test]
    fn letの型注釈は型名を要求する() {
        let e = parse_src("fn f() {\n  let n: 1 = 1\n}\n").unwrap_err();
        assert!(e.msg.contains("型名"), "{}", e.msg);
    }

    #[test]
    fn 型注釈のあるletも初期化子を要求する() {
        let e = parse_src("fn f() {\n  let n: int\n}\n").unwrap_err();
        assert!(e.msg.contains("`=`"), "{}", e.msg);
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

    /// `_` は arm 全体の pattern として、単純式形もブロック形も取れる
    #[test]
    fn armの全体patternにアンダースコアを書ける() {
        assert_eq!(
            arm_patterns(
                "fn f(r: Rank) {\n\
                  \x20 let x = match r {\n\
                  \x20   Rank::Gold: 1\n\
                  \x20   _ {\n\
                  \x20     audit()\n\
                  \x20     0\n\
                  \x20   }\n\
                  \x20 }\n\
                  }\n"
            ),
            vec![
                ("Rank::Gold".to_string(), Vec::new()),
                ("_".to_string(), Vec::new()),
            ]
        );
    }

    #[test]
    fn 全体patternのアンダースコアはpayloadを取れない() {
        let e = parse_src("fn f(l: Lookup) {\n match l { _(reason): 1 }\n}\n").unwrap_err();
        assert!(e.msg.contains("payload を束縛できません"), "{}", e.msg);
    }

    /// arm の guard の有無を取り出す
    fn arm_guards(src: &str) -> Vec<bool> {
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
        arms.iter().map(|a| a.guard.is_some()).collect()
    }

    #[test]
    fn 限定armにifのguardを書ける() {
        // 単純式の本体でもブロックの本体でも guard を読み終えてから本体へ進む
        assert_eq!(
            arm_guards(
                "fn f(l: Lookup) {\n\
                  \x20 let x = match l {\n\
                  \x20   Lookup::Found(user) if user.age == 1: 1\n\
                  \x20   Lookup::Missing(reason, _) if ready {\n\
                  \x20     2\n\
                  \x20   }\n\
                  \x20   Lookup::Skipped: 3\n\
                  \x20   _: 4\n\
                  \x20 }\n\
                  }\n"
            ),
            vec![true, true, false, false]
        );
    }

    #[test]
    fn guardの直後のbraceは本体になる() {
        // `if ready { .. }` の `{` を `ready` の struct リテラルにはしない
        assert_eq!(
            arm_guards(
                "fn f(r: Rank) {\n let x = match r {\n Rank::Gold if ready { 1 }\n _: 0\n }\n}\n"
            ),
            vec![true, false]
        );
    }

    #[test]
    fn アンダースコアにguardは付けられない() {
        let e = parse_src("fn f(r: Rank) {\n match r { _ if ready: 1 }\n}\n").unwrap_err();
        assert!(e.msg.contains("`_` に `if`"), "{}", e.msg);
    }

    #[test]
    fn guardの条件が無いと落ちる() {
        let e = parse_src("fn f(r: Rank) {\n match r { Rank::Gold if: 1 }\n}\n").unwrap_err();
        assert!(!e.msg.is_empty());
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
            trait_ref,
            target,
            methods,
            ..
        } = &p.items[0]
        else {
            panic!()
        };
        assert_eq!(
            trait_ref.as_ref().map(|r| r.name.as_str()),
            Some("Database")
        );
        assert_eq!(target.to_string(), "Postgres");
        // self は params には入らない。has_self に出る
        assert!(methods[0].0.has_self());
        assert_eq!(methods[0].0.params.len(), 1);
        assert!(!methods[1].0.has_self());
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
                mode: TypeMode::Owned,
                kind: TypeKind::Named(name.to_string()),
                optional,
            }
        }
        fn array(element: Type, optional: bool) -> Type {
            Type {
                mode: TypeMode::Owned,
                kind: TypeKind::Array(Box::new(element)),
                optional,
            }
        }

        let p = ok("struct Store { users: [User]\ntags: [[str]?] }\n\
                    fn pick(xs: [User], ys: [User?]? -> [User]?) {\n 1\n}\n");

        let Item::Struct { fields, .. } = &p.items[0] else {
            panic!()
        };
        assert_eq!(fields[0].ty, array(named("User", false), false));
        assert_eq!(
            fields[1].ty,
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

    /// `pub use` は `use` と同じ形をすべて受ける。違いは公開フラグ1つだけ
    #[test]
    fn pub_useはuseと同じ形を受けて公開フラグだけが変わる() {
        let p = ok("pub use data::database as db_module\n\
             pub use services::{\n\
             \x20 users,\n\
             \x20 billing as payments,\n\
             }\n\
             use private_dep\n\
             fn main() { db_module::connect() }\n");
        assert_eq!(p.uses.len(), 3);
        assert!(p.uses[0].public);
        assert_eq!(p.uses[0].alias.as_deref(), Some("db_module"));
        assert!(p.uses[1].public);
        assert_eq!(
            p.uses[1].members.as_ref().unwrap()[1].alias.as_deref(),
            Some("payments")
        );
        assert!(!p.uses[2].public);
    }

    #[test]
    fn pub_useもトップレベル接頭部にだけ書ける() {
        let error = parse_src("fn first() { 1 }\npub use services\n").unwrap_err();
        assert!(error.msg.contains("先頭"), "{}", error.msg);
    }

    /// `pub` 単体は宣言を始めない。`use` が続かなければ従来どおり識別子
    #[test]
    fn pubはuse以外では既存どおり識別子として使える() {
        ok("fn pub() { 1 }\nfn main() { pub() }\n");
    }

    #[test]
    fn useに相対モジュールパスは書けない() {
        let error = parse_src("use super::services\nfn main() { 0 }\n").unwrap_err();
        assert!(error.msg.contains("絶対"), "{}", error.msg);
    }

    // -----------------------------------------------------------------------
    // 所有権と借用の構文 (introduce-ownership-and-borrowing フェーズ1)
    // -----------------------------------------------------------------------

    /// `&T` / `&mut T` は名前が書ける型位置ならどこでも読める
    #[test]
    fn 参照型を全ての型位置で読む() {
        let p = ok("struct Session { user: &User\nedit: &mut [User]? }\n\
                    fn touch(u: &User, e: &mut User -> &[User]?) {\n\
                    \x20 let v: &mut User = e\n\
                    \x20 1\n\
                    }\n\
                    enum Ref { One(&User, &mut [str]) }\n");

        let Item::Struct { fields, .. } = &p.items[0] else {
            panic!()
        };
        assert_eq!(show_type(&fields[0].ty), "&User");
        assert_eq!(
            show_type(&fields[1].ty),
            "&mut [User]?",
            "後置 `?` は参照の内側の完成した型に付く"
        );

        let Item::Fn { sig, body, .. } = &p.items[1] else {
            panic!()
        };
        assert_eq!(show_type(&sig.params[0].ty), "&User");
        assert_eq!(show_type(&sig.params[1].ty), "&mut User");
        assert_eq!(show_type(sig.ret.as_ref().unwrap()), "&[User]?");
        let ExprKind::Let { annotation, .. } = &body[0].kind else {
            panic!("let ではない: {:?}", body[0].kind)
        };
        assert_eq!(show_type(annotation.as_ref().unwrap()), "&mut User");

        assert_eq!(
            variants("enum Ref { One(&User, &mut [str]) }\n"),
            vec![(
                "One".to_string(),
                vec!["&User".to_string(), "&mut [str]".to_string()]
            )]
        );
    }

    /// 所有型・共有借用・排他借用は別の型として綴られる
    #[test]
    fn 所有と借用は別の型になる() {
        let p = ok("fn f(a: User, b: &User, c: &mut User) {\n 1\n}\n");
        let Item::Fn { sig, .. } = &p.items[0] else {
            panic!()
        };
        let modes: Vec<TypeMode> = sig.params.iter().map(|p| p.ty.mode).collect();
        assert_eq!(
            modes,
            vec![TypeMode::Owned, TypeMode::Shared, TypeMode::Mutable]
        );
        assert_ne!(sig.params[0].ty, sig.params[1].ty);
        assert_ne!(sig.params[1].ty, sig.params[2].ty);
    }

    /// `self` / `&self` / `&mut self` とレシーバ無しの4通り
    #[test]
    fn レシーバの所有モードを読み分ける() {
        let p = ok("impl Database for Postgres {\n\
                    \x20 fn finish(self -> unit) { 1 }\n\
                    \x20 fn find(&self, id: int -> User?) { nil }\n\
                    \x20 fn save(&mut self, u: User -> unit) { 1 }\n\
                    \x20 fn new(url: str -> Postgres) { 2 }\n\
                    }\n");
        let Item::Impl { methods, .. } = &p.items[0] else {
            panic!()
        };
        let receivers: Vec<Option<ReceiverMode>> = methods.iter().map(|m| m.0.receiver).collect();
        assert_eq!(
            receivers,
            vec![
                Some(ReceiverMode::Owned),
                Some(ReceiverMode::Shared),
                Some(ReceiverMode::Mutable),
                None,
            ]
        );
        // self は params に入らない。`&self` でも同じ
        assert_eq!(methods[1].0.params.len(), 1);
        assert!(methods[1].0.has_self());
        assert!(!methods[3].0.has_self());

        // trait 側も同じ形を宣言できる
        let p = ok(
            "trait Database {\n fn find(&self, id: int -> User?)\n fn save(&mut self, u: User -> unit)\n}\n",
        );
        let Item::Trait { methods, .. } = &p.items[0] else {
            panic!()
        };
        assert_eq!(methods[0].receiver, Some(ReceiverMode::Shared));
        assert_eq!(methods[1].receiver, Some(ReceiverMode::Mutable));
    }

    #[test]
    fn 参照だけのレシーバはselfを要求する() {
        assert!(error("impl P {\n fn f(&mut x: int) { 1 }\n}\n").contains("`self`"));
    }

    /// `let` は不変、`let mut` だけが可変。注釈はどちらにも付く
    #[test]
    fn letとlet_mutを読み分ける() {
        fn mutability(src: &str) -> (bool, Option<String>) {
            let p = ok(src);
            let Item::Fn { body, .. } = &p.items[0] else {
                panic!()
            };
            let ExprKind::Let {
                mutable,
                annotation,
                ..
            } = &body[0].kind
            else {
                panic!("let ではない: {:?}", body[0].kind)
            };
            (*mutable, annotation.as_ref().map(show_type))
        }

        assert_eq!(mutability("fn f() {\n let n = 1\n}\n"), (false, None));
        assert_eq!(mutability("fn f() {\n let mut n = 1\n}\n"), (true, None));
        assert_eq!(
            mutability("fn f() {\n let mut u: User = value\n}\n"),
            (true, Some("User".to_string()))
        );
        assert_eq!(
            mutability("fn f() {\n let v: &User = other\n}\n"),
            (false, Some("&User".to_string()))
        );

        // 初期化子の所有権修飾も普通の式として読む
        for (src, want) in [
            ("fn f() {\n let v = &user\n}\n", "(&user)"),
            ("fn f() {\n let e = &mut user\n}\n", "(&mut user)"),
            ("fn f() {\n let o = move user\n}\n", "(move user)"),
        ] {
            let p = ok(src);
            let Item::Fn { body, .. } = &p.items[0] else {
                panic!()
            };
            let ExprKind::Let { value, .. } = &body[0].kind else {
                panic!("let ではない: {:?}", body[0].kind)
            };
            assert_eq!(show_expr(value), want, "{src}");
        }
    }

    /// `indirect` は struct フィールドと enum payload の宣言位置に付く
    #[test]
    fn indirectは宣言の所有エッジに付く() {
        let p = ok("struct Node { value: int\nindirect next: Node? }\n");
        let Item::Struct { fields, .. } = &p.items[0] else {
            panic!()
        };
        assert!(!fields[0].indirect);
        assert!(fields[1].indirect);
        assert_eq!(show_type(&fields[1].ty), "Node?");

        assert_eq!(
            variants("enum List { Cons(int, indirect List)\nEmpty }\n"),
            vec![
                (
                    "Cons".to_string(),
                    vec!["int".to_string(), "indirect List".to_string()]
                ),
                ("Empty".to_string(), Vec::new()),
            ]
        );
    }

    /// 宣言位置の外の `indirect` は従来どおりただの名前
    #[test]
    fn indirectは修飾でなければ識別子に戻る() {
        let p = ok("struct S { indirect: int }\nfn indirect() { 1 }\nfn f() { indirect() }\n");
        let Item::Struct { fields, .. } = &p.items[0] else {
            panic!()
        };
        assert_eq!(fields[0].name, "indirect");
        assert!(!fields[0].indirect);
    }

    /// 修飾が付くのは場所。末尾のメソッド呼び出しがあればそのレシーバ
    /// (design.md 決定2)
    #[test]
    fn 所有権修飾は場所に付く() {
        // メソッド呼び出しが無ければ射影全体
        assert_eq!(first("fn f() { &user }\n"), "(&user)");
        assert_eq!(first("fn f() { &mut user.name }\n"), "(&mut user.name)");
        assert_eq!(first("fn f() { move user.name }\n"), "(move user.name)");
        assert_eq!(first("fn f() { &self.rank }\n"), "(&self.rank)");

        // 末尾がメソッド呼び出しならレシーバだけ
        assert_eq!(
            first("fn f() { &mut account.user.rename(name) }\n"),
            "(&mut account.user).rename(name)"
        );
        assert_eq!(
            first("fn f() { move value.finish() }\n"),
            "(move value).finish()"
        );
        assert_eq!(
            first("fn f() { &mut a.b().c() }\n"),
            "(&mut a.b()).c()",
            "レシーバは末尾呼び出しの直前まで"
        );

        // 呼び出しがメソッドでなければ射影全体に付く
        assert_eq!(first("fn f() { move make(x) }\n"), "(move make(x))");
    }

    /// 括弧は受けるが要らない。同じ構文木になる(design.md 決定2)
    #[test]
    fn 所有権修飾に括弧は要らない() {
        for (bare, parenthesized) in [
            (
                "fn f() { &mut account.user.rename(name) }\n",
                "fn f() { (&mut account.user).rename(name) }\n",
            ),
            (
                "fn f() { move value.finish() }\n",
                "fn f() { (move value).finish() }\n",
            ),
            (
                "fn f() { &mut user.name }\n",
                "fn f() { &mut (user.name) }\n",
            ),
        ] {
            assert_eq!(first(bare), first(parenthesized), "{bare}");
        }
    }

    /// 行継続規則は修飾された場所の後置連鎖にもそのまま効く
    #[test]
    fn 所有権修飾は複数行の後置連鎖に跨がる() {
        assert_eq!(
            first("fn f() {\n &mut account\n .user\n .rename(name)\n}\n"),
            "(&mut account.user).rename(name)"
        );
        assert_eq!(
            first("fn f() {\n save(\n move user\n )\n}\n"),
            "save((move user))"
        );
    }

    /// 修飾は二項演算子の被演算子にもそのまま置ける
    #[test]
    fn 所有権修飾は式の中に置ける() {
        assert_eq!(
            first("fn f() { &a.name == &b.name }\n"),
            "((&a.name) Eq (&b.name))"
        );
        assert_eq!(
            first("fn f() { found ?? move fallback }\n"),
            "(found Coalesce (move fallback))"
        );
        assert_eq!(
            first("fn f() { move found ?? fallback }\n"),
            "((move found) Coalesce fallback)"
        );
    }

    /// `match` / `for` / `with` の対象にも同じ修飾が付く(design.md 決定7)
    #[test]
    fn 制御構造の対象に所有権修飾を付けられる() {
        fn subject(src: &str) -> String {
            let p = ok(src);
            let Item::Fn { body, .. } = &p.items[0] else {
                panic!()
            };
            match &body[0].kind {
                ExprKind::Match { subject, .. } => show_expr(subject),
                ExprKind::Head {
                    head: Head::For { iter, .. },
                    ..
                } => show_expr(iter),
                ExprKind::Head {
                    head: Head::Ambient(provisions),
                    ..
                } => match &provisions[0] {
                    Provision::Value { value, .. } => show_expr(value),
                    Provision::Type { type_name, .. } => type_name.clone(),
                },
                other => panic!("対象を持たない: {other:?}"),
            }
        }

        assert_eq!(subject("fn f() {\n match &mut l { _: 1 }\n}\n"), "(&mut l)");
        assert_eq!(subject("fn f() {\n match move l { _: 1 }\n}\n"), "(move l)");
        assert_eq!(
            subject("fn f() {\n for u in &mut users { go() }\n}\n"),
            "(&mut users)"
        );
        assert_eq!(
            subject("fn f() {\n for u in move users { go() }\n}\n"),
            "(move users)"
        );
        assert_eq!(
            subject("fn f() {\n with db(&mut store) { go() }\n}\n"),
            "(&mut store)"
        );
        assert_eq!(
            subject("fn f() {\n with db(move store) { go() }\n}\n"),
            "(move store)"
        );
        // 型だけの提供は値を持たないので従来どおり
        assert_eq!(
            subject("fn f() {\n with db<Postgres> { go() }\n}\n"),
            "Postgres"
        );
    }

    // ---- 拒否する形 ----

    #[test]
    fn ライフタイム注釈は書けない() {
        let e = lex("fn f(u: &'a User) { 1 }\n").unwrap_err();
        assert!(e.msg.contains("ライフタイム"), "{}", e.msg);
        assert!(e.span.is_some(), "位置を持たない診断");
    }

    #[test]
    fn patternに所有権修飾は書けない() {
        for src in [
            "fn f(l: Lookup) {\n match l { Lookup::Found(move user): 1 }\n}\n",
            "fn f(l: Lookup) {\n match l { Lookup::Found(&user): 1 }\n}\n",
            "fn f(l: Lookup) {\n match l { Lookup::Found(&mut user): 1 }\n}\n",
        ] {
            assert!(error(src).contains("pattern に所有権修飾"), "{src}");
        }
    }

    #[test]
    fn 壊れた所有権修飾を報告する() {
        assert!(
            error("fn f() {\n mut user.rename(x)\n}\n").contains("`mut` だけでは"),
            "`mut` 単独は修飾にならない"
        );
        for src in [
            "fn f() {\n &&user\n}\n",
            "fn f() {\n move &user\n}\n",
            "fn f() {\n move move user\n}\n",
            "fn f() {\n &mut mut user\n}\n",
        ] {
            assert!(error(src).contains("重ねて書けません"), "{src}");
        }
        assert!(error("fn f(u: &&User) { 1 }\n").contains("参照の参照"));
        // 場所の続かない `&mut` に専用の文言は無く、式が無いところで落ちる
        assert!(
            error("fn f() {\n let x = &mut\n}\n").contains("式が必要です"),
            "{}",
            error("fn f() {\n let x = &mut\n}\n")
        );
    }

    #[test]
    fn 生ポインタ型はない() {
        for src in [
            "fn f(p: *User) { 1 }\n",
            "fn f(p: &*User) { 1 }\n",
            "struct S { p: *int }\n",
        ] {
            assert!(error(src).contains("生ポインタ"), "{src}");
        }
    }

    #[test]
    fn unsafeはない() {
        assert!(error("fn f() {\n unsafe { go() }\n}\n").contains("`unsafe`"));
        assert!(error("unsafe fn f() { 1 }\n").contains("`unsafe`"));
    }

    #[test]
    fn ユーザー定義のデストラクタは書けない() {
        for name in ["drop", "deinit", "finalize"] {
            let src = format!("impl Node {{\n fn {name}(self) {{ 1 }}\n}}\n");
            let e = error(&src);
            assert!(e.contains("デストラクタ"), "{src}: {e}");
        }
        // trait 宣言側も同じ
        assert!(error("trait Resource {\n fn drop(&mut self)\n}\n").contains("デストラクタ"));
        // 禁じるのは後始末フックになりうるレシーバ付きだけ。自由関数は従来どおり
        ok("fn drop(x: int -> int) {\n x\n}\n");
        ok("impl Node {\n fn drop(x: int -> int) { x }\n}\n");
    }

    /// `unsafe` は識別子に戻さない。戻すと 1.4 の診断を出す先が無くなる
    #[test]
    fn unsafeは識別子にも戻らない() {
        assert!(error("fn unsafe() { 1 }\n").contains("Unsafe"));
        assert!(error("fn f() {\n let unsafe = 1\n}\n").contains("変数名"));
        assert!(error("struct S { unsafe: int }\n").contains("フィールド名"));
    }

    /// 所有権の綴りは、修飾にならない位置では従来どおり識別子
    #[test]
    fn 所有権の綴りは修飾でなければ識別子に戻る() {
        ok("fn mut() { 1 }\nfn main() { mut() }\n");
        ok("fn move() { 1 }\nfn main() { move() }\n");
        ok("fn f(u: User) { u.mut }\n");
        assert_eq!(first("fn f() { move + 1 }\n"), "(move Add Int(1))");
        // pattern の束縛名にも使える(修飾は対象全体に付くのでここには来ない)
        assert_eq!(
            arm_patterns("fn f(l: Lookup) {\n let x = match l { Lookup::Found(move): 1 }\n}\n"),
            vec![("Lookup::Found".to_string(), vec!["move".to_string()])]
        );
    }

    /// 行末の所有権の綴りは識別子。落とすと次の行が繋がって別の木になる
    #[test]
    fn 行末の所有権の綴りは次の行を巻き込まない() {
        for (src, initializer) in [
            (
                "fn main( -> int) {\n let move = 1\n let x = move\n x\n}\n",
                "move",
            ),
            ("fn main(mut: int -> int) {\n let x = mut\n g()\n}\n", "mut"),
            (
                "fn main( -> int) {\n let indirect = 1\n let x = indirect\n x\n}\n",
                "indirect",
            ),
        ] {
            let p = ok(src);
            let Item::Fn { body, .. } = &p.items[0] else {
                panic!()
            };
            let last = body.len() - 1;
            let ExprKind::Let { value, .. } = &body[last - 1].kind else {
                panic!("let ではない: {:?}", body[last - 1].kind)
            };
            assert_eq!(show_expr(value), initializer, "{src}");
            // 続く行は巻き込まれず、独立した式のまま残っている
            assert!(matches!(
                &body[last].kind,
                ExprKind::Ident(_) | ExprKind::Call(..)
            ));
        }
    }

    /// ponytail: `move (place)` は `move` という名前の呼び出しに読める。
    /// `&(place)` と `(move place)` は書けるので回避路はある
    #[test]
    fn moveと括弧の組み合わせは呼び出しに読める() {
        assert_eq!(first("fn f() { move (u.name) }\n"), "move(u.name)");
        assert_eq!(first("fn f() { &(u.name) }\n"), "(&u.name)");
        assert_eq!(first("fn f() { (move u.name) }\n"), "(move u.name)");
    }

    // -----------------------------------------------------------------------
    // 型パラメータ(MAP-010)
    // -----------------------------------------------------------------------

    /// 型パラメータ名とその綴りの範囲。span は重複診断がそのまま指す位置
    fn param_spellings(src: &str, params: &[TypeParam]) -> Vec<String> {
        params
            .iter()
            .map(|p| {
                assert_eq!(
                    &src[p.span.start as usize..p.span.end as usize],
                    p.name,
                    "span が名前を指していない: {p:?}"
                );
                p.name.clone()
            })
            .collect()
    }

    #[test]
    fn fnは名前の後ろに型パラメータを取る() {
        let src = "fn identity<T>(x: T -> T) { x }\n";
        let p = ok(src);
        let Item::Fn { sig, .. } = &p.items[0] else {
            panic!("fn ではない")
        };
        assert_eq!(param_spellings(src, &sig.type_params), ["T"]);
        assert_eq!(sig.params[0].ty.to_string(), "T");
        assert_eq!(sig.ret.as_ref().unwrap().to_string(), "T");
    }

    #[test]
    fn 型パラメータは複数書ける() {
        let src = "fn apply<T, U>(f: fn(T -> U), x: T -> U) { f(x) }\n";
        let p = ok(src);
        let Item::Fn { sig, .. } = &p.items[0] else {
            panic!("fn ではない")
        };
        assert_eq!(param_spellings(src, &sig.type_params), ["T", "U"]);
    }

    /// trait とその method、`impl` とその method の4箇所すべてに乗る
    #[test]
    fn traitとimplは型パラメータと型引数付き参照を取る() {
        let src = "trait Map<T> { fn map<U>(self, f: fn(T -> U) -> [U]) }\n\
                   impl<T> Map<T> for [T] {\n\
                   \x20 fn map<U>(self, f: fn(T -> U) -> [U]) { self }\n\
                   }\n";
        let p = ok(src);
        let Item::Trait {
            type_params,
            methods,
            ..
        } = &p.items[0]
        else {
            panic!("trait ではない")
        };
        assert_eq!(param_spellings(src, type_params), ["T"]);
        assert_eq!(param_spellings(src, &methods[0].type_params), ["U"]);

        let Item::Impl {
            type_params,
            trait_ref,
            target,
            methods,
            ..
        } = &p.items[1]
        else {
            panic!("impl ではない")
        };
        assert_eq!(param_spellings(src, type_params), ["T"]);
        let trait_ref = trait_ref.as_ref().unwrap();
        assert_eq!(trait_ref.name, "Map");
        // 型引数も対象型も、型注釈の文法そのままで往復する
        assert_eq!(
            trait_ref
                .args
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["T"]
        );
        assert_eq!(target.to_string(), "[T]");
        assert_eq!(param_spellings(src, &methods[0].0.type_params), ["U"]);
    }

    /// 型パラメータの無い `impl` は今までどおり。対象型は `Named` の葉のまま
    #[test]
    fn 型パラメータの無いimplは従来の形で読める() {
        for (src, trait_, target_spelling) in [
            ("impl Postgres { fn f() { 1 } }\n", None, "Postgres"),
            (
                "impl data::Database for data::Postgres { fn f() { 1 } }\n",
                Some("data::Database"),
                "data::Postgres",
            ),
        ] {
            let p = ok(src);
            let Item::Impl {
                type_params,
                trait_ref,
                target,
                ..
            } = &p.items[0]
            else {
                panic!("impl ではない")
            };
            assert!(type_params.is_empty(), "{src}");
            assert_eq!(trait_ref.as_ref().map(|r| r.name.as_str()), trait_, "{src}");
            assert!(trait_ref.iter().all(|r| r.args.is_empty()), "{src}");
            assert_eq!(target.to_string(), target_spelling, "{src}");
            assert_eq!(target.name(), Some(target_spelling), "{src}");
        }
    }

    #[test]
    fn structとenumには型パラメータを書けない() {
        assert_eq!(
            error("struct Box<T> { value: T }\n"),
            "struct には型パラメータを書けません"
        );
        assert_eq!(
            error("enum Option<T> { Some(T) None }\n"),
            "enum には型パラメータを書けません"
        );
    }

    /// `for` の無い `impl` の対象は型注釈なので、型引数の置き場所が無い
    #[test]
    fn 型引数付きの対象型だけのimplは断る() {
        assert!(
            error("impl Map<T> { fn f() { 1 } }\n").contains("型引数は書けません"),
            "{}",
            error("impl Map<T> { fn f() { 1 } }\n")
        );
    }

    #[test]
    fn 閉じない型パラメータリストは断る() {
        assert!(error("fn f<T(x: T) { x }\n").contains("`>`"));
        assert!(error("fn f<>(x: int) { x }\n").contains("型パラメータ名"));
    }

    // --- IDX-010: 添字構文 ---

    /// 最初の関数本体の式を1つずつ取り出す
    fn stmts(src: &str) -> &'static [Expr] {
        // 借用の都合で Program を漏らす。テスト内だけの割り切り
        let p = Box::leak(Box::new(ok(src)));
        p.items
            .iter()
            .find_map(|item| match item {
                Item::Fn { body, .. } => Some(body.as_slice()),
                _ => None,
            })
            .expect("fn が無い")
    }

    /// 単一の式を関数本体に包んで綴る
    fn only(src: &str) -> String {
        first(&format!("fn f() {{\n  {src}\n}}\n"))
    }

    /// 添字は後置の一段。`.field` / `(args)` と同じ優先順位で左から積む
    #[test]
    fn 添字は後置式として読める() {
        for (src, spelling) in [
            ("xs[i]", "xs[i]"),
            ("xs[i][j]", "xs[i][j]"),
            ("xs.get(i)[0]", "xs.get(i)[Int(0)]"),
            ("xs[i].field", "xs[i].field"),
            ("xs[i].get(j)", "xs[i].get(j)"),
            ("xs[i + 1]", "xs[(i Add Int(1))]"),
            ("xs[f(y)]", "xs[f(y)]"),
            ("xs[ys[j]]", "xs[ys[j]]"),
            // 後置なので、外側の二項演算子や所有権修飾より内側で束縛する
            ("xs[i] + 1", "(xs[i] Add Int(1))"),
            ("&xs[i]", "(&xs[i])"),
        ] {
            assert_eq!(only(src), spelling, "{src}");
        }

        // 配列リテラルへの添字。リテラル側の Debug 綴りは span を含むので形で見る
        let ExprKind::Index(base, index) = &stmts("fn f() {\n  [1, 2, 3][0]\n}\n")[0].kind else {
            panic!("Index ではない")
        };
        assert!(
            matches!(&base.kind, ExprKind::Array(items) if items.len() == 3),
            "基底が配列リテラルではない: {:?}",
            base.kind
        );
        assert!(matches!(&index.kind, ExprKind::Int(0)), "{:?}", index.kind);
    }

    /// IDX-Q1: `len()` に専用構文は無い。`push` と同じメソッド呼び出しの産出
    #[test]
    fn lenは通常のメソッド呼び出しとして読める() {
        assert_eq!(only("xs.len()"), "xs.len()");
        assert_eq!(only("xs.push(y)"), "xs.push(y)");

        let ExprKind::Call(callee, args) = &stmts("fn f() {\n  xs.len()\n}\n")[0].kind else {
            panic!("Call ではない")
        };
        assert!(args.is_empty(), "`len()` は引数なしの呼び出し");
        assert!(
            matches!(&callee.kind, ExprKind::Field(recv, name)
                if matches!(&recv.kind, ExprKind::Ident(n) if n == "xs") && name == "len"),
            "callee が `Field(xs, len)` ではない: {:?}",
            callee.kind
        );
    }

    /// IDX-Q5: `xs[i] = v` は `user.id = id` と同じ代入の産出に載る
    #[test]
    fn 添字は代入の左辺になれる() {
        let body = stmts("fn f() {\n  xs[i] = v\n  user.id = id\n}\n");
        let spelled: Vec<(String, String)> = body
            .iter()
            .map(|e| {
                let ExprKind::Assign { target, value } = &e.kind else {
                    panic!("Assign ではない: {:?}", e.kind)
                };
                (show_expr(target), show_expr(value))
            })
            .collect();
        assert_eq!(
            spelled,
            [
                ("xs[i]".to_string(), "v".to_string()),
                ("user.id".to_string(), "id".to_string()),
            ]
        );
    }

    /// 代入の右辺・引数・文位置・局所束縛のどれでも、添字は同じ普通の式のまま
    #[test]
    fn 代入以外の位置の添字は普通の式() {
        let body = stmts("fn f() {\n  xs[i]\n  g(xs[i])\n  y = xs[i]\n  let v = xs[i]\n}\n");

        let indexed = |e: &Expr| {
            assert!(
                matches!(&e.kind, ExprKind::Index(base, index)
                    if matches!(&base.kind, ExprKind::Ident(n) if n == "xs")
                        && matches!(&index.kind, ExprKind::Ident(n) if n == "i")),
                "`xs[i]` の形ではない: {:?}",
                e.kind
            );
        };

        indexed(&body[0]);
        let ExprKind::Call(_, args) = &body[1].kind else {
            panic!("Call ではない: {:?}", body[1].kind)
        };
        indexed(&args[0]);
        let ExprKind::Assign { value, .. } = &body[2].kind else {
            panic!("Assign ではない: {:?}", body[2].kind)
        };
        indexed(value);
        let ExprKind::Let { value, .. } = &body[3].kind else {
            panic!("Let ではない: {:?}", body[3].kind)
        };
        indexed(value);
    }

    /// 壊れた添字はソース位置つきで断る
    #[test]
    fn 壊れた添字をソース位置つきで断る() {
        for (src, needle) in [
            ("fn f() {\n  xs[i\n}\n", "`]`"),
            ("fn f() {\n  xs[]\n}\n", "式"),
        ] {
            let diagnostic = parse_src(src).expect_err(src);
            assert!(diagnostic.msg.contains(needle), "{src}: {}", diagnostic.msg);
            let span = diagnostic
                .span
                .unwrap_or_else(|| panic!("span が無い: {src}"));
            // 添字を開いた `[` 以降を指す
            assert!(
                span.start >= 13 && span.end as usize <= src.len(),
                "{src}: {span:?}"
            );
        }
    }

    /// 添字を含まない既存の構文の読みは変わらない
    #[test]
    fn 添字以外の構文の読みは変わらない() {
        for (src, spelling) in [
            ("xs.push(y)", "xs.push(y)"),
            ("user.id", "user.id"),
            ("user.?rank", "user.?rank"),
            ("Rank::Gold", "Rank::Gold"),
            ("f(a, b)", "f(a, b)"),
        ] {
            assert_eq!(only(src), spelling, "{src}");
        }
        // 配列リテラル・struct リテラル・配列型注釈も従来どおり読める
        assert!(matches!(&stmts("fn f() {\n  [1, 2, 3]\n}\n")[0].kind,
                ExprKind::Array(items) if items.len() == 3));
        assert!(matches!(
            &stmts("struct Circle { r: int }\nfn f() {\n  Circle { r = 1 }\n}\n")[0].kind,
            ExprKind::StructLit { .. }
        ));
        ok("fn f(xs: [int] -> int) {\n  1\n}\n");
    }

    // --- CLO-010: 無名関数リテラル ---

    /// 引数・戻り値・本体の式数だけを綴る。名前付き関数の署名と同じ形なので
    /// `show_type` をそのまま使える
    fn spell_closure(e: &Expr) -> String {
        let ExprKind::Closure { params, ret, body } = &e.kind else {
            panic!("Closure ではない: {:?}", e.kind)
        };
        let params: Vec<String> = params
            .iter()
            .map(|p| format!("{}: {}", p.name, show_type(&p.ty)))
            .collect();
        let ret = match ret {
            Some(ty) => show_type(ty),
            None => "(省略)".to_string(),
        };
        format!("fn({} -> {}) {{{}式}}", params.join(", "), ret, body.len())
    }

    /// 引数ゼロ・複数引数・戻り値省略。文法は名前付き関数の署名と同じ産出
    #[test]
    fn 無名関数リテラルを式として読める() {
        let body = stmts(concat!(
            "fn f() {\n",
            "  fn(-> int) { 1 }\n",
            "  fn(a: int, b: int -> int) { a + b }\n",
            "  fn(x: int) { let ignored = x }\n",
            "  fn(u: &User, xs: [int]? -> User?) { u }\n",
            "}\n"
        ));
        assert_eq!(spell_closure(&body[0]), "fn( -> int) {1式}");
        assert_eq!(
            spell_closure(&body[1]),
            "fn(a: int, b: int -> int) {1式}",
            "引数は名前付き関数と同じ `name: Type` の並び"
        );
        // `->` の省略は推論ではない。注釈が無いという事実だけが残る
        assert_eq!(spell_closure(&body[2]), "fn(x: int -> (省略)) {1式}");
        assert_eq!(
            spell_closure(&body[3]),
            "fn(u: &User, xs: [int]? -> User?) {1式}",
            "型注釈は `ty()` の全産出をそのまま使える"
        );
    }

    /// 式なので `let` の初期化子にも呼び出し引数にも置ける
    #[test]
    fn 無名関数リテラルは式の置ける場所に置ける() {
        let body = stmts("fn f() {\n  let g = fn(x: int -> int) { x }\n}\n");
        let ExprKind::Let { name, value, .. } = &body[0].kind else {
            panic!("Let ではない: {:?}", body[0].kind)
        };
        assert_eq!(name, "g");
        assert_eq!(spell_closure(value), "fn(x: int -> int) {1式}");

        let body = stmts("fn f() {\n  apply(fn(x: int -> int) { x }, 1)\n}\n");
        let ExprKind::Call(callee, args) = &body[0].kind else {
            panic!("Call ではない: {:?}", body[0].kind)
        };
        assert_eq!(show_expr(callee), "apply");
        assert_eq!(args.len(), 2);
        assert_eq!(spell_closure(&args[0]), "fn(x: int -> int) {1式}");
    }

    /// CLO-Q3: closure の宣言型は名前付き関数値の `fn(P1, P2 -> R)` と同じ形。
    /// 別の「closure 型」は導入しない
    #[test]
    fn 無名関数の宣言型は名前付き関数値の型と一致する() {
        let p = ok(concat!(
            "fn add(a: int, b: int -> int) { a + b }\n",
            "fn f() {\n",
            "  let g: fn(int, int -> int) = add\n",
            "  fn(a: int, b: int -> int) { a + b }\n",
            "}\n"
        ));
        let named = p
            .items
            .iter()
            .find_map(|item| match item {
                Item::Fn { sig, .. } if sig.name == "add" => Some(sig),
                _ => None,
            })
            .expect("add がある");
        let body = p
            .items
            .iter()
            .find_map(|item| match item {
                Item::Fn { sig, body, .. } if sig.name == "f" => Some(body),
                _ => None,
            })
            .expect("f がある");

        let ExprKind::Let {
            annotation: Some(annotation),
            ..
        } = &body[0].kind
        else {
            panic!("注釈つき Let ではない: {:?}", body[0].kind)
        };
        let ExprKind::Closure { params, ret, .. } = &body[1].kind else {
            panic!("Closure ではない: {:?}", body[1].kind)
        };

        // closure の注釈から組んだ callable 型が、名前付き関数値の型注釈と等しい
        let from_closure = Type {
            mode: TypeMode::Owned,
            kind: TypeKind::Callable {
                params: params.iter().map(|p| p.ty.clone()).collect(),
                result: Box::new(ret.clone().expect("戻り値注釈がある")),
            },
            optional: false,
        };
        assert_eq!(&from_closure, annotation);

        // 引数・戻り値そのものも名前付き関数の署名と要素ごとに同じ
        let spell = |params: &[Param], ret: &Option<Type>| {
            let params: Vec<String> = params
                .iter()
                .map(|p| format!("{}: {}", p.name, show_type(&p.ty)))
                .collect();
            format!("{} -> {:?}", params.join(", "), ret.as_ref().map(show_type))
        };
        assert_eq!(spell(params, ret), spell(&named.params, &named.ret));
    }

    /// 壊れた無名関数リテラルは、名前付き関数の署名とまったく同じ診断で断る
    #[test]
    fn 壊れた無名関数リテラルをソース位置つきで断る() {
        for (src, needle) in [
            // 引数の型注釈欠落
            ("fn f() {\n  fn(x -> int) { x }\n}\n", "`:`"),
            // 閉じない引数リスト
            ("fn f() {\n  fn(x: int -> int { x }\n}\n", "`)`"),
            // 閉じない本体
            ("fn f() {\n  fn(x: int -> int) { x\n}\n", "`}`"),
            // 本体そのものが無い
            ("fn f() {\n  fn(x: int -> int)\n}\n", "`{`"),
        ] {
            let diagnostic = parse_src(src).expect_err(src);
            assert!(diagnostic.msg.contains(needle), "{src}: {}", diagnostic.msg);
            let span = diagnostic
                .span
                .unwrap_or_else(|| panic!("span が無い: {src}"));
            assert!(
                span.start as usize >= 11 && span.end as usize <= src.len(),
                "{src}: {span:?}"
            );
        }

        // 同じ欠落には名前付き関数と同じ文言が出る(産出を共有しているため)
        assert_eq!(
            error("fn f() {\n  fn(x -> int) { x }\n}\n"),
            error("fn g(x -> int) { x }\n"),
        );
    }

    /// 既存の `fn` の3つの位置は読みが変わらない
    #[test]
    fn 無名関数リテラルは既存のfnの読みを変えない() {
        // 宣言位置 — 名前が続くので従来どおり item
        let p = ok("fn add(a: int, b: int -> int) { a + b }\n");
        let Item::Fn { sig, .. } = &p.items[0] else {
            panic!("fn ではない")
        };
        assert_eq!(sig.name, "add");
        assert_eq!(sig.params.len(), 2);

        // 型注釈位置 — 引数に名前を書かない `Callable` のまま
        let body = stmts("fn f() {\n  let g: fn(int, int -> int) = add\n}\n");
        let ExprKind::Let {
            annotation: Some(annotation),
            ..
        } = &body[0].kind
        else {
            panic!("注釈つき Let ではない")
        };
        assert_eq!(show_type(annotation), "fn(int, int -> int)");
        assert!(matches!(annotation.kind, TypeKind::Callable { .. }));
        // 型注釈位置では `->` は今までどおり必須
        assert!(error("fn f(g: fn(int) -> int) { 1 }\n").contains("`->`"));

        // 裸のブロックは第二級のまま。`fn` が付かない限り Closure にはならない
        assert!(matches!(
            &stmts("fn f() {\n  { 1 }\n}\n")[0].kind,
            ExprKind::Block(items) if items.len() == 1
        ));
    }
}
