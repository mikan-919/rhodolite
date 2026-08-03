# コンパイラへのロードマップ

Rhodolite は、言語の意味をインタプリタで固め、型付き HIR を共通の境界にしてから
**Core WebAssembly** バックエンドを追加する。インタプリタは捨てず、生成コードの
振る舞いを照合する参照実装として残す。

成果物を Core Wasm にした理由と、その上のホスト契約は
[ADR-0009](./adr/0009-core-wasm-is-the-compiler-artifact.md) と
[ADR-0011](./adr/0011-owned-data-layout-and-abi-v1.md)。Component Model・
WIT・WASI・JavaScript は言語の契約に入れず、必要なフレームワークが下流で包む。

この文書は**順序と完了線の地図**であり、個々の機能仕様ではない。OpenSpec change は
各段階へ着手する直前に作り、その時点までに判明した制約を反映する。状態が「未着手」の
change 名は予約名であり、まだ作成済みであることを意味しない。

## 到達点

最初のコンパイル版（compiled v1）は、次をすべて満たした状態とする。

- `examples/canonical.rd` を Core Wasm へ変換できる
- 生成した `.wasm` を独立した Core Wasm ランタイムが検証・実行できる
- 生成モジュールで正典テストが成功する
- 同じプログラムについて HIR インタプリタと生成モジュールの結果が一致する
- 提供忘れ・型エラー・未解決呼び出しは Wasm 生成前に拒否される
- 複数モジュールからなるプログラムをコンパイルできる

compiled v1 は汎用言語としての完成ではない。外部パッケージ、最適化、async、
セルフホスト、本格 GC は含まない。所有権・借用の静的契約はデータ値の Wasm 表現より
先に固める。

## 全体の順序

```text
v1 interpreter（完了）
        │
        ▼
close-static-type-checking（完了）
        │
        ▼
introduce-typed-hir（完了）
        │
        ▼
define-ambient-runtime-abi（完了）
        │
        ▼
emit-core-wasm-programs（完了）
        │
        ▼
introduce-ownership-and-borrowing（完了）
        │
        ▼
compile-wasm-owned-data-values（完了）
        │
        ▼
compile-wasm-traits-and-ambient（完了）
        │
        ▼
add-differential-execution（完了）
        │
        ▼
compiled v1（到達）
```

| 段階 | 状態 | 想定 OpenSpec change | 成果 |
|---|---|---|---|
| 0 | 完了 | archived changes | AST インタプリタと v1 正典 |
| 1 | 完了 | archived `close-static-type-checking` | Unknown のない検査成功 |
| 2 | 完了 | `introduce-typed-hir` | 型付き・名前解決済み HIR |
| 3 | 完了 | archived `define-ambient-runtime-abi` | ambient を明示化できる低水準契約 |
| 4 | 完了 | archived `emit-core-wasm-programs` | スカラーと制御フローの Core Wasm 生成 |
| 5 | 完了 | archived `introduce-ownership-and-borrowing` | 単独所有、借用推論、決定的 drop、checked HIR 境界 |
| 6 | 完了 | `compile-wasm-owned-data-values` | owned data の Wasm 表現、allocator、ABI v1 |
| 7 | 完了 | `compile-wasm-traits-and-ambient` | trait・slot・`with` の Wasm 生成 |
| 8 | 完了 | `add-differential-execution` | 二つの実行系の一致を継続検証し、compiled v1 に到達 |

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
[`introduce-typed-hir`](../openspec/changes/archive/2026-07-30-introduce-typed-hir/)

この段階は利用者向けの新機能ではなく内部表現の置換なので、OpenSpec では
`skip_specs: true` のリファクタ change とする。

AST 上の文字列名を、`CallableId`、`StructId`、`FieldId`、`EnumId`、`VariantId`、
`TraitId`、`TraitMethodId`、`TraitImplId`、`SlotId`、`LocalId`、`ExprId` の
解決済み ID へ変換する。各 HIR 式は具体型または制御脱出の分類と、元ソースの
span を持つ。

完了条件(すべて達成):

- HIR 評価中に名前探索・メソッド候補探索・型推論を行わない
- HIR インタプリタで現在の正典プログラムが同じ結果になる
- 既存の診断位置と要求表示が変わらない
- AST 直接評価を正式な実行経路から外せる

