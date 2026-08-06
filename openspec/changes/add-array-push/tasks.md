> 実装メモ(design.md 決定1からの逸脱): 組み込み `push` の同一性は
> `hir::Callable` の「本体の種別」ではなく、専用の HIR ノード
> `hir::ExprKind::Push { array, value }` が担っている。現在の木では
> `GenericOwner::Impl` の対象は struct に限られ(`check_generic_impls`
> 「struct ではありません」、`instance_owner` の struct 前提、
> `TraitImplDecl::type_: StructId`)、`[T]` を対象にした generic impl を
> 解決させるには MAP-080 が必要とする「配列を対象にした generic impl」機構を
> 丸ごと先に作ることになる。観測可能な契約(spec の全 scenario)は同じまま、
> 追加した分岐は `.clone()` の組み込みと同じ1本の resolve 分岐 +
> 各パス1アームで済むこの形を選んだ。`array_clone` / `array_drop` と同じ
> 「コンパイラが本体を持つ」位置付けは変わらない。

## 1. `Push<T>` 宣言と組み込みの `[T]` 実装

- [x] 1.1 `trait Push<T> { fn push(&mut self, x: T) }` は既存の generic trait
      宣言でそのまま通る(MAP-025 の `GenericOwner::Trait` に載る)。
      焦点テスト `push契約のtraitを宣言できる` で契約の形を固定した。
- [x] 1.2 組み込み実装の同一性は `hir::ExprKind::Push` が持つ(冒頭のメモ)。
      `src/typecheck.rs` の `resolve` が配列レシーバの `.push` を
      `push_of` / `push_receiver` で直接そこへ解決する。ユーザーの `impl` は
      1つも要らない。
- [x] 1.3 本体の種別ではなく式の種別で分岐するので、各パスに1アームずつ:
      要求推論(`src/requirement.rs`)・ambient ABI 計画
      (`src/ambient_abi.rs`)は部分式だけを歩き、要求も提供も増やさない。
      所有権計画(`src/ownership.rs`)はレシーバを排他、引数を move-once に
      する。具体化(MAP-040)は組み込みが generic 宣言でないので触らない。
- [x] 1.4 typecheck 焦点テスト:`push契約のtraitを宣言できる` /
      `pushはimplを書かずに複数の要素型で解決する`(`[int]` と `[str]`)/
      `pushはambient要求を増やさない`。

## 2. レシーバ修飾の例外と所有権

- [x] 2.1 `push_receiver`(`src/typecheck.rs`)が排他借用を1つ挿す。
      呼び出し地点の `&mut` は要らない。挿すのは場所のときだけで、
      共有借用と一時値は断る。
- [x] 2.2 挿した借用はソースに `&mut xs` と書いたときと同じ
      `ExprKind::Access { mode: Mutable }` なので、所有権計画は他の
      `&mut self` 呼び出しと同じ道を通る。引数は `Need::Argument` で
      move-once。
- [x] 2.3 焦点テスト:`pushは修飾なしで排他借用を取る` /
      `借用が生きている間のpushを断る` / `不変な束縛へのpushを断る` /
      `pushした値はmoveされる` / `共有借用と一時値へのpushを報告する` /
      `配列でないpushはこれまでどおり修飾が要る`(無関係な struct の
      `push` メソッドは従来どおり `&mut` を要求する negative test)。

## 3. インタプリタ実行

- [x] 3.1 `src/eval.rs` の `ExprKind::Push` は `Vec::push` 1本。場所は右辺より
      先に解決する(所有権計画の順に合わせる)。
- [x] 3.2 eval テスト:`pushは順序と長さを保つ` / `空の配列へもpushできる` /
      `pushした非copyの値は配列が持つ`。

## 4. Core Wasm 生成

- [x] 4.1 並びごとの `reserve_array(ptr)` を `src/wasm_data.rs` に追加
      (`array_clone` / `array_drop` の隣、`reserve_functions` で glue の
      後ろへ並べる)。`len == capacity` なら capacity 0→1、以降は倍にして
      `alloc` し、生きている bytes を `MemoryCopy` して古い buffer を `free`
      する。要素の書き込みと長さの更新は呼び出し地点が受け持つ。
      `data` が番兵になることは無い(空配列も `alloc(0)` の一意なアドレスを
      持ち、`array_drop` も無条件に `free` している)ので番兵の分岐は無い。
- [x] 4.2 `src/wasm.rs` の `push_element` が `reserve` を呼び、次の席を
      求めて `install_slot` で要素を収め、`len` を1つ進める。`reserve` を
      出すのは `push` が届いた並びだけ(`pushed_layouts`)なので、`push` を
      持たないモジュールのバイト列は変わらない。
- [x] 4.3 `src/wasm_data.rs` の単体テスト `pushの容量は倍々に伸びる`
      (0→1→2→4→8 と、余りがあるときは伸びないこと)/
      `容量を伸ばしても既存の要素は残る`。独立したエンジンに allocator と
      `reserve` だけを載せて走らせる。
- [x] 4.4 `pushで伸ばした記憶は使い回される` — 1ページに縛った有界ループ。
      伸ばして解放した buffer を使い回さなければ trap する。
- [x] 4.5 `pushの割り当てが取れなければtrapする` — 1ページでは伸ばせずに
      trap し、16ページなら同じプログラムが通る。push 固有の失敗値は無い。

## 5. 差分実行

- [x] 5.1 `push-capacity-growth` fixture(`src/differential.rs`)。capacity
      1 から 2 度の倍化境界を越え、capacity 0 の配列への初回 push も通す。
- [x] 5.2 `push-owned-element` fixture。非 `Copy` な struct を `move` で
      押し込み、最終状態を両実行系で照合する。
- [x] 5.3 `cargo test --bin rhodolite differential::` が
      `maintained_corpus_has_matching_observable_outcomes` と
      `every_fixture_is_byte_deterministic_and_independently_executable` を
      無改造で通ることを確認した。

## 6. 仕様と締め

- [x] 6.1 delta spec の scenario と 1〜5 のテストの対応を確認した
      (array-push の 13 scenario、method-call-type-checking の
      `push needs no receiver modifier`、differential-execution の
      push 2 件)。
- [x] 6.2 `cargo test` 全通過(1012 + 125)。
- [x] 6.3 `cargo fmt --check` と `cargo clippy --all-targets -- -D warnings`
      が通る。
- [x] 6.4 新 fixture の連続2回のビルドが byte 単位で一致することを
      `every_fixture_is_byte_deterministic_and_independently_executable` で
      確認した。
- [x] 6.5 安定状態を単独のスナップショットとしてコミットした。
- [ ] 6.6 `array-push` / `method-call-type-checking` /
      `differential-execution` の delta spec を `openspec/specs/` へ sync し、
      ROADMAP.md の MAP-075 を `done` にする(後続ステップ)。
