---
status: accepted
date: 2026-07-27
---

# ファイルとディレクトリをモジュールとし、読み込みを `use` に一本化する

Rhodolite にはモジュールがなく、すべての宣言が1つの大域名前空間に入っている。
スロットが増えると、別の関心ごとがどちらも `effect db: Database` と宣言しただけで
衝突する。ソースを複数ファイルへ分けるだけではこの衝突は解けないため、
**モジュール分割と名前空間を同じ機能として導入する**。

Rustのように `mod` でモジュールを読み込み、`use` で名前を導入する二段構えにはしない。
Rhodoliteでは `use` が依存先の読み込みと名前の導入を同時に担う。

## Decision

### ファイルシステムとモジュール木

エントリーファイルの親ディレクトリをソースルートとし、`use` のパスは常に
ソースルート基準の絶対モジュールパスとする。エントリーファイル自身も、
ファイル名を持つ通常のモジュールである。

```text
app.rd                 -> app
services/users.rd      -> services::users
services/billing.rd    -> services::billing
```

ファイル名とディレクトリ名の各要素はRhodoliteの有効な識別子でなければならない。
大文字と小文字は区別し、パスの綴りはファイルシステムの扱いにかかわらず完全一致を
要求する。

リーフモジュールとディレクトリモジュールを分ける。

- `foo.rd` は宣言を持つリーフモジュールで、子モジュールを持たない
- `foo/` は子モジュールだけを持つディレクトリモジュールで、直接の宣言を持たない
- 同じ位置の `foo.rd` と `foo/` は共存できず、両方あればエラー
- `mod.rd`、`index.rd`、モジュール宣言構文は持たない

### `use` は読み込みと名前導入を兼ねる

`use` はモジュール先頭のトップレベル接頭部にだけ書ける。関数やブロック内には
書けない。

波括弧なしはモジュール自身を導入する。

```rhodolite
use data::database
use data::database as db_module

database::connect()
db_module::connect()
```

波括弧ありはモジュールのメンバーを選択して導入する。

```rhodolite
use data::database::{Database, db}
use data::database::{db as primary_db}
```

ディレクトリモジュールのメンバーは子モジュール、リーフモジュールのメンバーは
ファイル内の宣言である。

```text
services/
├── users.rd
└── billing.rd
```

```rhodolite
use services::{users, billing as payments}

users::run()
payments::charge()
```

選択リストは複数行と末尾カンマを許すが、空にはできない。

```rhodolite
use infrastructure::database::{
    Database,
    Postgres,
    db as primary_db,
}
```

glob import、相対パス、ファイルパス文字列、外部パッケージは扱わない。

```rhodolite
use foo::*          // 無い
use super::foo      // 無い
use "../foo.rd"     // 無い
```

### 読み込まれるプログラム

プログラムへ入るのは、エントリーモジュールから `use` と修飾参照を通じて到達した
モジュールだけである。ソースルートやディレクトリ配下の `.rd` を一括走査しない。

```rhodolite
use services

fn main() {
    services::users::run()
}
```

この場合は `services::users` を読み込むが、参照されていない
`services::billing` は読み込まない。`services` を `use` せず本体だけに
`services::users::run()` と書くことはできない。依存の入口は必ず `use` に現れる。

モジュール間の循環 `use` は許可する。トップレベルは宣言だけで実行時初期化がないため、
初期化順の意味論を持ち込む必要がない。読み込みは到達モジュールの収集、全宣言名の登録、
名前解決の順に行う。

### 名前解決

導入したモジュールからの参照は、宣言種別にかかわらず `module::item` と書く。

```rhodolite
database::Database
database::Postgres
database::connect()
database::db.save(user)

with database::db(store) {
    service::run()
}
```

`::` はモジュールからメンバーへの射影と、既存の型・スロットから関連関数への射影で
共用する。先頭の名前が何へ解決されたかで区別する。

自モジュールの名前は暗黙にスコープへ入れない。自モジュール内の宣言は裸の名前で参照し、
`self::` や `crate::` は導入しない。

初期実装ではモジュールのメンバーを1つの名前空間に置く。trait、struct、effect、fn、
子モジュールなど、種類が違っても同名なら衝突エラーにする。import同士、importと
ローカル宣言の同名も暗黙に上書きせずエラーにし、`as` で解消する。

関数内ではADR-0005の字句的シャドーイングを維持する。ローカル束縛は外側のimportや
スロットを隠し、構文位置によって外側の同名宣言へフォールバックしない。

