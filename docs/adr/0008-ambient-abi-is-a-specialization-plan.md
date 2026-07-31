---
status: accepted
date: 2026-07-31
---

# ambient の実行時契約は特殊化計画で表す

ambient は実行時に**存在しない**。到達した本体を「どの実装の組み合わせで呼ばれたか」
ごとに複製すれば、スロット呼び出しは具体的な関数への直接呼び出しになり、値提供だけが
不変な record として引数に載る。この ADR は、その境界を実行可能な内部表現
(`src/ambient_abi.rs`)として固定する。

[ADR-0003](./0003-whole-program-monomorphization.md) が「エフェクト変数を単相化で
消す」と決めたことの、ambient 部分の具体化にあたる。C 生成そのものは次段。

## Decision

### 1. 到達した本体だけを、実装の組み合わせごとに単相化する

`main`(または選ばれたエントリ)と全ての `test` を根に、空の提供文脈から需要駆動で
歩く。instance の同一性は次と等しい。

```text
InstanceKey {
    body:      BodyId
    providers: [(SlotId, TraitImplId)]   // SlotId 順、その本体の要求に制限
}
```

**提供された値・それを作った式・呼び出し地点は鍵に入らない。**別の `InMemoryDb` を渡す
2つの呼び出しは同じコードを共有し、違うのは record に載る handle だけ。呼び出し元が
持っていても呼び先が要求しないスロットは鍵に入らないので、無関係なスロットでコードが
増えない。

宣言 × 実装の全組み合わせを作る案は却下した。使われない instance が出て、出力量を
「到達したコード」ではなく「実装の個数」が支配する。

### 2. vtable は無い

`Call::Slot` は、その地点の提供が持つ `TraitImplId` と `TraitMethodId` から
実装本体の `CallableId` を1手で引き、その特殊化 instance への直接の辺になる。実行時の
分岐表も実装 ID による switch も持たない。

提供は全て HIR で静的に解決済みで、第一級の動的 provider は言語に無い。混成の
フォールバックは、呼び出し規約が2つになる代償に見合わない。

### 3. 型提供は実行時から消え、値提供だけが record になる

型要求(`db::new()` の側)は instance の鍵と直接呼び出し先を変えるが、実行時には
何も運ばないので欄を持たない。値要求(`db.save(u)` の側)だけが1欄を占める。

```text
RecordLayoutKey = [(SlotId, StructId)]   // SlotId 順、値要求のみ
```

値要求が1つも無い instance は**隠し ambient 引数を持たない**。runtime tag も
nullable な欄も置かない。実装も要求の強さも静的に分かっているため、実行時に
言い分ける必要がない。

### 4. record は不変な値で、欄は同一性を保つ handle

呼び先の record は値として渡す。欄は `with` が置いた provider そのものへの handle で、
record を作る・写す・渡す操作は provider を複製しない。provider 経由の変更は全ての
別名から見え、これは HIR インタプリタの共有 struct 意味論と一致する。

物理表現はこの段では抽象のまま置く。compiled v1 の第一候補はプロセス寿命の arena への
ポインタだが、安定した同一性さえ保てば後段の data layout が別の handle を選んでよい。

呼び出し元のスタック上 record への借用ポインタを意味論にする案は却下した。寿命が再帰と
将来の async 下ろしへ漏れる。provider の実体を record へ複製する案も却下した。同一性が
壊れ、正典の共有変更テストが落ちる。

### 5. `with` は外で評価してから、内側の写しを1つ作る

```text
with db(make()), clock<Frozen> { body }
```

1. 提供値を**全て外側の文脈で**ソース順に評価する
2. 外側の提供文脈を写す
3. 名指されたスロットを、選ばれた実装と(あれば)値の出どころで**まとめて**置き換える
4. その内側の文脈で本体を計画する
5. ブロックを抜けたら内側の文脈は捨てる

同じ `with` の提供は互いを見ない。入れ子の `with` は外側の record を書き換えずに
置き換える。順に差し込む案は、インタプリタとも要求解析とも食い違うので却下した。

### 6. 要求は保守的なまま。使わない欄が残ることを許す

契約メソッドの要求は、その契約を実装する**全ての本体**の要求の合併(既存の挙動)。
計画はこの結果をそのまま使うので、選ばれた実装がもっと少ないスロットで足りる場合でも、
呼び出し元の record には使われない欄が残ることがある。

例: `Database::save` の実装の一方が `clock` を使い、他方が使わないとき、`db.save` を
呼ぶ関数は `clock` を使わない実装を選んでも `db + clock` を運び続ける。呼び出し自体は
直接呼び出しになるが、欄は消えない。

要求を provider 依存にすると、通るプログラム・表示される要求・提供忘れの到達経路が
同時に変わる。それはこの change の範囲ではない。計画の決定的な出力は、その無駄を
後から**測れる**形で残すためにある。

