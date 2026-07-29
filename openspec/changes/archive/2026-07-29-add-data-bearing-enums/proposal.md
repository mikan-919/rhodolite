## Why

fieldless enum と網羅的な `match` までは動くが、成功値と失敗理由のような「場合ごとに異なるデータ」を一つの型で表せず、struct と enum を別々に組み合わせる必要がある。`docs/overview.md` が次の最優先として挙げる穴を、guard や一般的な pattern 言語まで広げず、payload の構築と取り出しが往復する最小の縦切りで埋める。

## What Changes

- enum variant に0個以上の型付き positional payload を宣言できるようにする。
- payload を持つ variant を `Enum::Variant(...)` で構築し、個数と各値の型を実行前に検査する。
- `match` arm で payload を同じ個数の名前へ分解し、arm 本体だけで使える型付き束縛を導入する。
- `_` で個々の payload を束縛せずに捨てられるようにする。
- fieldless variant、限定 variant 値、既存の「variant ごとに過不足なく一つの arm」という網羅性規則を維持する。
- guard、variant 全体を覆う catch-all pattern、入れ子 pattern、名前付き payload、enum メソッドは追加しない。

## Capabilities

### New Capabilities

- `data-bearing-enums`: 型付き payload を持つ enum variant の宣言・構築・静的検査・評価と、網羅的な `match` による payload 束縛を定める。

### Modified Capabilities

- `fieldless-enum-matching`: match arm が payload pattern を持つ場合の局所束縛と、既存の lexical / ambient analysis に対する振る舞いを追加する。

## Impact

lexer、AST、parser、モジュール名前解決、型検査、要求走査、evaluator と各層のテストが影響を受ける。`docs/grammar.md`、`docs/overview.md`、`README.md` に新しい構文と残る境界を反映する。外部依存関係、ambient の意味論、fieldless enum を使う既存ソースは変更しない。