import名と `as` 別名は、そのモジュール内だけの綴りである。宣言の同一性には常に
宣言元の完全修飾名を使う。

```text
primary_db -> infrastructure::primary::db
replica_db -> infrastructure::replica::db

infrastructure::primary::db != infrastructure::replica::db
```

要求推論、提供判定、重複診断、到達経路でも完全修飾名を表示する。

### 可視性と再公開

初期実装ではトップレベル宣言をすべて他モジュールから参照可能とし、`pub` / privateを
導入しない。通常の `use` は現在のモジュールだけに効き、自動的に再公開しない。

再公開には将来 `pub use` を使う方針だけを予約するが、今回の実装範囲には含めない。

### テスト

テストの所属・探索・実行方法はこのADRで決めない。現在の `test` 構文自体を将来
再設計するため、モジュール対応では既存の単一ファイルの挙動を壊さないことだけを求める。
複数モジュールにまたがるテスト実行は別の判断とする。

## 完了線

最初の縦切りは、同じローカル名のスロットを持つ2つのリーフモジュールを、
ディレクトリモジュールから選択・別名導入し、1つのエントリーから別々に提供して
実行できること。

```text
modules/
├── main.rd
└── sides/
    ├── left.rd
    └── right.rd
```

```rhodolite
// main.rd
use sides::{left as primary, right as replica}

fn main() {
    with primary::db(primary::Store {}),
         replica::db(replica::Store {}) {
        primary::read() + replica::read()
    }
}
```

```rhodolite
// sides/left.rd
trait StoreApi {
    fn read(self -> Int)
}

struct Store {}

impl StoreApi for Store {
    fn read(self -> Int) { 1 }
}

effect db: StoreApi

fn read() {
    db.read()
}
```

`sides/right.rd` も同じローカル名 `StoreApi`、`Store`、`db`、`read` を宣言し、
`StoreApi::read` が `2` を返す。

`sides::left::db` と `sides::right::db` は別のスロットとして推論・提供され、
プログラムは `3` を返す。

併せて、存在しないモジュール、存在しない選択メンバー、import名の衝突、
`foo.rd` / `foo/` の衝突、`use` していないモジュール参照を診断し、
循環 `use` が解決できることを検査する。

## Considered Options

- **Rustと同じく `mod` と `use` を分ける。** ファイルパスがモジュール名を一意に
  決めるため、親ファイルで子を再宣言する二重管理は要らない。読み込みと名前導入を
  `use` に一本化する。
- **ファイル名とは別に `module` / `namespace` を宣言する。** パスと宣言名の不一致を
  検査する規則が増えるため採らない。
- **`foo.rd` と `foo/` を1つのモジュールとして合成する。** 宣言と子を同居できるが、
  1つのモジュールが2つのファイルシステム要素に分かれる。リーフとディレクトリを
  排他的にして、モジュールの所在を一意にする。
- **ディレクトリを `use` したら配下を全読み込みする。** 未使用ファイルのエラーまで
  ビルドを壊し、whole-programの範囲が意図せず広がるため採らない。
- **glob importを許す。** モジュール自身を導入すれば全メンバーへ修飾アクセスできる。
  裸の名前を大量に流し込み、スロットの出所と衝突を見えにくくするglobは採らない。
- **Rustのように型・値など複数のモジュール名前空間を持つ。** 型検査前の段階で
  `use` の名前解決規則が増えるため、まず単一名前空間にする。
- **循環 `use` を禁止する。** トップレベル初期化がなく、whole-programで全本体を見る
  設計では禁止する意味上の理由がないため許可する。

## Consequences

- モジュール対応はファイルローダーだけでは終わらない。AST中の宣言・参照を完全修飾名へ
  解決してから、要求推論と評価器へ渡す名前解決段が必要になる。
- スロットのキーと関数の呼び出し辺は裸の文字列名ではなく完全修飾名になる。
  同名スロットを分離できる一方、解析表示とエラー経路もモジュール名を含む。
- whole-programの意味は「エントリーから到達したモジュール本体をすべて見る」であり、
  ADR-0003のモジュール単独型検査を行わない方針は変わらない。
- 外部パッケージを導入する場合は、将来のマニフェストでパッケージ名を別の
  ソースルートへ対応させれば、`use package::module` の表面構文を維持できる。
- この判断を実装するRust側の構造はまだ決めていない。ADR-0004に従い、新しい型・
  フィールド・名前解決段・不変条件を追加する前に一覧を申告し、mikanが決める。
