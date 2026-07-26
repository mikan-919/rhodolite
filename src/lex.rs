//! 字句解析と、行継続規則の適用。
//!
//! 2段構成:
//!   1. `lex`  … 文字列 → トークン列。改行も `Newline` として残す
//!   2. `join` … 行継続規則を適用して「継続する改行」を落とす
//!
//! 2段目を独立させているのは、行継続が「前後のトークンだけで決まる」局所的な規則で、
//! 構文木を知る必要がないため。パーサに混ぜると全産出に「ここで改行は許すか」が
//! 滲み出すが、独立パスなら規則が `can_end_expr` / `can_start_expr` の2関数に閉じる。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Tok {
    // リテラルと識別子
    Ident(String),
    Int(i64),
    Str(String),

    // キーワード
    Fn,
    Let,
    If,
    Elif,
    Else,
    For,
    In,
    While,
    With,
    Trait,
    Struct,
    Impl,
    SelfKw,
    Effect,
    Test,
    Return,
    Assert,
    True,
    False,
    Nil,

    // 記号
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Colon,      // Head の区切り。意味はこれ一つだけ
    ColonColon, // パス
    Comma,
    Dot,
    Eq,       // 束縛
    EqEq,     // 比較
    Arrow,    // -> 戻り値
    Question, // 後置 ? = Option 型
    Coalesce, // ??
    Plus,
    Minus,
    Star,
    Slash,

    Newline,
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// 1始まりの行番号。`Head ':' 単純式` の同一物理行制約の検査に使う。
    /// 行継続で改行トークンが消えても、この値は元の位置を覚えている。
    pub line: u32,
}

pub fn lex(src: &str) -> Result<Vec<Token>, String> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut out: Vec<Token> = Vec::new();

    while i < b.len() {
        let start = i;
        let c = b[i];

        // 行内の空白。改行は下でトークンにするので飛ばさない
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }

        // 行コメント。末尾の改行自体は残す(文の区切りとして意味がある)
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        if c == b'\n' {
            i += 1;
            out.push(tok(Tok::Newline, start, i));
            continue;
        }

        // 識別子とキーワード
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            out.push(tok(keyword_or_ident(word), start, i));
            continue;
        }

        // 整数
        if c.is_ascii_digit() {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let n: i64 = src[start..i]
                .parse()
                .map_err(|_| format!("整数が大きすぎます: {}", &src[start..i]))?;
            out.push(tok(Tok::Int(n), start, i));
            continue;
        }

        // 文字列。エスケープは v1 では扱わない(正典に出てこない)
        if c == b'"' {
            i += 1;
            let text_start = i;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\n' {
                    return Err("文字列が閉じられていません".to_string());
                }
                i += 1;
            }
            if i >= b.len() {
                return Err("文字列が閉じられていません".to_string());
            }
            let text = src[text_start..i].to_string();
            i += 1; // 閉じ引用符
            out.push(tok(Tok::Str(text), start, i));
            continue;
        }

        // 記号。2文字のものを先に見る
        let two = if i + 1 < b.len() {
            Some(&src[i..i + 2])
        } else {
            None
        };
        let t = match two {
            Some("::") => Some(Tok::ColonColon),
            Some("==") => Some(Tok::EqEq),
            Some("->") => Some(Tok::Arrow),
            Some("??") => Some(Tok::Coalesce),
            _ => None,
        };
        if let Some(t) = t {
            i += 2;
            out.push(tok(t, start, i));
            continue;
        }

        let t = match c {
            b'{' => Tok::LBrace,
            b'}' => Tok::RBrace,
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'[' => Tok::LBracket,
            b']' => Tok::RBracket,
            b':' => Tok::Colon,
            b',' => Tok::Comma,
            b'.' => Tok::Dot,
            b'=' => Tok::Eq,
            b'?' => Tok::Question,
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            _ => return Err(format!("読めない文字です: {:?}", c as char)),
        };
        i += 1;
        out.push(tok(t, start, i));
    }

    out.push(tok(Tok::Eof, b.len(), b.len()));

    // 行番号は最後にまとめて振る。トークン生成側を汚さずに済み、
    // span (バイト位置) が正なので二重管理にならない。
    let mut line = 1u32;
    let mut cursor = 0usize;
    for t in &mut out {
        while cursor < t.span.start as usize {
            if b[cursor] == b'\n' {
                line += 1;
            }
            cursor += 1;
        }
        t.line = line;
    }

    Ok(out)
}

fn tok(t: Tok, start: usize, end: usize) -> Token {
    Token {
        tok: t,
        span: Span {
            start: start as u32,
            end: end as u32,
        },
        line: 1,
    }
}

fn keyword_or_ident(w: &str) -> Tok {
    match w {
        "fn" => Tok::Fn,
        "let" => Tok::Let,
        "if" => Tok::If,
        "elif" => Tok::Elif,
        "else" => Tok::Else,
        "for" => Tok::For,
        "in" => Tok::In,
        "while" => Tok::While,
        "with" => Tok::With,
        "trait" => Tok::Trait,
        "struct" => Tok::Struct,
        "impl" => Tok::Impl,
        "self" => Tok::SelfKw,
        "effect" => Tok::Effect,
        "test" => Tok::Test,
        "return" => Tok::Return,
        "assert" => Tok::Assert,
        "true" => Tok::True,
        "false" => Tok::False,
        "nil" => Tok::Nil,
        _ => Tok::Ident(w.to_string()),
    }
}

