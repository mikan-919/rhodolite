## Why

fieldless enum は有限個の値を一つの型として表せるが、現在は等値比較と `if` を重ねる以外に分岐できず、variant の追加漏れも検出できない。また裸の variant 名しか使えないため、複数の enum が同名 variant を持つと宣言名前空間で衝突する。既存 roadmap が次の段として enum の拡張を挙げている今、payload まで広げずにこの二つを同じ狭い縦切りで解決する。

## What Changes

- `Enum::Variant` 形式で fieldless enum variant を限定参照できるようにする。
- fieldless enum 値を variant ごとに分岐し、各 arm の値を式全体の値にできる `match` 式を追加する。
- 対象 enum の全 variant を一度ずつ扱うことを静的に要求し、欠落・重複・別 enum の variant を診断する。
- 各 arm の結果型が分かる場合は既存の型適合規則で統一し、`match` の結果型を後続の引数・代入・戻り値検査へ流す。
- 裸の variant 参照は互換性のため維持する。
- データ付き variant、束縛を伴うパターン、ワイルドカード、guard、enum メソッドは追加しない。

## Capabilities

### New Capabilities

- `fieldless-enum-matching`: fieldless enum に対する値ベースの分岐、網羅性・重複検査、arm 結果型の規則。

### Modified Capabilities

- `fieldless-enums`: 既存の裸の variant 値に加え、enum 名で限定した `Enum::Variant` 値を名前解決・評価・型検査できるようにする。

## Impact

lexer、AST、parser、モジュール名前解決、型検査、要求走査、evaluator、CLI 診断と各層のテストが影響を受ける。`docs/grammar.md` と `docs/overview.md` に新しい構文と保証範囲を反映する。外部依存関係、既存の enum 宣言、裸の variant 参照、ambient の意味論は変更しない。
