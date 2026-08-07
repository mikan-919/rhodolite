---
status: accepted
date: 2026-08-07
---

# `extern fn` と host import — ADR-0009 決定1の限定的な差し替え

effect の `impl`（ハンドラ）は今のところ通常の Rhodolite ソースでしか書けず、
import-free な Core Wasm の中だけで完結する。したがって実際のファイル・
ネットワーク・DB といった host 側の I/O には原理的に届かない。ここで決めるのは、
host が実装する関数を `extern fn` として宣言し、ハンドラの中から呼べるように
することだけである。

[ADR-0009](./0009-core-wasm-is-the-compiler-artifact.md) の決定1
「成果物は import を1つも要求しない」は、ここに限り差し替える。[ADR-0011](./0011-owned-data-layout-and-abi-v1.md)
が決定6（v0 は scalar だけ）を差し替えた前例と同じく、0009 の他の決定
（host 面は `pub use` の明示選択だけ、export 名は予約名+公開名、失敗は trap、
メタデータは埋め込み JSON、到達したところだけを対応検査）は変えない。

## Decision

### 1. `extern fn` は本体を持たない host 実装の宣言

```rhodolite
extern fn pg_save(u: User -> unit)
extern fn pg_find(id: int -> User?)

impl Database for Postgres {
    fn save(u: User) { pg_save(u) }
    fn find(id: int -> User?) { pg_find(id) }
}
```

`extern fn name(params -> ret)` は本体を持たず、named 関数値と同じ型
`fn(P1, P2 -> R)` を持つ。impl・通常の関数どちらの本体からも、既存の関数呼び出しと
同じ構文で呼べる。

trait 実装ごと host へ丸投げする `extern impl` 専用構文は却下した。`impl` の
意味論が「本体は Rhodolite ソース」と「本体は host」の2種類に分岐し、
method 単位で一部だけ host 実装にしたい場合にも対応できない。`extern fn` を
普通の呼び出し可能な値にして、必要な method の中でだけ呼ぶ方が最小の変更で済む。

### 2. 型と符号化契約は公開 ABI と共有する

`extern fn` の引数・戻り値は、[ADR-0011](./0011-owned-data-layout-and-abi-v1.md)
が定めた ABI v1 の型集合（scalar、`str`、struct、enum、optional、配列）を
そのまま使う。符号化・復号も既存の encode/decode をそのまま再利用する。
`&T` / `&mut T` は、既存の公開境界と同じ理由（内部アドレスを host へ渡さない）
で拒否する。

scalar だけに絞る案は却下した。`User` のような struct を渡せない I/O ハンドラは
実用にならず、ABI v1 の符号化はすでにあるので絞る理由がない。

### 3. instantiate は host が満たすべき契約になる

到達した `extern fn` だけを Wasm の import として生成する。import module
namespace は `"host"` に固定し、import 名は Rhodolite 側の `extern fn` 名を
そのまま使う。`rhodolite.abi` には、既存の `exports`（`name` / `params` /
`result`）と対称な形で `imports` 配列を追加する。

```json
{"version":0,"entry":{"name":"__rhodolite_main","params":[],"result":"int"},
 "imports":[{"name":"pg_save","params":["t0"],"result":"unit"}],
 "exports":[{"name":"find_user","params":["int"],"result":"bool"}]}
```

host が対応する import 関数を用意できなければ、instantiate は Wasm 標準の
仕組みにより失敗する。Rhodolite 側で専用の起動前チェックは追加しない。
これにより ADR-0009 決定1「instantiate は何も走らせない」の前提は、`extern`
を使うモジュールに限って崩れる。`extern` を1つも持たないモジュールは
これまでどおり import-free のままで、instantiate は何も要求しない。

extern ごとに別々の namespace を持たせる案は却下した。仕様と実装が複雑になる
一方、host 側が受け取る情報は `imports` 配列で十分であり、namespace を
分ける実利がない。

### 4. ambient 要求はゼロのみ許可する

`extern fn` は ambient を要求できない。host に要求解決の概念が無いため、
ambient を要求する `extern fn` 宣言は実行前に拒否する。一方、effect の
`impl` メソッドが内部で `extern fn` を呼んで実際の I/O を行うことは、この
ADR が想定する主要な用途である（decision 1 の例のとおり）。

### 5. 失敗は既存の trap 契約にそのまま乗せる

ADR-0009 決定4をそのまま踏襲する。`extern fn` 呼び出しの失敗に専用の状態
コードや例外表現は追加しない。回復可能な失敗を表したい `extern fn` は、
戻り値の型に `T?` や enum を使う通常の Rhodolite の表現方法をそのまま使う。

### 6. capture は持ち込まない

`extern fn` は常にキャプチャの無い named 関数として扱う。CLO で導入した
捕捉付き closure に相当する概念は host 側に持ち込まない。テストでは HIR
インタプリタに host スタブ（名前ごとに差し替え可能な実装）を登録する仕組みを
別途 EXT-040 で用意し、`extern fn` をインタプリタ上でも実行できるようにする。

## Consequences

- Core Wasm 成果物は、もはや無条件に import-free ではない。`extern fn` を
  1つも使わないモジュールは引き続き import-free で、この差し替えの影響を受けない
- `extern fn` を使うモジュールの host は、`rhodolite.abi` の `imports` を読み、
  シグネチャの合う関数を用意しなければ instantiate できない。「Wasm+JSON を
  読めれば誰でも動かせる」という ADR-0009 の主張は、この場合に限り
  「対応する import を用意できれば」という条件付きになる
- effect ハンドラは、必要な `extern fn` を呼ぶことで初めて実際の I/O を
  行える。ambient の差し替え可能性という言語の主眼はそのまま、host 側の
  実装差し替えも `extern fn` の実装差し替えとして自然に乗る
- ADR-0009 の他の決定（host 面、export 名、失敗伝達、メタデータ形式、
  v0 の scalar 境界）はここでは変わらない
