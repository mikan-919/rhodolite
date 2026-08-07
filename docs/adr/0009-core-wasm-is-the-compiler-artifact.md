---
status: accepted
date: 2026-07-31
---

# コンパイラの成果物は Core Wasm と Rhodolite ABI v0

コンパイラが出すのは、**import を1つも要求しない Core WebAssembly モジュール**と、
その中に埋め込んだ1つのインタフェース記述だけとする。Component Model・WIT・WASI・
JavaScript・ホストフレームワークは、いずれもコンパイラの契約に入れない。

[ADR-0008](./0008-ambient-abi-is-a-specialization-plan.md) が決めた特殊化計画の、
**出力側**を固定する ADR にあたる。C 生成を次段としていた計画をここで差し替える。

> [ADR-0011](./0011-owned-data-layout-and-abi-v1.md) が、下の決定6が引いた
> 「v0 は scalar だけ」という線と、境界表現が scalar に閉じるという前提を
> 差し替えた。成果物が import-free な Core Wasm + 埋め込み JSON であること、
> 到達したところだけを対応検査すること、scalar の境界表現そのものは変わらない。

## Decision

### 1. 正規の成果物は Core Wasm

生成物は普通の関数 export と Rhodolite のメタデータ用 custom section を1つ持つ。
host import は無く、Wasm の start section も持たない。したがって instantiate は
何も走らせず、`__rhodolite_main` を呼ぶかどうかはフレームワークが決める。

Core Wasm を選んだのは、ブラウザと単独ランタイムが共有する移植可能な実行基盤が
それだからである。Component バイナリを v0 の正規表現にする案は却下した。Rhodolite
自身のデータ意味と async がまだ決まっていない段階で、Component Model・Canonical
ABI・その async の進化を先に継承することになる。JavaScript バックエンドを主軸に
する案も却下した。生成コードの実行意味に JavaScript のランタイム意味が入り込む。

C や Cranelift への直接生成は後から足せるが、次のロードマップ目標ではなくなった。

WIT は将来 Rhodolite のインタフェース記述から生成しうるし、フレームワークが Core
モジュールを Component へ包むこともできる。**どちらも ABI v0 の正規表現ではない。**

### 2. ホスト面は `pub use` の明示選択だけ

`pub use path::{name}` でエントリーモジュールが明示選択した関数だけが、ホストから
呼べる export になる。モジュール形の `pub use path` は言語側の名前空間を1つ足すが、
配下の関数を再帰的にホスト面へ出さない。

読み込んだ宣言をすべて公開 ABI と見なす案は却下した。内部の補助関数を1つ足すたびに
ホストとの契約が黙って変わる。専用の `export` 宣言を足す案も却下した。モジュール
利用者とホストが別々の公開面を見ることになる。

### 3. export 名は予約名1つ + 公開名そのまま

エントリーは常に `__rhodolite_main`。公開関数はローカルの公開名で export する。
正準名も特殊化名も export にはならない。`__rhodolite_main` を公開名に使うことは
できず、その場合は `pub use` の宣言位置で拒否する。

エントリを Wasm の start 関数にする案は却下した。`main` は値を返しうるし、いつ
呼ぶかはコンパイラの決めることではない。

### 4. 失敗は trap で伝える

実行時の失敗はすべて Wasm の trap としてホストへ出る。状態コード・エラーバッファ・
例外オブジェクト・位置情報を公開署名に足さない。成功した呼び出しの結果は Rhodolite
の戻り値そのものと区別が付かない。

状態返しを足す案は却下した。すべての署名が変わり、まだ決めていないメモリの所有権が
先に必要になる。ソース位置付きの診断はインタプリタ側に残っており、構造化した
コンパイル時エラー転送はメモリと豊かな ABI 値が決まってからでよい。

### 5. インタフェース記述は埋め込みの正準 JSON

モジュールは `rhodolite.abi` という custom section をちょうど1つ持つ。中身は
圧縮形の UTF-8 JSON で、鍵の並びは `version` / `entry` / `exports`、関数は
`name` / `params` / `result`。公開 export は名前の UTF-8 バイト昇順。

```json
{"version":0,"entry":{"name":"__rhodolite_main","params":[],"result":"int"},
 "exports":[{"name":"find_user","params":["int"],"result":"bool"}]}
```

サイドカーファイルにする案は却下した。`.wasm` は単体で配れる単位でなければならない。
独自バイナリ・CBOR・MessagePack は却下した。記述は極小で、フレームワーク作者が
Rhodolite 専用のツール無しに読めることの方が価値が大きい。

**この JSON は Rhodolite の契約であって、ユーザーが書く IDL ではない。**

### 6. v0 が扱うのは scalar だけ、検査は到達したところだけ

v0 は `unit` / `bool` / `int`、scalar のリテラルとローカル、束縛と代入、算術と
等値、ブロック、`if`、`while`、直接の自由関数呼び出し、`return`、`assert` を扱う。

読み込んだコード全体は従来どおり全域の静的検査を通る。そのうえで **Wasm の対応検査は
生産の根から到達した instance だけ**に走らせる。到達しない豊かな宣言は生産ビルドを
止めない。到達したものを黙って飛ばすことはしない。正しい形の、振る舞いの違う
モジュールが出てしまう。

境界での表現は `int` → `i64`、`bool` → `i32` の 0/1、`unit` → 結果なし。公開の
bool 引数は 0/1 以外を受けたら本体へ入る前に trap する。`unit` の引数は拒否する。
消すとソースとホストで引数の個数がずれる。

## Consequences

- 最初の成果物は正典プログラムをまだコンパイルできない。データ値の段が要る
- ホスト連携は「Core Wasm + 小さな JSON」を読める側なら誰でも書ける
- Component/WIT が要るフレームワークは、この成果物を包むアダプタを自分で持つ
- 生産の依存は Wasm の符号化と検証だけ。実行エンジンは開発依存に留める
