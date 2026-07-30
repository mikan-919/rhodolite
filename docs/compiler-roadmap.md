# コンパイラへのロードマップ

Rhodolite は、言語の意味をインタプリタで固め、型付き HIR を共通の境界にしてから
C バックエンドを追加する。インタプリタは捨てず、生成コードの振る舞いを照合する
参照実装として残す。

この文書は**順序と完了線の地図**であり、個々の機能仕様ではない。OpenSpec change は
各段階へ着手する直前に作り、その時点までに判明した制約を反映する。状態が「未着手」の
change 名は予約名であり、まだ作成済みであることを意味しない。

## 到達点

最初のコンパイル版（compiled v1）は、次をすべて満たした状態とする。

- `examples/canonical.rd` を C へ変換できる
- 生成した C をシステムの C コンパイラで実行ファイルにできる
- 生成バイナリで正典テストが成功する
- 同じプログラムについて HIR インタプリタと生成バイナリの結果が一致する
- 提供忘れ・型エラー・未解決呼び出しは C 生成前に拒否される
- 複数モジュールからなるプログラムをコンパイルできる

compiled v1 は汎用言語としての完成ではない。外部パッケージ、最適化、async、
所有権、セルフホスト、本格 GC は含まない。

## 全体の順序

```text
v1 interpreter（完了）
        │
        ▼
close-static-type-checking（進行中）
        │
        ▼
introduce-typed-hir
        │
        ▼
define-ambient-runtime-abi
        │
        ▼
emit-core-c-programs
        │
        ▼
compile-data-values
        │
        ▼
compile-traits-and-ambient
        │
        ▼
add-differential-execution
        │
        ▼
compiled v1
```

| 段階 | 状態 | 想定 OpenSpec change | 成果 |
|---|---|---|---|
| 0 | 完了 | archived changes | AST インタプリタと v1 正典 |
| 1 | 完了 | archived `close-static-type-checking` | Unknown のない検査成功 |
| 2 | 計画済み | `introduce-typed-hir` | 型付き・名前解決済み HIR |
| 3 | 未着手 | `define-ambient-runtime-abi` | ambient を明示化できる低水準契約 |
| 4 | 未着手 | `emit-core-c-programs` | スカラーと制御フローの C 生成 |
| 5 | 未着手 | `compile-data-values` | struct・enum・optional・配列の C 表現 |
| 6 | 未着手 | `compile-traits-and-ambient` | trait・slot・`with` の C 生成 |
| 7 | 未着手 | `add-differential-execution` | 二つの実行系の一致を継続検証 |

同時に進行中にするのは原則として一段階だけとする。前段の完了線を満たし、change を
archive してから次段の提案を作る。

## 1. 型検査を閉じる

OpenSpec:
[`close-static-type-checking`](../openspec/changes/archive/2026-07-30-close-static-type-checking/)

目的は、検査成功後に「型が分からない」「呼び出し先が分からない」を残さないこと。
戻り値注釈なしを `unit` とし、文脈を必要とする局所値には
`let name: Type = value` を使う。

完了条件:

- すべての値を産む式が具体的な型を持つ
- `return` など値を産まない経路が Unknown と区別される
- すべての呼び出しが一意の宣言へ解決される
- 型不明による実行時への先送りが CLI の検査経路から無くなる
- 正典プログラムと全テストが通る

## 2. 型付き HIR を導入する

OpenSpec:
[`introduce-typed-hir`](../openspec/changes/introduce-typed-hir/)

この段階は利用者向けの新機能ではなく内部表現の置換なので、OpenSpec では
`skip_specs: true` のリファクタ change とする。

AST 上の文字列名を、`FnId`、`TypeId`、`FieldId`、`VariantId`、`SlotId` などの
解決済み ID へ変換する。各 HIR 式は具体型と元ソースの span を持つ。

完了条件:

- HIR 評価中に名前探索・メソッド候補探索・型推論を行わない
- HIR インタプリタで現在の正典プログラムが同じ結果になる
- 既存の診断位置と要求表示が変わらない
- AST 直接評価を正式な実行経路から外せる

## 3. ambient の低水準契約を決める

想定 change: `define-ambient-runtime-abi`

この段階も内部設計なので、原則として `skip_specs: true` とする。コード生成より先に、
要求推論の結果をどの引数とランタイム表現へ落とすかを固定する。

