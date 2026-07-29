## Why

実行前に出る診断はすべてただの `String` で、ソース上のどこが原因かを一切示せない。`lex::Span` と AST の `span` は既にあるのに、診断を組み立てる時点で捨てている。README が売りに掲げる「提供忘れを到達経路付きで報告」も、いまは関数名の連なり (`clock ← stamp ← promote ← main`) だけで、経路のどの呼び出しが原因かをソースで指せない。型検査の網が広がりきった今、次に品質を決めるのは検査の数ではなく診断の見え方である。

## What Changes

- `Span` にソース識別子を持たせ、複数モジュールを1つの `Program` に畳んだ後も、どのファイルのバイト範囲かを診断から一意に引けるようにする。
- モジュール読み込みが各モジュールのソース本文とパスを保持し、診断のレンダリングまで届ける。
- 実行前の診断 (`lex` / `parse` / `module` / `typecheck` / `requirement`) を、メッセージ・主 span・任意のラベル・任意の help を持つ診断値に統一する。文言そのものは変えない。
- `miette` を依存に追加し、CLI 境界でソース抜粋・下線付きに描画する。
- 要求解析の呼び出し辺に span を持たせ、提供忘れの到達経路の各ホップを、その呼び出し地点のラベルとして示す。
- `match` の網羅性診断が、欠落・重複・別 enum の arm を `match` 式と該当 arm の位置で指す。
- **BREAKING**(内部): `lex::lex` / `typecheck::check` / `module::load` / `Analysis::errors_for` の戻り値型が `String` から診断値に変わる。言語仕様と CLI の終了コードは変えない。
- 実行時エラー (`eval`) は `String` のまま据え置く。

## Capabilities

### New Capabilities

- `diagnostic-spans`: 実行前の診断がソース位置を持ち、モジュールを跨いでも原因箇所とその抜粋を示せること。到達経路を呼び出し地点のラベルとして示す規則を含む。

### Modified Capabilities

- `fieldless-enum-matching`: 網羅性・重複・別 enum の arm の診断が、名前だけでなく `match` 式と arm の位置を指すようになる。

## Impact

`src/lex.rs`(Span とエラー)、`src/parse.rs`(`ParseError`)、`src/module.rs`(ソース保持と診断)、`src/typecheck.rs`・`src/requirement.rs`(診断の組み立てと呼び出し辺の span)、`src/main.rs`(描画)、`tests/cli.rs` と各モジュールのテストが影響を受ける。依存に `miette` が1つ増える(ADR-0004 の申告事項として承認済み)。`docs/overview.md` と `docs/requirement-map.md` に診断の形を反映する。言語の構文・意味論・型規則・要求推論の結果は変えない。`src/eval.rs` の実行時エラーは変更しない。
