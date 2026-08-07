//! 字句解析と、行継続規則の適用。
//!
//! 2段構成:
//!   1. `lex`  … 文字列 → トークン列。改行も `Newline` として残す
//!   2. `join` … 行継続規則を適用して「継続する改行」を落とす
//!
//! 2段目を独立させているのは、行継続が「前後のトークンだけで決まる」局所的な規則で、
//! 構文木を知る必要がないため。パーサに混ぜると全産出に「ここで改行は許すか」が
//! 滲み出すが、独立パスなら規則が `can_end_expr` / `can_start_expr` の2関数に閉じる。

use crate::diag::Diag;

/// 読み込み済みソースの識別子。`module::LoadedProgram::sources` の添字。
pub type SourceId = u32;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Span {
    /// このバイト範囲がどのソースのものか。複数モジュールを1つの `Program` に
    /// 畳んだ後でも、span 単体から元のファイルを引けるようにするために持つ
    pub src: SourceId,
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
    Match,
    Trait,
    Struct,
    Enum,
    Impl,
    SelfKw,
    Effect,
    Test,
    Use,
    /// `pub use` の接頭辞。`use` と同じく、トップレベル接頭部でだけ
    /// キーワードとして働き、それ以外の位置では識別子に戻る
    Pub,
    As,
    /// `&mut T` / `let mut x` の可変修飾。単独では識別子に戻る
    Mut,
    /// `move place` の所有権移動修飾。場所が続くときだけ修飾として働く
    Move,
    /// `indirect next: Node?` — 再帰型の層を切る所有エッジ。
    /// 宣言位置の外では識別子に戻る
    Indirect,
    /// 予約語。この版に unsafe は無いので、識別子には戻さず拒否する
    Unsafe,
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
    Less,
    Greater,
    Eq,       // 束縛
    EqEq,     // 比較
    Arrow,    // -> 戻り値
    Question, // 後置 ? = Option 型
    Coalesce, // ??
    Plus,
    Minus,
    Star,
    Slash,
    Amp, // & 参照

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

/// ソース識別子を持たない入口。識別子 0 を刻む。実際に読み込むのは
/// `module` だけなので、これを使うのは字句解析器とパーサのテストに限る。
#[cfg(test)]
pub fn lex(src: &str) -> Result<Vec<Token>, Diag> {
    lex_source(src, 0)
}

pub fn lex_source(src: &str, source: SourceId) -> Result<Vec<Token>, Diag> {
    let b = src.as_bytes();
    let span = |start: usize, end: usize| Span {
        src: source,
        start: start as u32,
        end: end as u32,
    };
    let tok = |t, start: usize, end: usize| Token {
        tok: t,
        span: span(start, end),
        line: 1,
    };
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
            let n: i64 = src[start..i].parse().map_err(|_| {
                Diag::at(
                    span(start, i),
                    format!("整数が大きすぎます: {}", &src[start..i]),
                )
            })?;
            out.push(tok(Tok::Int(n), start, i));
            continue;
        }

        // 文字列。エスケープは v1 では扱わない(正典に出てこない)
        if c == b'"' {
            i += 1;
            let text_start = i;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\n' {
                    return Err(Diag::at(span(start, i), "文字列が閉じられていません"));
                }
                i += 1;
            }
            if i >= b.len() {
                return Err(Diag::at(span(start, i), "文字列が閉じられていません"));
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
            b'<' => Tok::Less,
            b'>' => Tok::Greater,
            b'=' => Tok::Eq,
            b'?' => Tok::Question,
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'&' => Tok::Amp,
            // ライフタイム引数はソース言語に無い。`'` が出る位置は他に無いので
            // ここで名指しで断る(ownership-and-borrowing)
            b'\'' => {
                return Err(Diag::at(
                    span(start, i + 1),
                    "ライフタイム注釈は書けません。参照は `&T` / `&mut T` と書きます",
                ));
            }
            _ => {
                return Err(Diag::at(
                    span(start, i + 1),
                    format!("読めない文字です: {:?}", c as char),
                ));
            }
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
        "match" => Tok::Match,
        "trait" => Tok::Trait,
        "struct" => Tok::Struct,
        "enum" => Tok::Enum,
        "impl" => Tok::Impl,
        "self" => Tok::SelfKw,
        "effect" => Tok::Effect,
        "test" => Tok::Test,
        "use" => Tok::Use,
        "pub" => Tok::Pub,
        "as" => Tok::As,
        "mut" => Tok::Mut,
        "move" => Tok::Move,
        "indirect" => Tok::Indirect,
        "unsafe" => Tok::Unsafe,
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
///
/// `mut` / `move` / `indirect` は識別子にも戻るので、識別子と同じ扱いを保つ。
/// 修飾として使うときは同じ行に場所が続くため、行末に立つのは識別子用法だけ。
/// ponytail: この選択で `move` 改行 `place` は継続しなくなる。修飾を複数行に
/// 跨げるようにするなら、行継続ではなくパーサ側で改行を跨ぐ規則が要る
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
            | Tok::Mut
            | Tok::Move
            | Tok::Indirect
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
            | Tok::Match
            | Tok::Trait
            | Tok::Struct
            | Tok::Enum
            | Tok::Impl
            | Tok::Effect
            | Tok::Test
            | Tok::Use
            | Tok::Pub
            | Tok::As
            // 所有権修飾は場所の手前に立つので、行頭に来たら式の始まり。
            // `mut` / `indirect` は識別子にも戻るので、識別子と同じ扱いを保つ
            | Tok::Amp
            | Tok::Move
            | Tok::Mut
            | Tok::Indirect
            | Tok::Unsafe
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

    /// `#` は識別子の文字ではない。剛体検査が型パラメータに使う合成名
    /// `#T<index>` がユーザーの書ける名前ともモジュール修飾名とも衝突しないのは
    /// この1点に乗っている(MAP-020 決定1)
    #[test]
    fn 番号記号は識別子にならない() {
        for src in ["#T0\n", "a#b\n", "#\n"] {
            let error = lex(src).expect_err(src);
            assert!(error.msg.contains("読めない文字です"), "{src}: {error:?}");
        }
    }

    #[test]
    fn 行末が式を終えられないなら継続する() {
        assert_eq!(newlines("let a = 1 +\n2\n"), 1);
    }

    #[test]
    fn 行頭が式を始められないなら継続する() {
        assert_eq!(newlines("let a = 1\n+ 2\n"), 1);
        assert_eq!(newlines("users.filter(f)\n.map(g)\n"), 1);
        assert_eq!(newlines("if c { a() }\nelse: b()\n"), 1);
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
    fn enumはキーワードで宣言の行頭に立てる() {
        assert_eq!(
            toks("enum Rank { Bronze Gold }"),
            vec![
                Tok::Enum,
                Tok::Ident("Rank".into()),
                Tok::LBrace,
                Tok::Ident("Bronze".into()),
                Tok::Ident("Gold".into()),
                Tok::RBrace,
                Tok::Eof
            ]
        );
        // 行頭に立てる = 直前の改行が文の区切りとして残る
        assert_eq!(newlines("a()\nenum Rank {}\n"), 2);
    }

    #[test]
    fn matchはキーワードで行頭に立てる() {
        assert_eq!(
            toks("match r { Rank::Gold: 1 }"),
            vec![
                Tok::Match,
                Tok::Ident("r".into()),
                Tok::LBrace,
                Tok::Ident("Rank".into()),
                Tok::ColonColon,
                Tok::Ident("Gold".into()),
                Tok::Colon,
                Tok::Int(1),
                Tok::RBrace,
                Tok::Eof
            ]
        );
        // 行頭に立てる = 直前の改行が文の区切りとして残る
        assert_eq!(newlines("a()\nmatch r { }\n"), 2);
        // arm どうしは改行で切れる
        assert_eq!(
            newlines("match r {\nRank::Bronze: 1\nRank::Gold: 2\n}\n"),
            3
        );
    }

    /// 所有権の綴りはトークンになり、`&` は読める文字になる
    #[test]
    fn 所有権の綴りをトークンにする() {
        assert_eq!(
            toks("&mut move indirect"),
            vec![Tok::Amp, Tok::Mut, Tok::Move, Tok::Indirect, Tok::Eof]
        );
        // 位置は1文字ずつ正確に刻む
        let t = lex("a & b").unwrap();
        assert_eq!(t[1].tok, Tok::Amp);
        assert_eq!((t[1].span.start, t[1].span.end), (2, 3));
    }

    /// `mut` / `indirect` は識別子にも戻るので、行の始まりの扱いを変えない
    #[test]
    fn 所有権の綴りは行頭に立てる() {
        for src in [
            "a()\nmut()\n",
            "a()\nmove x\n",
            "a()\nindirect y\n",
            "a()\n&x\n",
        ] {
            assert_eq!(newlines(src), 2, "{src}");
        }
    }

    /// 識別子として行末にも立てる。ここを落とすと次の行が繋がってしまう
    #[test]
    fn 所有権の綴りは行末に立てる() {
        for src in [
            "let x = mut\nx\n",
            "let x = move\nx\n",
            "let x = indirect\nx\n",
        ] {
            assert_eq!(newlines(src), 2, "{src}");
        }
    }

    #[test]
    fn ライフタイムの引用符を名指しで断る() {
        let e = lex("&'a User").unwrap_err();
        assert!(e.msg.contains("ライフタイム"), "{}", e.msg);
        assert_eq!(e.span.map(|s| (s.start, s.end)), Some((1, 2)));
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