### 7. 再帰は早期の interning で閉じる

鍵を初めて要求したときに `InstanceId` を確保し、**本体を歩く前に**表へ入れる。再帰の辺は
その ID を再利用し、worklist が後から本体を埋める。相互再帰も同じ仕掛けで閉じ、別の
呼び出し規約を要らない。

入れ子の提供が実装の組み合わせを変えれば別の instance になるが、callable・slot・
`TraitImplId` はいずれも有限で、提供値は鍵に入らないので状態空間は有限。

## 擬似 C

綴りは説明のためのもの。**規範なのは**欄の順・具体的な provider 型・値渡しで不変な
record・同一性を保つ handle・直接の呼び先の5つだけで、C の型名や記号名ではない。

### 本番とテストの提供の組み合わせ

正典プログラムの `stamp` は、`db` と `clock` の実装の組ごとに1つずつ出る。

```c
typedef struct { Handle_Postgres db;   Handle_SystemClock clock; } Ambient_db_Postgres__clock_SystemClock;
typedef struct { Handle_InMemoryDb db; Handle_Frozen      clock; } Ambient_db_InMemoryDb__clock_Frozen;

void stamp__db_Postgres__clock_SystemClock(Ambient_db_Postgres__clock_SystemClock ambient, Handle_User u) {
    u->promoted_at = SystemClock_now(ambient.clock);   /* clock.now() が直接呼び出しになる */
    Postgres_save(ambient.db, u);                      /* db.save(u) も同じ */
}

void stamp__db_InMemoryDb__clock_Frozen(Ambient_db_InMemoryDb__clock_Frozen ambient, Handle_User u) {
    u->promoted_at = Frozen_now(ambient.clock);
    InMemoryDb_save(ambient.db, u);
}
```

`promote` と `handle` は1文字も ambient を書いていないが、同じように2つ出て、
受け取った record をそのまま呼び先へ渡す。

```c
bool promote__db_InMemoryDb__clock_Frozen(Ambient_db_InMemoryDb__clock_Frozen ambient, int64_t id) {
    User u = InMemoryDb_find(ambient.db, id);
    if (u == NULL) return false;
    u->rank = Rank_Gold;
    stamp__db_InMemoryDb__clock_Frozen(ambient, u);   /* 射影は恒等 */
    return true;
}
```

### `with` は record を作る場所

```c
bool main(void) {
    Handle_Postgres    db    = Postgres_new("postgres://localhost/app");  /* 提供値は外で評価 */
    Handle_SystemClock clock = SystemClock_new();

    Ambient_db_Postgres__clock_SystemClock ambient = { .db = db, .clock = clock };
    return handle__db_Postgres__clock_SystemClock(ambient, 1);
}
```

### 入れ子の `with` は置き換えであって書き換えではない

```c
/* with clock(frozen) { ... with clock(zero) { spin() } ... } */
Ambient_clock_Frozen outer = { .clock = frozen };
Ambient_clock_Zero   inner = { .clock = zero };     /* outer は触らない */

spin__clock_Zero(inner);
spin__clock_Frozen(outer);                          /* 抜けたら外側の選択に戻る */
```

### 型だけの提供は引数を持たない

```c
/* with clock<Frozen> { clock::zero() } */
int64_t zeroed__clock_Frozen(void) {   /* ambient 引数が無い */
    return Frozen_zero();              /* 実装の選択は関数名にしか残っていない */
}
```

### 再帰は同じ record を運び続ける

```c
int64_t ping__clock_Frozen(Ambient_clock_Frozen ambient, int64_t n) {
    if (n == 0) return Frozen_now(ambient.clock);
    return ping__clock_Frozen(ambient, n - 1);   /* 自分自身の instance */
}
```

## Consequences

- 要求推論の結果は表示名ではなく `BodyId` / `SlotId` で後段から読める
- 正典プログラム全体が、決定的な instance と record layout の集合へ機械的に落ちる
- ソース言語・型検査・要求診断・インタプリタ・CLI の観測できる振る舞いは変わらない
- 計画は CLI から見えない。オプションは足していない

## 保留

この段では決めない。決めるときに前提を壊さないことだけを要求する。

- **handle の物理表現** … `compile-data-values` へ。ここで前提にしているのは
  「安定した同一性」だけ
- **C の型名と記号名の綴り** … emitter へ。計画の ID から決定的に導き、公開 ABI に
  しない
- **async のタスク継承と provider の寿命** … async が言語のロードマップに入るまで。
  ただし ambient record は**値として捕捉できる**必要があり、`with` のスタック枠が
  provider の記憶域を所有すると書いてはいけない
- **cross-thread の変更** と `Send` 相当の制約 … 同上
