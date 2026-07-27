# Rhodolite

> **関数呼び出しで切れない束縛を持つ言語。**

変数の束縛は関数を跨ぐと切れる。切れない束縛を言語に入れると、DIコンテナが不要になる。

```rhodolite
effect db: Database                       // スロット宣言
effect clock: Clock

fn stamp(u: User) {                       // 使用
    u.promoted_at = clock.now()
    db.save(u)
}

fn promote(id: int -> bool) {             // 経由するだけ = 無記述
    let u = db.find(id) ?? return false
    stamp(u)
    true
}

fn main() {
    with db(Postgres::new(url)), clock(system_clock) {   // 提供
        promote(user_id)
    }
}
```

`promote` は1文字も書いていないのに、`stamp` の要求が `main` まで届く。
`stamp` に新しい要求が増えても、触るのは `stamp` だけ。

用語の定義は [CONTEXT.md](./CONTEXT.md)、設計判断の理由は [docs/adr/](./docs/adr/)。
v1 の到達目標は [examples/canonical.rd](./examples/canonical.rd)。

## このプロジェクトの成功条件

1. **進学ポートフォリオで語れること** — 設計判断の理由を自分の言葉で説明できる
2. **自分でなるべく作ること** — 前作 iris で「設計者としても実装者としても中途半端」
   になった反省への回答

採用数・エコシステム・実用性は評価軸に**入れない**。判断に迷ったら
「一言で語れるか」と「自分の手で終わらせられるか」の2つで採点する。

## v1 の完了線

[examples/canonical.rd](./examples/canonical.rd) に対して、次の3つが動いたら v1 は終わり。

1. **差し替え** — 本番実装と in-memory 実装を、呼ばれる側を変更せずに入れ替えられる
2. **可視化** — 各関数が要求する trait を推論して表示できる
3. **経路エラー** — 提供忘れを到達経路付きで報告できる(`Clock ← stamp ← promote ← main`)

3つとも、呼び出しグラフを SCC 縮約して逆位相順に1パス舐める処理の副産物になる。

**機能の底**: 正典プログラム1本が動く分だけ実装する。1つも先回りしない。
足りないと判明した時点で1個だけ足す。

## 開発の規則

**中核は手で書く。**

| | 担当 |
|---|---|
| エフェクト推論 / ハンドラ解決 / 到達経路付きエラー生成 | **mikan が手書き** |
| lexer / parser / CLI / テストの定型 | AI 可 |
| 設計相談 / レビュー / 調査 | AI |

語れる部分と手を動かす部分を一致させるための規則。実装言語は Rust、
まずインタプリタから始める(バックエンドは未決のまま後ろに倒す)。

## 現状

v1 の到達目標は達成済み。lexer、parser、要求推論、インタプリタ、CLIがつながり、
`cargo run`で正典プログラムのテストが完走する。現在の地図と次の作業は
[docs/overview.md](./docs/overview.md)。

所有権・借用・`'a` 推論の柱はv1スコープ外として棚上げ中
（[ADR-0001](./docs/adr/0001-v1-scope-effects-only.md)）。
