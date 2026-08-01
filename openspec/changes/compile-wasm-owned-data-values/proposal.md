## Why

Rhodolite の Wasm backend は現在 `unit` / `bool` / `int` しか生成できず、所有権検査済みの文字列・struct・enum・optional・配列を含む正典プログラムへ進めない。ownership pass が move・borrow・drop を確定する境界になった今、その計画を Core Wasm の線形メモリと小さな ownership runtime へ落とす段階に進む。

## What Changes

- 到達した `str`、owned struct、payload enum、optional、配列と、それらに対する field access・代入・`clone()`・`match`・`??`・`for` を Core Wasm へ生成する。
- 決定的な型 layout、間接再帰表現、allocator、drop flag、clone/drop glue、OOM trap を持つ import-free の線形メモリ runtime を生成モジュールへ組み込む。
- ownership pass が確定した move と各 CFG edge の drop plan を Wasm lowering の入力にし、二重解放や未初期化値の破棄を防ぐ。
- 公開関数が owned data を受け渡せる Rhodolite ABI の次版と、その型 layout を発見できる決定的 metadata を定義する。borrowed public signature は引き続き拒否する。
- scalar-only program の既存 byte-determinism、export、trap、到達性に基づく対応検査を維持する。

## Capabilities

### New Capabilities

- `wasm-owned-data-values`: owned data の線形メモリ表現、生成される ownership runtime、データ操作・clone・drop の Core Wasm 実行契約を定義する。

### Modified Capabilities

- `core-wasm-build`: 到達可能な owned data とその制御構文を unsupported とせず、ownership/drop plan に従って生成するよう対応範囲を広げる。
- `rhodolite-wasm-abi`: scalar-only の ABI v0 から、owned data の公開引数・戻り値と layout metadata を扱う次版へ拡張する。

## Impact

- 主な実装対象は `src/wasm.rs`、`src/wasm_abi.rs` と、新設する layout/runtime lowering module。
- `src/ownership.rs` の検査結果と drop plan を、検査専用情報ではなく backend の正式な入力として消費する。
- Wasm encoder/validator/engine の既存依存は維持し、WASI・Component Model・WIT・host import・GC・RC は追加しない。
- aggregate borrow、shared ownership、trait／ambient の Wasm 生成、ユーザー定義 destructor は非目標のまま残す。