// ---------------------------------------------------------------------------
// 行継続
// ---------------------------------------------------------------------------

/// 行継続規則を適用して、継続する改行を落とす。
///
/// > 行末が式を終えられない、または次行の行頭が式を始められないなら、継続。
///
/// 特例リストは持たない。`.` のメソッド連鎖も `else`/`elif` の改行も
/// 二項演算子の前置き後置きも、この1本から出てくる。
///
/// 残った `Newline` が本物の文区切りになる。連続する改行は1つに畳み、
/// 先頭と `{` 直後の改行は落とす(空行を書けるようにするため)。
pub fn join(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());

    for t in tokens {
        if t.tok != Tok::Newline {
            out.push(t);
            continue;
        }

        // 直前の意味のあるトークン。無ければ(＝ソース先頭)改行は捨てる
        let Some(prev) = out.last() else { continue };

        // 連続する改行は1つに畳む
        if prev.tok == Tok::Newline {
            continue;
        }
        // ブロックを開いた直後の改行は区切りではない
        if prev.tok == Tok::LBrace {
            continue;
        }
        if !can_end_expr(&prev.tok) {
            continue; // 行末が式を終えられない → 継続
        }
        out.push(t);
    }

    // 次行の行頭が式を始められないなら、その手前の改行を落とす。
    // 前向きの判定なので、後ろから見て決める。
    let mut result: Vec<Token> = Vec::with_capacity(out.len());
    for (idx, t) in out.iter().enumerate() {
        if t.tok == Tok::Newline {
            match out.get(idx + 1) {
                Some(next) if !can_start_expr(&next.tok) => continue, // 継続
                None => continue,
                _ => {}
            }
        }
        result.push(t.clone());
    }
    result
}

/// このトークンで式を終えられるか(＝行末に来たとき文が完結しうるか)。
fn can_end_expr(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Ident(_)
            | Tok::SelfKw
            | Tok::Int(_)
            | Tok::Str(_)
            | Tok::True
            | Tok::False
            | Tok::Nil
            | Tok::RParen
            | Tok::RBrace
            | Tok::RBracket
            | Tok::Question // `User?` の後置
            | Tok::Return // 値なし return
    )
}

/// このトークンで式を始められるか(＝行頭に来たとき新しい文が始まりうるか)。
///
/// `Minus` を「始められる」側に置いているのは単項マイナスがあるため。
/// 結果として `a` 改行 `- b` は2つの式に切れる(継続しない)。意図と違う場合は
/// 「値が捨てられている」の警告で拾う方針(docs/grammar.md)。
fn can_start_expr(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Ident(_)
            | Tok::SelfKw
            | Tok::Int(_)
            | Tok::Str(_)
            | Tok::True
            | Tok::False
            | Tok::Nil
            | Tok::LParen
            | Tok::LBrace
            | Tok::LBracket
            | Tok::Minus
            | Tok::Fn
            | Tok::Let
            | Tok::If
            | Tok::For
            | Tok::While
            | Tok::With
            | Tok::Trait
            | Tok::Struct
            | Tok::Impl
            | Tok::Effect
            | Tok::Test
            | Tok::Return
            | Tok::Assert
            | Tok::RBrace // ブロックの終わりは「次の文」ではないが改行は残したい
            | Tok::Eof
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        join(lex(src).unwrap()).into_iter().map(|t| t.tok).collect()
    }

    fn newlines(src: &str) -> usize {
        toks(src).iter().filter(|t| **t == Tok::Newline).count()
    }

    #[test]
    fn 行末が式を終えられないなら継続する() {
        assert_eq!(newlines("let a = 1 +\n2\n"), 1);
    }

    #[test]
    fn 行頭が式を始められないなら継続する() {
        assert_eq!(newlines("let a = 1\n+ 2\n"), 1);
        assert_eq!(newlines("users.filter(f)\n.map(g)\n"), 1);
        assert_eq!(newlines("if c: { a() }\nelse: b()\n"), 1);
    }

    #[test]
    fn 両方満たすなら切れる() {
        assert_eq!(newlines("a()\nb()\n"), 2);
        // 単項マイナスがあるため `-` は式を始められる = 継続しない
        assert_eq!(newlines("a()\n-1\n"), 2);
    }

    #[test]
    fn 空行とブロック直後の改行は落ちる() {
        assert_eq!(newlines("a()\n\n\nb()\n"), 2);
        // `{` 直後の改行は落ちるので、残る区切りは `a()` の後の1つだけ
        assert_eq!(newlines("{\na()\n}"), 1);
    }

    #[test]
    fn コロンは一義でパスと区別される() {
        assert_eq!(
            toks("db: x"),
            vec![
                Tok::Ident("db".into()),
                Tok::Colon,
                Tok::Ident("x".into()),
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("Db::x"),
            vec![
                Tok::Ident("Db".into()),
                Tok::ColonColon,
                Tok::Ident("x".into()),
                Tok::Eof
            ]
        );
    }
}
