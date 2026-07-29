//! 実行前の診断。
//!
//! `lex` / `parse` / `module` / `typecheck` / `requirement` はすべてこの1つの
//! 型で診断を返す。診断ごとの variant を持つ enum にしないのは、目的が
//! 「位置を出すこと」だからで、42 種の variant 定義は目的に対して過剰
//! (design.md 決定3)。エラーコードや `--explain` が要るときに分解すればよい。
//!
//! miette はここでは使わない。描画は CLI 境界の1関数に閉じる(決定4)。

use crate::lex::Span;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diag {
    /// 文言。span の有無に関わらずこれだけは必ず出る
    pub msg: String,
    /// 主原因の位置。指せるソースが無い診断だけ `None`
    pub span: Option<Span>,
    /// 主 span に添える短い語
    pub label: Option<String>,
    /// 直し方、あるいは到達経路の1行表現
    pub help: Option<String>,
    /// 別の位置(別ファイルもありうる)を指す従属診断
    pub related: Vec<Diag>,
}

impl Diag {
    /// ソース位置を持たない診断。読み込みがソースに届く前に失敗した場合に使う。
    pub fn msg(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            span: None,
            label: None,
            help: None,
            related: Vec::new(),
        }
    }

    pub fn at(span: Span, msg: impl Into<String>) -> Self {
        Self {
            span: Some(span),
            ..Self::msg(msg)
        }
    }

    /// 位置を持つかどうかが呼び出し側の状況で決まるとき用。
    pub fn from_span(span: Option<Span>, msg: impl Into<String>) -> Self {
        Self {
            span,
            ..Self::msg(msg)
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn related(mut self, related: Vec<Diag>) -> Self {
        self.related = related;
        self
    }
}

/// span を落とした素のテキスト。文言だけを見る呼び出し(テストの部分一致など)は
/// これで従来どおり読める。
impl std::fmt::Display for Diag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}
