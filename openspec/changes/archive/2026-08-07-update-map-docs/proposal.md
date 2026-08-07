## Why

MAP-000〜MAP-090 で汎用 `map`(型パラメータ、汎用 trait/impl、whole-program 特殊化、
HIR インタプリタと Core Wasm の両方での実行、`Push<T>` 組み込み、正典 fixture と
決定性検証)が実装済みになった一方、README・`docs/overview.md`・`docs/grammar.md`・
`docs/compiler-roadmap.md`・`ROADMAP.md` の現状描写はまだ「型パラメータ・汎用的な
`map` は未実装」だった時点の文言のまま止まっている。利用者向け文書が実装済みの
言語境界と食い違っているので、MAP-100 としてこれらを実装後の状態へ揃える。

コード変更は伴わない。OpenSpec の main specs(`generic-map`、`array-push`、
`differential-execution` など)は MAP-080/MAP-090 の feat コミットで既に sync 済みで
未反映の delta は無いことを確認済み。本 change は文書更新のみを対象とする。

## What Changes

- `README.md`: 「型パラメータ、総称的な `map` はまだ無い」という記述を、
  型パラメータ・汎用 `map` が実装済みであることが分かる記述に更新する。
- `docs/overview.md`: 「現状」節末尾と「9. 次の一歩」節を、MAP-000〜090 完了後の
  状態(型パラメータ付き宣言の検査・具体化・実行、`Push<T>`、`Map<T>`)に合わせて
  書き換える。次の一歩は MAP-100(本文書更新)と、その先のクロージャに更新する。
- `docs/grammar.md`: 「型パラメータ」節の末尾(本体検査・呼び出し地点の型引数推論・
  具体化は未実装、という記述)を実装済みの内容に更新し、配列節に組み込み `push` を、
  型パラメータ節の直後に `Map<T>`/`impl<T> Map<T> for [T]` を通常ソースとして書く
  現在の利用形を追記する。「まだ決めていない」に該当しない既存の「まだ無い」記述
  (明示型引数、generic struct/enum、借用 `map`)は境界として明示のまま残す。
- `docs/compiler-roadmap.md`: 「compiled v1 より後」節の「入っていないもの」表と
  順序図を、型パラメータと汎用 `map` が到達済みであることが分かるように更新し、
  ROADMAP.md の MAP トラックを参照するリンクを足す。
- `ROADMAP.md`: 「Current State」の「現在の大きな制約」から達成済みの記述を外し、
  「Milestones」冒頭の「次の到達点は…汎用 `map` である」という導入と例コードを、
  実装済みの呼び出し規約(`move`、明示的な `trait Map<T>`/`impl` 宣言)に合わせて
  実績として書き換える。

すべて文書のみの変更で、`src/` 配下のコードは変更しない。

## Capabilities

### New Capabilities

なし。新しい観測可能な言語仕様は追加しない。

### Modified Capabilities

なし。`generic-map`、`array-push`、`generic-type-parameters`、
`generic-function-instantiation`、`generic-trait-resolution`、
`generic-instantiation-execution`、`generic-instantiation-wasm`、
`differential-execution` の main specs は MAP-010〜MAP-090 の feat コミットで
既に sync 済みで、今回変更する要件はない。本 change は `.openspec.yaml` に
`skip_specs: true` を設定し、これらの spec には触れない。

## Impact

- 影響ファイル: `README.md`、`docs/overview.md`、`docs/grammar.md`、
  `docs/compiler-roadmap.md`、`ROADMAP.md`
- 影響なし: `src/` 配下のコンパイラ実装、`openspec/specs/` 配下の main specs、
  既存テスト・fixture
- 検証方法: 文書間の用語とリンクのレビュー、
  `bunx @fission-ai/openspec validate --all --strict`
