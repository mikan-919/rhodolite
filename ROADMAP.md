# Roadmap

## Vision
- 書きやすいRubyやJavascriptのような書き心地のまま、Rustのような機能を使えること
- environment(ambient)をつかってプロップのバケツリレーを阻止すること


## Current State

Rhodolite は compiled v1 に到達している。正典プログラム
[`examples/canonical.rd`](./examples/canonical.rd) に対して、ambient の差し替え、
関数ごとの要求推論、提供忘れの到達経路付き診断が動作する。同じ検査済み
プログラムを HIR インタプリタと Core WebAssembly の両方で実行でき、
維持された fixture 群で結果と失敗の分類を差分検証している。

現在の処理系は次のパイプラインが縦につながっている。

```text
source
  → lex / parse / module load
  → static type check + typed HIR lowering
  → ownership / borrowing check
  → CheckedProgram
      ├─ requirement analysis
      ├─ HIR interpreter
      └─ ambient specialization → Core Wasm
```

実装済みの主な能力:

- `effect` slot、trait、`with` による ambient の宣言・使用・提供
- モジュール読み込みと `pub use` による公開関数の選択
- `int` / `bool` / `str` / `unit`、struct、enum、optional、配列
- 分岐、ループ、`match`、method、trait method、名前付き関数値
- 単独所有、明示的な `move` / `clone()`、`&T` / `&mut T` の借用検査、決定的 drop
- owned data を含む Core Wasm 生成、組み込み allocator、公開 ABI v0 / v1
- source span と要求の到達経路を伴う実行前診断

現在の大きな制約:

- 型・ambient 要求・借用の解決は whole-program 前提で、モジュール単独型検査は行わない
- 関数値は名前付きトップレベル関数のみ。クロージャ、型パラメータ、汎用的な高階関数は未実装
- borrow を aggregate に格納できず、shared ownership、GC、raw pointer、`unsafe` は持たない
- 公開 ABI に borrow や callable 値を出せない。ABI はまだ安定化の対象ではない
- async/await、ジェネレータ、バックトラッキング、language-level unwinding はサポートしない
- パッケージマネージャ、LSP、増分ビルド、最適化パイプライン、ネイティブ生成はまだない

この状態の基準線は `cargo test` で検証する。言語の全体像と実装の地図は
[`docs/overview.md`](./docs/overview.md)、compiled v1 までの経緯と完了条件は
[`docs/compiler-roadmap.md`](./docs/compiler-roadmap.md)を参照する。


## Principles

開発時に優先する判断基準。

- シンプルさを優先する
- 後方互換性を守る
- ランタイムを小さく保つ
- 実験的機能は安定APIから分離する
- なるべく読みやすいコードを記述する
  - 名前は言語の概念と役割を表す
  - モジュールの公開面を小さく保ち、実装詳細を内側に隠す
  - コメントには処理の言い換えではなく、理由や制約を書く
  - 実際の重複や利用例が現れる前に抽象化しない

### 自動実装の境界

ROADMAP は AI Agent が繰り返し実装するための作業キューでもある。
Agent は次の条件をすべて満たすタスクだけを自動で開始する。

- 状態が `ready` である
- 依存タスクがすべて `done` である
- 「設計判断」が `不要` である
- 完了条件と検証方法が明記されている

Agent は対象タスクを `in-progress` にし、1タスクずつ実装する。
完了条件を検証し、テストが通った安定状態をコミットしてから `done` にする。

実装中に新しい言語仕様、公開 API、データ表現、ランタイム契約の選択が
必要になった場合、Agent は推測で決めない。タスクを `blocked` にし、
判断が必要な問いを Open Questions に追加して実装を止める。

## Milestones

| タスクID | 状態 | タスク名 | 依存タスク | 工数 | 設計判断 | 完了条件 | 検証方法 |
|---|---|---|---|---|---|---|---|
| | | | | | | | |


## Future Directions

まだ実施を約束していない長期案。


## Non-goals

少なくとも現在は実装しないもの。


## Open Questions

まだ設計判断が終わっていない問題。
