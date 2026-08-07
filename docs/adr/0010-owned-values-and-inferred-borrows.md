---
status: accepted
date: 2026-08-01
---

# 値は単独所有し、借用は推論する

Rhodolite は、読み取りを簡潔に保ちながら、変更・移譲・複製と破棄の時点をソースで
追える言語にする。ここで決めるのはデータの Wasm 配置ではなく、型付き HIR の後に
確立する所有権契約である。Core Wasm の現在の scalar 境界は
[ADR-0009](./0009-core-wasm-is-the-compiler-artifact.md)のままにする。

## Decision

### 1. 非 Copy 値には所有者が1つだけある

`int`、`bool`、`unit`、fieldless enum と共有借用 `&T` は Copy とする。それ以外の
値は単独所有で、`let next = value` は所有権を移す。深い複製は `value.clone()` だけが
作り、暗黙の複製はしない。

関数シグネチャは所有 `T`、共有借用 `&T`、可変借用 `&mut T` を書く。`&T` 引数への
通常の読み取り呼び出しだけは自動借用する。一方、変更は `&mut place`、既存の非 Copy
local を所有引数または consuming receiver へ渡すには `move place`、複製には
`clone()` を明記する。これにより状態変更、移譲、入力サイズに比例しうる複製を呼び出し
地点で見分けられる。

```rhodolite
fn inspect(user: &User -> int) { user.id }
fn rename(user: &mut User, id: int) { user.id = id }
fn save(user: User) { let ignored = user }

let mut user = User { id = 1 }
inspect(user)             // 共有借用は簡潔
rename(&mut user, 2)      // 変更は明示
let backup = user.clone() // 深い複製は明示
save(move user)           // 移譲は明示
```

### 2. 寿命名は書かず、全プログラムで借用領域と返却元を推論する

借用は CFG 上の最後の使用まで続く最小の領域を取り、借用結果を返す関数は入力 place の
どこから返るかを要約する。分岐と再帰は保守的な固定点で合流する。従ってユーザーは
`'a` を書かないが、use-after-move、重なる共有／可変借用、所有者を越える借用を実行前に
診断できる。

借用可能な場所は local、引数、戻り値に限る。struct、enum、optional、配列の中へ
参照を格納する aggregate borrow はまだ扱わない。異なる struct field は区別するが、
enum payload と動的な配列添字は保守的にコンテナ全体と重なる。

### 3. `indirect` が有限な再帰所有を明示する

再帰する struct field と enum payload は `indirect` を付けた辺を少なくとも一つ持つ。
これは source 上は通常の所有値のまま、有限な配置を作るための明示的な間接化である。
直接の再帰閉路は型検査で拒否する。

```rhodolite
struct Node {
    value: int
    indirect next: Node?
}
```

### 4. drop は決定的で、言語機能として隠さない

コンパイラは scope を出る所有 local を逆宣言順に drop し、move 済みの source は二重に
drop しない。複合値はまだ所有する内容を再帰的に一度だけ drop する。`return`、分岐、
loop の出口も同じ cleanup を通る。trap は言語レベルの unwinding をしない。

Rhodolite に user-defined destructor、finalizer、manual free はない。最後の使用での
早期解放は、その順序が観測不能な最適化としてのみ許す。

### 5. 実行時の所有権機構と脱出口を足さない

GC、reference count、runtime borrow counter、raw pointer、`unsafe` はこの契約に
含めない。インタプリタは所有 compound value を内部 store の location として持ち、
検査済みの reference place だけを辿る。これは意味論の共有所有ではなく、参照実装の
実装詳細である。

Wasm は当初、到達した `unit` / `bool` / `int` だけを生成し、非 scalar 値と borrow を
build 前に拒否していた。その owned data の allocator・layout・rich ABI は
[ADR-0011](./0011-owned-data-layout-and-abi-v1.md) が決めた。公開署名に出た borrow を
拒否することは変わらない。ここで決めた所有・借用・`indirect` の意味論そのものは
ADR-0011 でも動かない。

## Consequences

- 通るプログラムは、後段へ渡る前に所有・借用・drop の計画を持つ
- 通常の read-only API は短く、mutation・move・clone はレビューで見落としにくい
- second-class block は従来どおり外側の local scope を再利用する。借用範囲も波括弧で
  機械的に短縮されない
- aggregate borrow、shared ownership、allocator/data layout、async、rich public ABI は
  意図的に未実装のまま残る
