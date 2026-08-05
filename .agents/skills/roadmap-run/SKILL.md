---
name: roadmap-run
description: ROADMAP.md から自動実装可能なタスクを1件取り出し、propose → apply → sync+archive をサブエージェントに分担させて完了まで走らせる。ユーザーが「ROADMAP を進めて」「次のタスクを回して」「roadmap-run」と言ったときに使う。
allowed-tools: Read, Edit, Glob, Bash, Agent
---

# roadmap-run

親（このセッション）は Sonnet で動かす前提。実装だけ Opus のサブエージェントに任せる。

## When to Use

- ROADMAP.md の Milestones から次のタスクを1件、自動で最後まで回したいとき
- 「ROADMAP を進めて」「次のタスクを回して」「roadmap-run」と言われたとき

## When NOT to Use

- 特定の change を実装するだけ → `openspec-apply-change` を直接使う
- change が既に存在し、出荷だけしたい → `opsx-ship-change` を使う
- 設計判断が必要なタスク → `openspec-explore` で先に詰める
- ROADMAP を経由しない場当たりの修正 → 通常どおり直接実装する

## 0. 中断からの復帰

`git status` が clean でなければ、何が残っているかを報告して終了する。
`in-progress` のタスクがあれば別のタスクを選ばず、そのタスクの
`openspec/changes/` を見て再開位置を決める。

| 観測した状態 | 再開先 |
|---|---|
| change ディレクトリが無い | 2 |
| ある / `tasks.md` に未チェックが残る | 3 |
| `tasks.md` は全チェック / `openspec/specs/` 未更新 | 4 |
| archive 済み | 5 |

## 1. タスク選択（親が自分でやる）

`ROADMAP.md` の Milestones 表を読み、「自動実装の境界」の条件を**すべて**満たす
最初のタスクを1件だけ選ぶ。条件は ROADMAP の「自動実装の境界」節が正典なので
そこを読んで判定する（状態 `ready` / 依存が全て `done` / 設計判断 `不要` /
完了条件と検証方法が明記されている）。完了条件と検証方法はタスク個別の節と
「共通の完了条件」節に分かれているので、両方読む。

- 該当タスクが無ければ、その旨だけ報告して終了。サブエージェントは起動しない。
- 選んだら状態を `in-progress` にし、**その1行だけを単独でコミットする**。
  このコミットの hash を控える。5 の検証で使う。
- ユーザーがタスクIDを指定していればそれを使う（条件は同じく確認する）。

## 2. propose（サブエージェント / Sonnet）

`Agent` を `subagent_type: "general-purpose"`, `model: "sonnet"` で起動し、
`.claude/skills/openspec-propose/SKILL.md` を読ませてから change を作らせる。
プロンプトにはタスクID・タスク名・完了条件・検証方法をそのまま渡す。
作った change をコミットさせる。

戻ってきたら親が `openspec/changes/` を見て、新規 change ディレクトリが
ちょうど1個できていることを確認する。0個または2個以上なら、タスクを `ready` に
戻して理由を報告し終了する。

## 3. apply（サブエージェント / Opus）

`Agent` を `subagent_type: "general-purpose"`, `model: "opus"` で起動し、
`.claude/skills/openspec-apply-change/SKILL.md` を読ませ、2 の change を実装させる。

必須の指示:
- ROADMAP の「共通の完了条件」を全て満たすこと（テスト、`cargo fmt --check`、
  warning をエラーにした Clippy、インタプリタと Wasm の差分検証、Wasm の byte 決定性）。
- 新しい言語仕様・公開API・データ表現・ランタイム契約の判断が必要になったら
  推測せず、何が未決かを報告して停止すること。
- 完了したら安定状態を単独のスナップショットとしてコミットすること。
- 3回試して完了条件を満たせなければ、状況を報告して停止すること。

## 4. sync + archive（サブエージェント / Sonnet）

`Agent` を `subagent_type: "general-purpose"`, `model: "sonnet"` で起動し、
`.claude/skills/openspec-sync-specs/SKILL.md` →
`.claude/skills/openspec-archive-change/SKILL.md` の順に読ませて実行させ、
コミットさせる。

## 5. 仕上げ（親）

サブエージェントの自己申告は信用しない。親が自分で検証する。

- `cargo test`、`cargo fmt --check`、warning をエラーにした Clippy
- `git status --porcelain` が空であること
- `git log --oneline <1でのhash>..HEAD` に実装と archive のコミットがあること

全て通ったらタスクを `done` にしてコミットし、結果を1〜3行で報告する。

## 中断時の後始末

3 か 4 が停止した、または 5 の検証が落ちた場合:

- 作業ツリーが汚れていれば `wip: <タスクID>` として単独コミットする（捨てない）
- ツリーが clean なら既存コミットはそのまま残す。落ちた検査の名前と出力を
  ROADMAP のタスク節に書き残す（次回 0 からの再開で使う）
- タスクを `blocked` にする。設計判断が要るなら問いを Open Questions に追加する
- 何が起きたかを報告して終了する。以降のステップには進まない
