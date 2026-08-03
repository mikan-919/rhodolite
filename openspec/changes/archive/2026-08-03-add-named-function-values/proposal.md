## Why

`map` のような高階関数で、渡した関数が使う ambient 要求を呼び出し元まで
届けるには、関数を値として渡して呼び出す最小の経路が要る。現在の Rhodolite は
すべての呼び出しを宣言済みの直接 call として解決するため、この経路をまだ持たない。

最初の slice は closure capture と型パラメータを切り離し、名前付きトップレベル関数を
値として扱えるようにする。これにより間接呼び出しとその ambient 要求伝播を先に
検証し、次段の generic `map` をその上に載せられる。

## What Changes

- 名前付きトップレベル関数を、その完全な引数・戻り値シグネチャを持つ callable value として
  参照できるようにする。
- callable value を同じシグネチャを要求する引数・immutable local へ渡し、値の local を
  呼び出せるようにする。
- 間接呼び出し先が名前付き関数値として静的に一意に分かる範囲で、要求解析、所有権検査、
  interpreter、Core Wasm 生成、および差分実行へ接続する。
- callable value を通る呼び出しでも、渡した関数の ambient 要求を呼び出し元へ推論し、
  提供忘れには既存形式の到達経路付き診断を出す。
- 無名関数、capture、closure environment、型パラメータ、generic `map`、配列への追加 API は
  この change の範囲外にする。

## Capabilities

### New Capabilities

- `named-function-values`: 名前付きトップレベル関数の値化、静的な間接呼び出し、そして
  その ambient 要求伝播を定義する。

### Modified Capabilities

- なし。

## Impact

- AST、型、型検査済み HIR、ownership 検査、要求解析、interpreter、ambient specialization
  plan、Wasm lowering に callable value と間接 call の表現を加える。
- パーサと文法ドキュメントに関数型と callable local の呼び出しを加える。
- 新しい differential fixture と Wasm snapshot を追加し、直接呼び出しの既存挙動を維持する。
- 新しい外部依存、CLI、公開 ABI は追加しない。
