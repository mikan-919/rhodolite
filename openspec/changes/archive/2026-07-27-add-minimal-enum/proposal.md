## Why

正典プログラムは `Rank` の値を表すためにフィールド0個の `struct Bronze {}` と
`struct Gold {}` を名札として代用しているが、`Bronze` / `Gold` が `Rank` に属することを
言語は表現できない。フィールド値の型検査へ進む前に、有限個の名前付き値を一つの型として
宣言できる最小限の enum を導入する。

## What Changes

- `enum Rank { Bronze Gold }` 形式の、データを持たない enum 宣言を追加する。
- enum variant を裸の値として参照し、比較・struct フィールドへの格納・代入に使えるようにする。
- variant が宣言元 enum の値であることを実行前に検査し、異なる enum の variant を
  enum 型のフィールドへ与えた場合は診断する。
- variant 宣言と参照を既存のモジュール読み込み・正準名・字句シャドーイング規則へ統合する。
- 正典プログラムの `Bronze` / `Gold` 用ゼロフィールド struct を `Rank` enum へ置き換える。
- データ付き variant、`match`、enum 固有のメソッド、網羅性検査は追加しない。

## Capabilities

### New Capabilities

- `fieldless-enums`: データを持たない enum の宣言、variant の名前解決・評価、および
  enum 型としての最小限の静的検査を規定する。

### Modified Capabilities

なし。

## Impact

- lexer、AST、parser に enum 宣言と variant の表現が加わる。
- モジュール解決は enum と variant の宣言・参照を正準名へ解決する。
- `typecheck` は enum variant の所属を追跡し、enum 型が明記された struct
  フィールドの生成値と代入値を検査する最初のフィールド値検査を担う。
- evaluator は enum variant を固有の値として生成し、既存の等値比較で扱う。
- 正典、文法、全体像、および関連する単体・CLI 結合テストを更新する。
- 外部依存関係は追加しない。
