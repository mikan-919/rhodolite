## Why

汎用 `map`(MAP milestone)は、`fn map<T, U>(...)`、`trait Map<T>`、
`impl<T> Map<T> for [T]` の型パラメータ構文を土台にする。MAP-000 が
`ROADMAP.md` の Decisions(MAP-Q1〜MAP-Q5)でこの構文と境界を固定した。
MAP-020 以降が型検査・具体化・trait解決・ownership・コード生成を積む前に、
まずパーサと型表現がこの構文を受け付け、型パラメータを安全に扱える土台が要る。

## What Changes

- `fn name<T, U>(...)`、`trait Name<T> { ... }`、`impl<T> Name<T> for Type`
  を parse できるようにする(`struct` / `enum` には広げない、MAP-Q1)。
- `Item::Impl` の trait 参照を型引数付き(`Map<T>`)に、対象型を任意の型注釈
  (`[T]` を含む)に広げる。
- 宣言した型パラメータ名を、その宣言(署名・trait 参照・対象型)の中だけで
  有効な安定 ID として解決する。重複宣言とスコープ外の参照は source span 付きで
  拒否する。
- generic 宣言の署名を保持する型表現を HIR 側に追加する。この表現は型パラメータ
  参照を保持できるが、ownership・要求解析・interpreter・Wasm 生成が読む既存の
  `hir::Type` には型パラメータ用の variant を追加しない。従来通り常に具体型だけを
  表せることが「具体化後の HIR に型変数が残らない」不変条件そのものになる。
- generic な `fn` / trait method / impl method は、この署名表現の構築と妥当性検査
  (重複・スコープ)だけを行い、既存の全域型検査・callable/method 解決・ownership・
  要求解析・interpreter・Wasm 生成のパイプラインには接続しない(MAP-020 以降の範囲)。
- 型パラメータを持たない既存の `fn` / `trait` / `impl` の AST 構造、型検査の結果、
  HIR、実行結果は変更しない。

## Capabilities

### New Capabilities
- `generic-type-parameters`: `fn` / `trait` / `impl` の型パラメータ宣言構文、
  宣言内スコープでの型パラメータ解決、重複・スコープ外診断、そして generic 本体を
  保持する型表現と具体化後 HIR の型変数不在不変条件を定義する。

### Modified Capabilities
- なし。既存の non-generic 宣言に対する挙動・spec は変更しない。

## Impact

- `src/lex.rs` / `src/parse.rs`: 型パラメータリストの字句・構文解析(`fn` 署名、
  `trait`、`impl`)、および `impl` の trait 参照・対象型の型注釈化。
- `src/ast.rs`: `Sig` / `Item::Trait` / `Item::Impl` に型パラメータ宣言を追加し、
  `Item::Impl` の `trait_name` / `type_name` を型引数・型注釈を持てる形に広げる。
- `src/module.rs`: 型パラメータ名を宣言内スコープとして扱い、モジュール名解決が
  それを誤って未解決名やグローバル宣言として扱わないようにする。
- `src/typecheck.rs` / `src/hir.rs`: 型パラメータの安定 ID・generic 署名表現を追加し、
  generic 宣言を既存の全域型検査・HIR 具体化パイプラインの対象から除外する。
- `src/render.rs` および既存の AST/HIR 診断出力: 型パラメータを持つ宣言の表示を
  追加しつつ、non-generic 宣言の出力は変更しない。
- `docs/grammar.md`: `fn` / `trait` / `impl` の型パラメータ構文を追記する。
- 新規外部依存、公開 ABI、CLI コマンドは追加しない。インタプリタと Wasm 生成の
  実行経路には generic 宣言をまだ接続しないため、実行結果への影響はない。
