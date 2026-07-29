//! 診断の描画。miette を知るのはこのファイルだけ(design.md 決定4)。
//!
//! `Diag` は素の構造体のままにしておき、ソース本文と組んで miette の
//! `Report` へ変換するのをここに閉じ込める。依存を外すか差し替える判断が
//! この1ファイルで済む。
//!
//! span を持たない診断は miette を通さず、従来どおりの素のテキストで出す。

use crate::diag::Diag;
use crate::module::SourceFile;
use miette::{Diagnostic, LabeledSpan, NamedSource, Report, SourceCode};

/// 折り返しを止める。診断の文言と到達経路の1行表現は、端末幅で折られると
/// 1行として読めなくなる(既存のテストも README も1行を前提にしている)。
fn install_handler() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = miette::set_hook(Box::new(|_| {
            Box::new(
                miette::MietteHandlerOpts::new()
                    // 到達経路のホップは主診断の下に入れ子で並べる
                    .show_related_errors_as_nested()
                    .wrap_lines(false)
                    .build(),
            )
        }));
    });
}

/// 診断を1件ずつ標準エラーへ書く。
pub fn report(diagnostics: &[Diag], sources: &[SourceFile]) {
    install_handler();
    for diagnostic in diagnostics {
        match rendered(diagnostic, sources) {
            Some(rendered) => eprintln!("{:?}", Report::new(rendered)),
            None => eprintln!("{diagnostic}"),
        }
    }
}

/// `Diag` と、その span が指すファイルの組。関連診断は自分のファイルを持つので、
/// 経路がモジュールを跨いでも各ホップが正しい抜粋の上に描かれる。
struct Rendered {
    msg: String,
    label: LabeledSpan,
    help: Option<String>,
    source: NamedSource<String>,
    related: Vec<Rendered>,
}

/// span を持ち、その src が読み込み済みのソースを指しているものだけ描ける。
fn rendered(diagnostic: &Diag, sources: &[SourceFile]) -> Option<Rendered> {
    let span = diagnostic.span?;
    let file = sources.get(span.src as usize)?;
    Some(Rendered {
        msg: diagnostic.msg.clone(),
        label: LabeledSpan::new(
            diagnostic.label.clone(),
            span.start as usize,
            span.end.saturating_sub(span.start) as usize,
        ),
        help: diagnostic.help.clone(),
        source: NamedSource::new(file.path.display().to_string(), file.text.clone()),
        related: diagnostic
            .related
            .iter()
            .filter_map(|related| rendered(related, sources))
            .collect(),
    })
}

impl std::fmt::Display for Rendered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::fmt::Debug for Rendered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl std::error::Error for Rendered {}

impl Diagnostic for Rendered {
    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(self.label.clone())))
    }

    fn help(&self) -> Option<Box<dyn std::fmt::Display + '_>> {
        self.help
            .as_ref()
            .map(|help| Box::new(help) as Box<dyn std::fmt::Display>)
    }

    fn related(&self) -> Option<Box<dyn Iterator<Item = &dyn Diagnostic> + '_>> {
        if self.related.is_empty() {
            return None;
        }
        Some(Box::new(self.related.iter().map(|r| r as &dyn Diagnostic)))
    }
}