当時の型付き HIR 導入段階では、パイプラインは
`load AST → check/lower HIR → analyze HIR → eval HIR` になった。
`typecheck::check_and_lower` が唯一の境界で、診断が1件でもあれば HIR は渡らなかった。
現行パイプラインは次の ownership 段階で `CheckedProgram` 境界を追加している。
下ろしを全域にするために、型注釈・`effect` の対象・inherent `impl` の対象は
宣言済みの名前でなければならず、代入の左辺は局所束縛か宣言フィールドで
なければならない。どれも以前は実行時に失敗するか黙って通っていた形で、
HIR に置き場所が無い。

## 3. ambient の低水準契約を決める

OpenSpec: archived `define-ambient-runtime-abi` /
ADR: [`0008`](./adr/0008-ambient-abi-is-a-specialization-plan.md)

この段階も内部設計なので `skip_specs: true` のリファクタ change とした。コード生成より
先に、要求推論の結果をどの引数とランタイム表現へ落とすかを固定した。

決まったもの(`src/ambient_abi.rs`):

- 隠し ambient record は**値要求だけ**を欄に持つ不変な値。値要求が無ければ引数が無い
- `with slot(value)` は具体 struct への同一性を保つ handle を1欄。**vtable は無い**
- `with slot<Type>` は instance の鍵と呼び先を変えるだけで、実行時に残らない
- 内側の `with` は外側の record を書き換えず、写した提供文脈を置き換える
- スロット呼び出しは提供の `TraitImplId` から実装本体への直接呼び出しになる
- 再帰・相互再帰は、鍵を歩く前に確保することで同じ instance を共有して閉じる

要求は既存どおり保守的なまま(契約メソッドは全実装の合併)なので、選ばれた実装が
使わない欄が record に残ることがある。要求を provider 依存にするかは別の change。

完了条件(すべて達成):

- 正典プログラム全体が決定的な計画へ機械的に落ちる(スナップショットで固定)
- 各 instance は自分の要求に制限された slot だけを運ぶ
- 型提供と実体提供を同じものとして誤魔化さない(前者は実行時から消える)
- C 側の擬似表現と呼び出し例が ADR-0008 に揃っている

## 4. 最小の Core Wasm 生成を縦に通す

OpenSpec: archived `emit-core-wasm-programs` /
ADR: [`0009`](./adr/0009-core-wasm-is-the-compiler-artifact.md)

ここから利用者が観測できるコンパイル機能になるので、CLI、生成、失敗条件を
OpenSpec capability として定義した。

扱う範囲(v0 の scalar 部分言語):

- `int`、`bool`、`unit`
- ローカル束縛と代入
- 算術と比較
- `if`、`while`
- 直接関数呼び出しと `return`、`assert`
- エントリ関数と、`pub use` で明示選択した公開関数

一緒に決まったもの:

- `int` は符号付き64bit。加減乗と単項マイナスは回り込み、割り算だけが失敗を持つ
- `pub use` が言語の公開再エクスポート。エントリーの明示選択がホスト面になる
- 計画の根は呼び出し側が決める。生産の根は `main` + 公開関数で、test は入らない
- 対応範囲の検査は**到達した instance だけ**。使わない豊かな宣言は止めない

完了条件(すべて達成):

- `.rd` から `.wasm` を生成できる(`rhodolite build <entry.rd> --target wasm`)
- 生成物を独立した Core Wasm の validator と engine が受け付ける
- 小さなプログラムでインタプリタと結果が一致する
- 各段の失敗が既存の診断描画で位置付きで出て、成果物を置き換えない
- 同じ入力・同じ選択肢からは byte 単位で同じモジュールが出る

## 5. 所有・借用を Wasm より先に閉じる

OpenSpec: archived `introduce-ownership-and-borrowing` /
ADR: [0010](./adr/0010-owned-values-and-inferred-borrows.md)

型付き HIR のあとに ownership pass を置く。非 Copy 値は単独所有、`&T` は共有 read、
`&mut T` は排他的 mutation、`move` は明示 transfer、`clone()` は明示 deep clone と
する。borrow の領域と borrowed return の provenance は全プログラムから推論し、
drop は lexical scope exit で決定的に計画する。