最低限決めるもの:

- 関数へ渡す隠し ambient record
- `with slot(value)` の値と trait vtable
- `with slot<Type>` の型側提供
- 内側の `with` によるスロットの置換
- 型要求と値要求の違い
- 再帰・相互再帰で共有できる呼び出し規約

完了条件:

- 正典プログラム全体を低水準表現へ機械的に落とせる
- 各関数が要求する slot だけを引数として運べる
- 型提供と実体提供を同じものとして誤魔化さない
- C 側の擬似表現と呼び出し例が設計文書に揃う

## 4. 最小の C 生成を縦に通す

想定 change: `emit-core-c-programs`

ここから利用者が観測できるコンパイル機能になるため、CLI、生成、失敗条件を
OpenSpec capability として定義する。

最初に扱う範囲:

- `int`、`bool`、`unit`
- ローカル束縛と代入
- 算術と比較
- `if`、`while`
- 直接関数呼び出しと `return`
- エントリ関数

完了条件:

- `.rd` から `.c` を生成できる
- 生成 C をビルドして実行できる
- 小さなプログラムでインタプリタと終了結果が一致する
- C コンパイラ失敗を Rhodolite CLI が明確に報告する
- 生成 C が決定的で、人間が読める

## 5. データ値と小さなランタイムを作る

想定 change: `compile-data-values`

追加する順序は、`str`、struct、enum payload、optional、`match`、配列、共有された
可変値、`for` とする。

compiled v1 のメモリ管理は短命な CLI を対象にしたプロセス寿命の arena を第一候補とする。
本格 GC は最初の C 生成を遮らないよう後段へ送る。この選択は change 作成時に改めて
実測し、設計判断として記録する。

完了条件:

- 現在のデータ型と値操作を C 側で表現できる
- 共有された struct と配列の変更が参照実装と一致する
- enum、optional、`match` の結果が参照実装と一致する
- sanitizer 付き生成バイナリで不正アクセスが出ない

## 6. trait と ambient をコンパイルする

想定 change: `compile-traits-and-ambient`

Rhodolite 固有の意味を C バックエンドへ接続する段階。

実装順:

1. inherent method
2. trait implementation
3. trait method の呼び出し表
4. `effect` slot
5. `with slot(value)`
6. `with slot<Type>`
7. 要求推論から生成する隠し ambient 引数
8. ネストした提供

完了条件:

- `examples/canonical.rd` の C を生成できる
- 生成バイナリで正典テストが成功する
- 差し替え先を変えても呼ばれる側の関数を変更しない
- 提供忘れがコード生成より前に到達経路付きで失敗する
- 生成関数が不要な slot を引数に持たない

## 7. 二つの実行系を継続的に照合する

想定 change: `add-differential-execution`

```text
               ┌─ HIR interpreter ───────── result A
source → HIR ──┤
               └─ C → C compiler → binary ─ result B

                         result A == result B
```

比較対象は、戻り値、終了コード、標準出力、テスト結果、実行時エラーの分類、
共有値の最終状態とする。

完了条件:

- 維持されたプログラム群を両方の実行系で自動実行する
- C 生成の snapshot test がある
- 小さな生成プログラムによる差分試験がある
- sanitizer を CI 相当の検証手順で実行する
- compiled v1 の到達点を正典プログラムで再現できる

## compiled v1 より後

[ADR-0003](./adr/0003-whole-program-monomorphization.md) の本丸である高階関数の
エフェクト多相は、最初の C バックエンドを通した後に進める。

```text
関数値・クロージャ
        ↓
型パラメータと高階関数
        ↓
呼び出し地点の具体化
        ↓
エフェクト要求を含む単相化
        ↓
関数単位キャッシュと増分ビルド
```

LLVM／Cranelift、最適化、セルフホスト、パッケージマネージャ、LSP、async、
所有権・借用、本格 GC、安定 ABI は、必要な実プログラムが現れてから別の地図を作る。

## この文書の更新規則

- 現在の段階を archive したとき、状態を「完了」に更新する
- 次の change を作る前に、開始条件と完了条件を実測結果に合わせて見直す
- change 名を変えたら表と該当節を同じコミットで更新する
- 個別仕様や設計をこの文書へ複製せず、OpenSpec change または ADR へリンクする
- 後続段階の詳細が変わっても、compiled v1 の到達点を変える場合は理由を ADR に残す
