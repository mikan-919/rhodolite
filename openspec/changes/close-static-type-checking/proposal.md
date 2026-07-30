## Why

現在の型検査は、式の型や呼び出し先を推論できない場合に診断を保留し、評価器の動的検査へ委ねる。この境界のままでは型検査済みプログラムを型付き HIR や C バックエンドへ安全に渡せないため、検査成功後に未知の型・未解決の呼び出しを残さない契約へ移行する。

## What Changes

- すべての式が既知の型、または制御を脱出する内部型を持つことを型検査成功の条件にする。
- すべての直接呼び出し・メソッド呼び出し・関連関数呼び出しを検査時に一意の宣言へ解決する。
- 戻り値型を省略した関数とメソッドを `unit` 戻りとして扱う。
- `nil` や空配列など初期化子だけでは型が決まらない局所束縛のため、明示的な局所型注釈を導入する。
- 型を確定できないフィールドアクセス、演算、比較、条件、代入、配列、optional、match、呼び出し、`with` 提供を実行前に診断する。
- 型検査に成功したプログラムでは、後続段が未知の式型や未解決の呼び出しに遭遇しない不変条件を確立する。
- **BREAKING**: 従来は型検査を通過して実行時に判定されていた型不明プログラムの一部が、実行前エラーになる。
- **BREAKING**: 戻り値型を省略しながら `unit` 以外の値を返す関数・メソッドはエラーになる。

## Capabilities

### New Capabilities

- `total-static-type-checking`: 型検査成功後に未知の式型と未解決の参照を残さず、後続段へ検査結果を引き渡す全体契約。
- `local-binding-type-annotations`: 推論できない初期化子へ期待型を与える局所束縛の型注釈。

### Modified Capabilities

- `basic-expression-type-checking`: 型不明のフィールド、演算、比較、条件、代入を保留せず診断する。
- `function-signature-type-checking`: 引数・戻り値を完全に検査し、省略した戻り値型を `unit` とする。
- `method-call-type-checking`: 型不明レシーバを含む呼び出しを必ず一意に解決し、引数と結果を完全に型付けする。
- `array-type-checking`: 型不明の配列要素・空配列・反復対象を保留せず、文脈または注釈を要求する。
- `optional-core-type-checking`: 裸の `nil` と型不明の fallback を保留せず、期待型または注釈を要求する。
- `optional-field-access`: 型不明レシーバの optional field access を実行前に診断する。
- `fieldless-enum-matching`: 型不明の match 対象・arm 結果・guard を実行前に診断する。
- `fieldless-enums`: 型不明の値を enum 型の宛先へ渡す場合も完全な型照合を要求する。
- `with-provision-type-checking`: 型不明の提供値を実行時へ回さず、検査時に slot 契約との適合を確定する。

## Impact

- `src/typecheck.rs` の推論境界を全式へ広げ、未知の型・参照を診断へ変える。
- `src/ast.rs` と `src/parse.rs` に局所束縛の型注釈を追加し、`src/module.rs` でその型名を正準化する。
- `src/main.rs` は完全な検査結果が得られた場合だけ要求解析と評価へ進む。
- `src/eval.rs` に残る型・呼び出し解決の動的な防御は当面維持するが、型検査済みプログラムでは到達不能になる。
- 既存の型不明を許容する単体・CLI テストと、型検査の説明文書を更新する。
- 外部依存の追加は予定しない。
