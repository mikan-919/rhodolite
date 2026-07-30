## Why

型検査はすべての式型と呼び出し先を確定できるようになったが、その事実は検査中の一時状態にだけ存在し、要求解析と評価器は文字列名を持つ AST を再走査して名前・slot・メソッドを再解決している。C バックエンドへ同じ意味を渡す前に、型と参照を一度だけ解決した共通の HIR を処理系の境界にする。

## What Changes

- 型検査成功時に、モジュール解決済み AST から型付き・参照解決済み HIR を構築する。
- 型、関数、メソッド、struct field、enum variant、trait、slot、局所束縛を文字列ではなくプログラム内 ID で参照する。
- すべての HIR 式に具体型または制御脱出の分類と元ソースの span を保持する。
- 呼び出しを直接関数、具体型メソッド、関連関数、slot trait メソッド、enum constructorの解決済み形に分ける。
- 要求解析を AST の名前探索から HIR の ID と解決済み呼び出し辺へ移す。
- HIR インタプリタを追加し、既存 AST インタプリタとの同値を確認してから正式な実行経路を切り替える。
- CLI のパイプラインを `load AST → check/lower HIR → analyze HIR → eval HIR` に変更する。
- AST 直接評価と重複した実行時名前解決は、同値確認後に正式経路から除く。
- 言語構文、型規則、要求推論、診断文言、実行結果には意図的な変更を加えない。

## Capabilities

### New Capabilities

なし。このchangeは既存言語の内部表現と処理経路だけを置き換える。

### Modified Capabilities

なし。既存main specsの観測可能な要件をすべて維持する。

## Impact

- `src/hir.rs` と、ASTからHIRへ下げる境界を新設する。
- `src/typecheck.rs` は診断列だけでなく、成功時に完全な HIR を返す。
- `src/requirement.rs` と `src/eval.rs` は段階的に HIR を入力とする実装へ移行する。
- `src/main.rs` は HIR を要求解析と評価へ渡す。
- AST、module loader、lexer、parser、診断描画、言語仕様、外部依存は原則として変更しない。
- 利用者向け仕様変更がないため、このchangeは`skip_specs: true`としdelta specsを作らない。
