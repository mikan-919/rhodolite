## Why

Rhodolite の Wasm backend は owned data まで生成できる一方、到達可能な inherent method、trait method、`effect` slot、`with` を未対応として拒否するため、言語の中心機能を使う `examples/canonical.rd` をコンパイルできない。型・所有権検査と ADR-0008 の特殊化計画がすでに呼び先と provider access を確定している今、その計画を既存の owned-data runtime へ接続する。

## What Changes

- 到達した inherent method と trait implementation body を、通常関数と同じ checked calling convention で Core Wasm へ生成する。
- trait method と slot method の呼び出しを、ambient 特殊化計画が解決した具体的な callable instance への直接呼び出しとして生成する。vtable や実行時実装選択は導入しない。
- 値提供だけを含む決定的な ambient record を生成し、必要な instance に限って隠し引数として渡す。型だけの提供は実行時から消去する。
- `with slot(value)` と `with slot<Type>`、複数提供、入れ子の上書きを、検査済みの ownership mode と provider context に従って生成する。
- `examples/canonical.rd` の production path を Core Wasm へ生成・実行し、差し替えた provider が経由関数の変更なしに選択される統合テストを追加する。
- aggregate-stored borrow、shared ownership、動的 provider、vtable、公開 ABI 上の ambient injection は引き続き対象外とする。

## Capabilities

### New Capabilities

- `wasm-traits-and-ambient`: method、trait implementation、slot、`with`、特殊化された ambient record の Core Wasm 実行契約を定義する。

### Modified Capabilities

- `core-wasm-build`: 到達可能な method・trait・ambient operation と空でない runtime ambient record を unsupported とせず、特殊化計画に従って生成するよう対応範囲を広げる。

## Impact

- 主な実装対象は `src/wasm.rs`、`src/ambient_abi.rs` と、ambient record／呼び出し lowering を分離する新しい backend module。
- `CheckedProgram`、ownership plan、deterministic layout/runtime、ABI v0/v1、import-free Core Wasm という既存境界は維持する。
- ソース構文、型規則、要求推論、インタプリタの意味、公開 Wasm ABI に破壊的変更はない。