この段階は Wasm memory layout を決めない。interpreter が store と検査済み place で
参照意味を実行し、Wasm v0 は引き続き到達した scalar だけを生成する。borrowed public
ABI と reachable non-scalar data は build 前に拒否する。

後続へ意図的に残すもの:

- aggregate に格納する borrow とその region model
- explicit shared ownership（RC / GC を含むかは concrete use case で決める）
- owned data の allocator、memory layout、drop flag、rich public ABI
- async / closure / separate compilation での ownership と provider lifetime

## 6. owned data の Wasm 表現と小さなランタイムを作る

OpenSpec: `compile-wasm-owned-data-values` /
ADR: [0011](./adr/0011-owned-data-layout-and-abi-v1.md)

`str`、owned struct、enum payload、optional、`match`、配列、`for` の順に追加した。
aggregate borrow と shared ownership はこの段階の前提にしていない。

生成モジュールは import-free の coalescing allocator、決定的なデータ layout、drop flag、
clone/drop/equality glue、OOM trap を持つ。公開面は scalar だけなら ABI v0 を維持し、
owned data を含めばモジュール全体で ABI v1 を選ぶ。ABI v1 は内部 heap pointer ではなく、
検証付きの正準 bytes を export memory と予約済み exchange area 経由で運ぶ。

完了条件(すべて達成):

- 現在のデータ型と値操作を Wasm 側で表現できる
- owned struct と配列の変更が参照実装と一致する
- enum、optional、`match` の結果が参照実装と一致する
- 公開 ABI が scalar 以外の値を運べるようになる

意図的に残した aggregate に格納する borrow と shared ownership は、引き続きこの段階の外に
置く。到達した inherent method・trait・slot・`with`・空でない ambient record の生成は次段で
ADR-0008 の計画へ接続し、完了した。

## 7. trait と ambient をコンパイルする

OpenSpec: `compile-wasm-traits-and-ambient`（完了）

Rhodolite 固有の意味を Wasm バックエンドへ接続する段階。ADR-0008 の隠し ambient
record を、空でない layout も運べる実行時表現として初めて実装する。

実装順:

1. inherent method
2. trait implementation
3. trait method の呼び出し表
4. `effect` slot
5. `with slot(value)`
6. `with slot<Type>`
7. 要求推論から生成する隠し ambient 引数
8. ネストした提供

完了条件(すべて達成):

- `examples/canonical.rd` の Wasm を生成できる
- 生成モジュールで正典テストが成功する
- 差し替え先を変えても呼ばれる側の関数を変更しない
- 提供忘れがコード生成より前に到達経路付きで失敗する
- 生成関数が不要な slot を引数に持たない

## 8. 二つの実行系を継続的に照合する

想定 change: `add-differential-execution`

```text
               ┌─ HIR interpreter ──────────── result A
source → HIR ──┤
               └─ Core Wasm → Wasm engine ──── result B

                         result A == result B
```

比較対象は、戻り値、終了コード、標準出力、テスト結果、実行時エラーの分類、
owned 値と明示 borrow を経た最終状態とする。

完了条件:

- 維持されたプログラム群を両方の実行系で自動実行する
- Wasm 生成の snapshot test がある
- 小さな生成プログラムによる差分試験がある
- 生成モジュールの再ビルドが byte 単位で一致することを検証手順に含める
- compiled v1 の到達点を正典プログラムで再現できる

## compiled v1 より後

[ADR-0003](./adr/0003-whole-program-monomorphization.md) の本丸である高階関数の
エフェクト多相は、最初の Wasm バックエンドを通した後に進める。

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

Component Model／WIT の生成、LLVM／Cranelift、ネイティブ生成、最適化、セルフホスト、
パッケージマネージャ、LSP、本格 GC、安定 ABI は、必要な実プログラムが現れてから
別の地図を作る。ownership の後続としては、aggregate borrow、explicit shared
ownership、allocator/data layout、async、rich ABI を別 change で扱う。

## この文書の更新規則

- 現在の段階を archive したとき、状態を「完了」に更新する
- 次の change を作る前に、開始条件と完了条件を実測結果に合わせて見直す
- change 名を変えたら表と該当節を同じコミットで更新する
- 個別仕様や設計をこの文書へ複製せず、OpenSpec change または ADR へリンクする
- 後続段階の詳細が変わっても、compiled v1 の到達点を変える場合は理由を ADR に残す
