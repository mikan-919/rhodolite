# Ruby / JavaScript の設計参考調査

調査日: 2026-08-01（Asia/Tokyo）

対象: Ruby 4.1 系の公式 RDoc と MRI の公式ソース、ECMAScript Language
Specification（ECMA-262）、Mozilla の MDN、WebAssembly JavaScript Interface
Specification。各リンクは、記載した挙動を直接説明する一次資料である。

ここでいう「採用」は、Ruby/JavaScript の機能をそのまま移植することではなく、
Rhodolite の制約（ambient bindings、trait、第二級ブロック、whole-program
monomorphization、Core Wasm）に変換できる設計パターンを指す。「評価」はこのリポジトリの
既存文書からの推論であり、仕様そのものではない。

## 結論

Ruby からは、暗黙ブロックを使う iterator API、`Enumerable` の小さな中核プロトコル、
`with` に似た動的スコープの「入ると置き換え、出ると復元する」という操作感を参考にできる。
一方、非 lambda `Proc` の非局所 `return`/`break`、`method_missing`/`eval`、オープンクラス、
暗黙の thread/fiber-local は、静的要求推論と Wasm の決定性を壊すので移植しない。

JavaScript からは、語彙的クロージャ、静的 module import/export、`Iterator` の
`next() -> {value, done}` という境界、Wasm の明示的 import/export を参考にできる。
`this` の呼び出し形依存、prototype chain の実行時変更、Promise の job queue、
`WeakRef`/`FinalizationRegistry` の非決定的 GC、ホスト例外の透過伝播は core 言語へ入れず、
必要なら標準 trait または JS adapter の外側へ隔離する。

| 論点 | 取り込める要素 | 取り込まない要素 | Rhodolite での着地点 |
|---|---|---|---|
| ブロック/closure | 語彙環境を捕捉する通常の closure、iterator の body block | 非局所脱出を持つ非 lambda `Proc`、可変 arity | 第二級ブロックは現在のまま。first-class function は明示的な関数値として lowering |
| ambient / context | 動的スコープの push/pop と fiber 境界の明確化 | hidden global/thread-local/fiber-local | `with` が作る ambient record を静的に要求推論し、退出時に復元 |
| module / trait | namespace と小さな iterable protocol | open class、prototype/monkey patch、実行時 duck typing | `use`、nominal struct、静的 trait 解決 |
| 失敗/cleanup | `ensure`/`finally` の必ず実行される cleanup | 型に出ない例外、GC finalizer を資源解放に使うこと | explicit error channel と `defer`/cleanup lowering（仕様化時に別途決定） |
| iteration / async | `each`/`Iterator` を trait にする、外部 iterator は状態機械にする | implicit Promise scheduling、generator の任意 continuation | 同期 iterator を優先。async は effect/Task を導入する場合だけ明示 |
| Wasm 境界 | imports/exports を capability 境界として利用 | JS host へ任意の object/function を渡すこと | Core Wasm は import-free（[ADR-0009](adr/0009-core-wasm-is-the-compiler-artifact.md)）、adapter で検証済み ABI を接続 |

## Rhodolite の比較対象（調査の前提）

既存 ADR では、ambient は名前付き slot として宣言し、要求は呼び出しを越えて推論する
（[ADR-0002](adr/0002-traits-not-effects.md)）。`with db(value) { ... }` は本体へ入る前に値を
評価し、内側の slot を作って本体終了時に外側へ戻す
（[ADR-0005](adr/0005-with-provision-and-head-blocks.md)）。要求は whole-program の
specialization で具体的な実装へ埋め込まれ、実行時 vtable/ambient 引数は残さない
（[ADR-0003](adr/0003-whole-program-monomorphization.md)、
[ADR-0008](adr/0008-ambient-abi-is-a-specialization-plan.md)）。成果物はホスト import を
要求しない Core Wasm と ABI metadata である（[ADR-0009](adr/0009-core-wasm-is-the-compiler-artifact.md)）。

したがって、Ruby/JavaScript の機能を評価するときは「実行時に名前を引き直す仕組みを
導入しても、静的な要求集合と単相化後の ABI を保てるか」を判定軸にした。

## Ruby

### 1. block、Proc、lambda

Ruby の block は通常の positional argument ではなく、メソッド呼び出しに付随する一つの
暗黙 block である。実装者は `yield` で直接呼ぶか、末尾の `&block` で `Proc` に
変換して渡せる。`Proc` は保存・引数渡し・呼び出しができ、作成時の語彙コンテキストを
closure として保持する。([methods / Block Argument](https://docs.ruby-lang.org/en/master/syntax/methods_rdoc.html#label-Block+Argument),
[Proc](https://docs.ruby-lang.org/en/master/Proc.html))

`lambda`/`->` はメソッドに近い引数検査と局所的な `return`/`break` を持つ。一方、通常の
`proc` は引数不足を `nil` で埋め、余分な引数を捨て、`return` は包んでいるメソッドへ、
`break` は block を与えたメソッドへ非局所脱出する。対象メソッドがすでに戻っている
場合は `LocalJumpError` になる。([Proc: Lambda and non-lambda semantics](https://docs.ruby-lang.org/en/master/Proc.html#label-Lambda+and+non-lambda+semantics))

Rhodolite への含意:

- **採用:** block 本体を語彙的な第二級ブロックとして扱い、`if`/`match`/`with` と同じ
  本体値・局所束縛の規則にする。Ruby の `yield` という「呼び出し側で body を渡し、
  受け側は名前を付けずに実行できる」操作感は、iterator の糖衣として有用である。
- **注意:** `Proc` 化すると closure 環境、escape、再入、例外/早期脱出の ABI が必要になる。
  second-class block と first-class function value を同一表現にしない。特に block から
  `return` が関数境界を越えられる仕様は、whole-program の関数 instance と Wasm stack
  の対応を複雑にする。
- **不採用:** 非 lambda `Proc` の柔軟な arity と non-local control を移植しない。静的な
  関数型では arity と脱出先を検査時に固定でき、Rhodolite の型付き HIR/要求経路に
  `LocalJumpError` 相当の実行時穴を作らずに済む。これは設計上の評価である。

### 2. dynamic scope、Thread-local、Fiber-local

Ruby の通常のローカル変数は block/メソッドの規則に従うが、`Thread#[]` は実際には
**fiber-local** storage であり、同じ thread の別 Fiber からは見えない。真の thread-local
は `thread_variable_get`/`thread_variable_set` であり、Fiber を跨いで共有される。
公式ドキュメントは、fiber-local を使うと「一時的な値を設定して block を yield し、
ensure で元へ戻す」dynamic-scope idiom を Fiber 切替時にも漏らさず実装できると説明する。
([Thread: Fiber-local vs. Thread-local](https://docs.ruby-lang.org/en/master/Thread.html#label-Fiber-local+vs.+Thread-local),
[Thread#thread_variable_get](https://docs.ruby-lang.org/en/master/Thread.html#method-i-thread_variable_get),
[MRI thread local implementation](https://github.com/ruby/ruby/blob/master/thread.c))

Rhodolite への含意:

- **採用:** `with` の意味を「provider を評価 → inner context を push → body → pop/restore」と
  固定する。これは Ruby の dynamic-scope idiom から得られる操作上の教訓であり、
  [ADR-0005](adr/0005-with-provision-and-head-blocks.md) ですでに採用済みの方針と整合する。
- **注意:** Fiber は実行スタックと storage の境界を変える。将来 async/Fiber を入れるなら、
  ambient が (a) lexical closure に捕捉されるのか、(b) task/Fiber ごとに複製されるのか、
  (c) thread 全体で共有されるのかを型/ABI で明示する。Ruby の二種類の local を暗黙に
  まねると、同じ source が scheduler により違う provider を見る。
- **不採用:** `Thread.current[:name]` のような名前付き global storage を ambient の代替に
  しない。静的要求集合から見えず、テスト間の漏れ、並行実行時の非局所 aliasing、Wasm
  adapter が与えた capability の監査不能性を招く。

### 3. module、mixin、trait 相当

Ruby の `Module` は namespace と mix-in の二つを兼ね、`include` された instance method は
class の ancestor chain に入り、`Enumerable` のような共有 API を後から供給する。module/class
は何度でも reopen でき、メソッドの追加・変更・削除が可能である。公式文書自身が、所有者
でない module を reopen すると名前衝突と診断困難なバグになり得ると注意している。
([modules and classes](https://docs.ruby-lang.org/en/master/syntax/modules_and_classes_rdoc.html),
[Module](https://docs.ruby-lang.org/en/master/Module.html))

Rhodolite への含意:

- **採用:** namespace と実装の組み合わせは Rhodolite の `use` + nominal `trait`/`impl` に
  対応する。trait の「必要な operation だけを小さな契約として提示する」という設計は、
  Ruby mixin の利用者が `each` だけを実装して多くの derived method を得る形から学べる。
- **注意:** Ruby の ancestor lookup は順序と reopen 時点に依存する。Rhodolite の trait
  impl はコンパイル時に一意解決し、orphan/重複実装を診断する。method name の衝突を
  実行時の探索で解決しない。
- **不採用:** open class、monkey patch、refinement、runtime `include` を core に入れない。
  これらは呼び出しグラフと単相化 instance がコンパイル後にも変わるため、Wasm の
  静的 function index、要求経路、再現可能なビルドと両立しない。これは Ruby の仕様を
  Rhodolite の目的に照らした評価である。

### 4. 例外、ensure、cleanup

Ruby の `raise` は現在の実行を中断し、対応する `rescue` へ探索して移る。`ensure` は
  例外の有無や rescue の成否にかかわらず実行される。未処理例外は interpreter が
  メッセージを出してプログラムまたは thread を終了させる。([Ruby exceptions](https://docs.ruby-lang.org/en/master/language/exceptions_md.html),
[Ruby FAQ: exception handling](https://www.ruby-lang.org/en/documentation/faq/6/))

Rhodolite への含意:

- **採用:** `ensure` が提供する「すべての脱出経路で cleanup」という契約は、将来の
  resource/IO API と `defer` lowering の設計原則にできる。cleanup は lexical scope に
  紐付け、`with` の provider を抜ける時点で順序を明示する。
- **注意:** Ruby の例外階層は型署名に現れず、`rescue StandardError` のような広い捕捉も
  可能である。Wasm host trap、`return`、`break`、provider の差し替えを一つの hidden
  channel にまとめると要求解析が不健全になる。
- **不採用:** 例外を ambient/effect の代わりに使わない。Rhodolite の future error model
  は、戻り値または明示した error channel と cleanup を分離して設計するのが安全である。
  Ruby の broad rescue や `throw`/`catch` は core ではなく adapter/標準ライブラリに置く。

### 5. GC と ownership の対照

MRI Ruby は mark-and-sweep GC を提供し、世代別 GC（RGenGC）や compaction、write barrier
などを実装する。GC の起動タイミングはプログラムの論理値ではなく runtime policy であり、
拡張作者は `VALUE` を GC から保護し、write barrier 規則を守る必要がある。
([GC module](https://docs.ruby-lang.org/en/master/GC.html),
[Ruby extension guide: generational GC](https://docs.ruby-lang.org/en/master/extension_rdoc.html),
[MRI default GC](https://github.com/ruby/ruby/tree/master/gc/default))

Rhodolite v1 の ADR は ownership/borrowing を表面へ出さず、Core Wasm では別の ABI/heap
方針を取る。Ruby の GC は「安全な手動解放を要求しない」という ergonomics の参考には
なるが、deterministic resource lifetime のモデルではない。

- **採用:** finalizer に意味を持たせず、資源を明示的な scope/trait operation として閉じる。
  GC がいつ動いても observable な cleanup が変わらないようにする。
- **注意:** closure、Proc、module の動的メソッドが値を長く保持するため、Ruby のメモリ
  使用量を ownership の代替指標にしない。Wasm backend の linear-memory representation
  と外部 host object の寿命を分けて測る。
- **不採用:** MRI の conservative/generational GC や C API の write barrier を Rhodolite
  の表面言語仕様へ移植しない。Wasm の determinism と単相化済み data layout に対して、
  それは実装詳細であり、portable semantic contract ではない。

### 6. DSL / metaprogramming

Ruby の DSL は block と receiver を組み合わせる。`instance_eval`/`instance_exec` は block
実行時の `self` を差し替え、private method/instance variable まで参照できる。`define_method`
は Proc/Method を instance method として install し、`method_missing` は未知の message を
動的に処理する。([BasicObject: instance_eval/instance_exec](https://docs.ruby-lang.org/en/master/BasicObject.html#method-i-instance_eval),
[Module#define_method](https://docs.ruby-lang.org/en/master/Module.html#method-i-define_method),
[BasicObject: method_missing](https://docs.ruby-lang.org/en/master/BasicObject.html#method-i-method_missing),
[Kernel#eval and Binding](https://docs.ruby-lang.org/en/master/Kernel.html#method-i-eval))

- **採用:** block を受ける builder API は、Rhodolite の `with`/head block の読みやすさを
  検証する材料になる。設定を構築する場合も、必要な trait/slot を引数または `with` に
  明示し、body の lexical scope を保つ。
- **注意:** receiver を差し替える DSL は「見えている名前」がソースから分からない。
  ambient slot の名前解決（通常の local が slot を隠す）と同じ規則を維持し、magic `self`
  は導入しない。
- **不採用:** `eval`、`instance_eval`、`method_missing`、動的 `define_method` を core に
  入れない。全プログラムを読んで呼び出しと要求を解決する前提を破り、未検査の host
  capability や任意コード生成を許すためである。

### 7. Enumerable と iterator

`Enumerable` は `each` を実装する class に mix-in すると、`map`、`filter`、`reduce`、
`take` など多数の derived operation を提供する。`Enumerator` は internal iteration
（caller の block を iterator が駆動）と external iteration（caller が `next` を呼ぶ）を
分け、external 版は `StopIteration` で終了する。([Enumerable](https://docs.ruby-lang.org/en/master/Enumerable.html),
[Enumerable usage](https://docs.ruby-lang.org/en/master/Enumerable.html#label-Usage),
[Enumerator](https://docs.ruby-lang.org/en/master/Enumerator.html))

- **採用:** `Each<T>` のような trait を最小の必須 operation とし、`map`/`filter`/`fold` を
  derived な通常関数にする。これは Rhodolite の「trait は契約、ambient は slot」という
  分離を保ったまま、標準ライブラリを小さくできる。
- **注意:** Ruby の block は `break`/`next` で iterator の外へ制御を返せる。Rhodolite の
  block は body value と lexical return に限定し、必要なら `Result`/明示状態機械で
  early stop を表す。
- **不採用:** `to_proc` による任意 object の iterator 化や duck-typed `each` を自動採用
  しない。trait 実装を静的に解決し、Wasm backend が element layout と呼び出し署名を
  決められるようにする。

## JavaScript / ECMAScript

### 1. lexical closure と `this`

ECMAScript は Lexical Environment と Environment Record の連鎖で識別子を解決する。
function は作成時の周囲の lexical environment を保持するため、外側関数が戻った後も
captured binding を使える。`let`/`const` は block scope、module scope も closure の対象で
ある。([ECMA-262 Lexical Environments](https://tc39.es/ecma262/#sec-lexical-environments),
[MDN Closures](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Closures))

通常の function の `this` は lexical 変数ではなく、呼び出し形（`obj.m()`、裸の call、
`call/apply/bind`）で決まる hidden binding である。arrow function だけは enclosing context
の `this` を capture し、新しい `this` binding を作らない。([ECMA-262 This Resolution](https://tc39.es/ecma262/#sec-resolve-this-binding),
[MDN `this`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/this))

Rhodolite への含意:

- **採用:** lexical closure の「関数値 + capture 環境」というモデルを Wasm closure の
  lowering（environment record と code pointer）に使う。ただし capture は型検査後に
  列挙し、ambient slot は lexical capture と別に要求解析する。
- **注意:** closure が module の imported live binding を読む場合、binding の値変更が
  closure から観測できる。Rhodolite は immutable/local shadowing と specialization を
  前提にするため、live mutable import を追加するなら aliasing と再コンパイル境界を
  先に定義する。
- **不採用:** JavaScript の dynamic `this` を暗黙 receiver にしない。呼び出し構文だけで
  結果が変わるため、trait method の一意解決、要求経路、単相化された Wasm function
  signature を壊す。receiver は Rhodolite の `self` 引数のように明示する。

### 2. modules、live binding、依存グラフ

ES modules の `import`/`export` は module の top-level に置き、module は strict mode で
一度だけ評価される。imported binding は live binding であり、cycle は dependency graph
上で許されるが、初期化前に読むと `ReferenceError` になる。([ECMA-262 Modules](https://tc39.es/ecma262/#sec-modules),
[Module Environment Records](https://tc39.es/ecma262/#sec-module-environment-records),
[MDN JavaScript modules](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Modules))

- **採用:** Rhodolite の `use` を module boundary として明示し、module export を公開 API
  の単位にする。JS の top-level import/export のように、依存を本文から発見可能にする
  のは要求グラフと診断に有益である。
- **注意:** JS の cycle/live binding は実行時初期化順序を持つ。Rhodolite は whole-program
  なので cycle を拒否するか、SCC 単位で宣言だけを先に固定するかを決める必要がある。
  live mutable binding を黙って再現すると、単相化後の instance cache が無効になる。
- **不採用:** dynamic `import()`、global object への export、module evaluation の副作用を
  ambient capability として暗黙に数えない。必要なら `ModuleLoader`/`Host` trait を明示する。

### 3. prototype chain と metaprogramming

JS object は own property と `[[Prototype]]` へのリンクを持ち、property lookup は chain を
辿る。prototype やその chain は実行時に変更でき、静的 dispatch は仕様上存在しない。
動的変更は engine の最適化を deoptimize し得る。([ECMA-262 Ordinary Objects](https://tc39.es/ecma262/#sec-ordinary-object-internal-methods-and-internal-slots),
[MDN Inheritance and the prototype chain](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Inheritance_and_the_prototype_chain))

- **採用:** property lookup と method call を分けて考える教材として使う。Rhodolite では
  lookup を nominal field、method call を trait/inherent impl として別の静的規則にする。
- **注意:** field と method を同じ名前空間へ置く JS 流儀は、ambient slot 名を local が
  隠す Rhodolite の規則と混ぜない。値射影/type 射影も明示的に区別する。
- **不採用:** prototype chain、`Object.setPrototypeOf`、Proxy/monkey patch を core に
  入れない。実行時変更で dispatch と layout が変わり、Wasm data layout、whole-program
  monomorphization、trait coherence を保証できない。

### 4. Promise、async/await、exception

`Promise` は将来の fulfillment value または rejection reason を表す object で、`then`/
`catch`/`finally` の handler が状態確定後に実行される。`async function` は常に Promise を
返し、`await` は body を分割して後続を非同期に進める。([ECMA-262 Promise Objects](https://tc39.es/ecma262/#sec-promise-objects),
[ECMA-262 Async Functions](https://tc39.es/ecma262/#sec-async-function-definitions),
[MDN Promise](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Promise),
[MDN async function](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/async_function))

Promise callbacks は ECMAScript の Job/host scheduler と結び付くため、observable な順序は
single-thread の call stack だけでは決まらない。([ECMA-262 Jobs and Job Queues](https://tc39.es/ecma262/#sec-jobs-and-job-queues))

- **採用:** 将来 async を導入するなら、`Task<T>`/`Future<T>` を明示的な trait/effect とし、
  `await` が continuation を保存すること、scheduler と host capability が必要なことを
  型または entrypoint に出す。Promise の `fulfilled/rejected/pending` 三状態は標準
  library data type の参考になる。
- **注意:** `await` は call stack を切り、closure と ambient provider の寿命を跨ぐ。
  specialization 前に捕捉される ambient を確定し、task-local context を導入するなら
  Ruby の Fiber-local と同じく共有範囲を明記する。
- **不採用:** Promise の microtask/job queue を core evaluator や Core Wasm ABI に埋め込ま
  ない。決定的同期 semantics と異なり scheduler、host clock、rejection tracking が
  capability になるため、まず `Async` trait/adapter として隔離する。

ECMAScript の `try`/`catch`/`finally` は例外を捕捉し、`finally` は正常終了・`throw`・
`return`・`break` の前に必ず実行される。([ECMA-262 Exceptions](https://tc39.es/ecma262/#sec-exceptions),
[ECMA-262 Try Statement](https://tc39.es/ecma262/#sec-try-statement),
[MDN try...catch](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/try...catch))
これは Ruby の `ensure` と同じく cleanup の仕様化に参考になるが、例外型が署名に無い点は
Rhodolite の静的要求解析には不向きである。

### 5. Iterator / Generator

JS の iterable は `[Symbol.iterator]()` から iterator を得て、`next()` が
`{ value, done }` を返す。generator function は呼び出しても body を直ちに実行せず、
`next()` ごとに `yield` まで進む suspend/resume 状態機械になる。一つの Generator は通常
一度だけ走査でき、`throw()`/`return()` で suspended context に制御を注入できる。
([ECMA-262 Iteration](https://tc39.es/ecma262/#sec-iteration),
[ECMA-262 Generator Objects](https://tc39.es/ecma262/#sec-generator-objects),
[MDN Iterators and generators](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Iterators_and_generators))

- **採用:** `Iterator<T>` を trait（`next` と終了状態）として定義し、lazy sequence を
  explicit state machine に lower する設計は、Ruby `Enumerator` と合わせて標準ライブラリ
  の有力な参考になる。`{value, done}` は `Option<T>` 相当へ静的に符号化できる。
- **注意:** generator の suspended frame は stack から heap へ逃がれ、captured ambient と
  resource cleanup を長く保持する。`throw()`/任意 continuation まで許すと第二級 block、
  `defer`、Wasm stack unwinding の境界が増える。
- **不採用:** v1 の第二級 block を JS generator の full continuation に拡張しない。必要な
  early stop は `Iterator::next` の返却値や明示した error/state で表現する。generator
  syntax は async と同じく将来の別提案に切り出す。

### 6. GC、WeakRef、FinalizationRegistry

ECMAScript は GC のアルゴリズム・タイミングを規定せず、`WeakRef` と
`FinalizationRegistry` についても、いつ回収・cleanup callback が走るか、または走らない
かは実装依存である。MDN は closure/cache が参照を保持すること、GC の観測可能な挙動に
依存してはならないことを明記する。([ECMA-262 WeakRef Objects](https://tc39.es/ecma262/#sec-weak-ref-objects),
[ECMA-262 FinalizationRegistry Objects](https://tc39.es/ecma262/#sec-finalization-registry-objects),
[MDN WeakRef](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/WeakRef),
[MDN FinalizationRegistry](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/FinalizationRegistry))

- **採用:** GC の非決定性を「普通の値の寿命と resource cleanup を分離すべき」という負の
  参考にする。Core Wasm では explicit ownership/ARC を選ぶ場合でも、GC が意味を持つ
  host object を ABI に漏らさない。
- **不採用:** WeakRef/finalizer を cache invalidation、file/socket close、ambient provider
  の解放条件に使わない。Wasm と JS の backend で観測可能性が変わり、テストと再生可能性
  を失う。

### 7. WebAssembly / JavaScript boundary と host capability

WebAssembly JS Interface は `Module` の import/export descriptor を列挙し、`Instance`
の exports を JS function/object として公開する。Wasm は JS import を同期呼び出しでき、
import 側 JS が throw すると例外は Wasm activation を越えて caller へ伝播する。Wasm trap も
通常は JS exception として外へ出る。([WebAssembly JS Interface Specification](https://webassembly.github.io/spec/js-api/),
[Wasm JS API: imports/exports](https://webassembly.github.io/spec/js-api/#modules),
[Wasm JS API: error condition mappings](https://webassembly.github.io/spec/js-api/#error-condition-mappings-to-javascript),
[MDN WebAssembly concepts](https://developer.mozilla.org/en-US/docs/WebAssembly/Guides/Concepts))

Memory は JS が import/export できる共有 linear buffer であり、function import/export と同じ
明示的な instance construction 境界で渡される。([MDN Using the WebAssembly JavaScript API](https://developer.mozilla.org/en-US/docs/WebAssembly/Guides/Using_the_JavaScript_API),
[MDN WebAssembly.Memory](https://developer.mozilla.org/en-US/docs/WebAssembly/Reference/JavaScript_interface/Memory))

Rhodolite への含意:

- **採用:** import/export descriptor を capability manifest/ABI の検査対象として扱う。
  [ADR-0009](adr/0009-core-wasm-is-the-compiler-artifact.md) の import-free Core Wasm は
  JS embedding の任意 import object より強い境界であり、必要な host API は検証済み
  adapter module に集約できる。
- **注意:** JS object/function/Memory を直接渡すと、prototype mutation、host GC、例外、
  aliasing が Core Wasm の静的保証外へ出る。境界では flat numeric/string ABI、handle の
  所有権、例外→明示 error/trap の変換、capability 名の allow-list を定義する。
- **不採用:** 「Wasm が JS を import できるから」という理由で、Rhodolite source の任意の
  ambient use を JS import へ自動変換しない。要求集合が source/metadata と一致することを
  adapter で検証し、Core Wasm 本体は import-free のままにする。

## 実装に落とす際の優先順位（提案）

1. `Each<T>`/`Iterator<T>` を nominal trait として標準化し、同期の internal iteration と
   external iteration を explicit state machine で提供する。Ruby/JS の二つの API から
   得られる最大の再利用点で、async/GC/host object を必要としない。
2. 第二級 block の body value・local scope・`break`/`return` の脱出範囲を仕様化する。
   Ruby の non-lambda Proc と JS generator は、採用せずに「どこまで禁止するか」のテスト
   ケースとして利用する。
3. `with` と将来 `defer` の相互作用を、正常 return、error、match arm の脱出、nested
   provider の順序でテストする。Ruby `ensure`/JS `finally` の必ず実行される性質だけを
   取り込む。
4. JS adapter の import/export を manifest 検証する。Core Wasm に動的 host object、
   Promise scheduler、finalizer を持ち込まない。
5. async/generator、Fiber/task-local、resource ownership を追加する場合は、別 ADR で
   context の共有範囲と ABI を決めてから実装する。Ruby/JS の実行時 semantics を暗黙に
   借りない。

## 主要一次資料一覧

### Ruby

- [Ruby methods / block arguments](https://docs.ruby-lang.org/en/master/syntax/methods_rdoc.html)
- [Ruby Proc](https://docs.ruby-lang.org/en/master/Proc.html)
- [Ruby Thread](https://docs.ruby-lang.org/en/master/Thread.html)
- [Ruby modules and classes syntax](https://docs.ruby-lang.org/en/master/syntax/modules_and_classes_rdoc.html)
- [Ruby Module API](https://docs.ruby-lang.org/en/master/Module.html)
- [Ruby exceptions](https://docs.ruby-lang.org/en/master/language/exceptions_md.html)
- [Ruby Enumerable](https://docs.ruby-lang.org/en/master/Enumerable.html)
- [Ruby Enumerator](https://docs.ruby-lang.org/en/master/Enumerator.html)
- [Ruby GC](https://docs.ruby-lang.org/en/master/GC.html)
- [Ruby extension / generational GC](https://docs.ruby-lang.org/en/master/extension_rdoc.html)
- [Ruby MRI `proc.c`](https://github.com/ruby/ruby/blob/master/proc.c), [`thread.c`](https://github.com/ruby/ruby/blob/master/thread.c), [`enumerator.c`](https://github.com/ruby/ruby/blob/master/enumerator.c), [`gc/default`](https://github.com/ruby/ruby/tree/master/gc/default)

### JavaScript / WebAssembly

- [ECMA-262 Lexical Environments](https://tc39.es/ecma262/#sec-lexical-environments), [This Resolution](https://tc39.es/ecma262/#sec-resolve-this-binding), [Modules](https://tc39.es/ecma262/#sec-modules), [Module Environment Records](https://tc39.es/ecma262/#sec-module-environment-records)
- [ECMA-262 Ordinary Objects](https://tc39.es/ecma262/#sec-ordinary-object-internal-methods-and-internal-slots)
- [ECMA-262 Promise Objects](https://tc39.es/ecma262/#sec-promise-objects), [Async Functions](https://tc39.es/ecma262/#sec-async-function-definitions), [Jobs and Job Queues](https://tc39.es/ecma262/#sec-jobs-and-job-queues)
- [ECMA-262 Iteration](https://tc39.es/ecma262/#sec-iteration), [Generator Objects](https://tc39.es/ecma262/#sec-generator-objects)
- [ECMA-262 Exceptions](https://tc39.es/ecma262/#sec-exceptions), [Try Statement](https://tc39.es/ecma262/#sec-try-statement)
- [ECMA-262 WeakRef / FinalizationRegistry](https://tc39.es/ecma262/#sec-weak-ref-objects), [FinalizationRegistry](https://tc39.es/ecma262/#sec-finalization-registry-objects)
- [MDN Closures](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Closures), [`this`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Operators/this), [Modules](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Modules), [Prototype chain](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Inheritance_and_the_prototype_chain)
- [MDN Promise](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Promise), [async function](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/async_function), [Iterators and generators](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Guide/Iterators_and_generators), [WeakRef](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/WeakRef), [FinalizationRegistry](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/FinalizationRegistry), [try...catch](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/try...catch)
- [WebAssembly JavaScript Interface Specification](https://webassembly.github.io/spec/js-api/), [MDN WebAssembly concepts](https://developer.mozilla.org/en-US/docs/WebAssembly/Guides/Concepts), [MDN Using the WebAssembly JavaScript API](https://developer.mozilla.org/en-US/docs/WebAssembly/Guides/Using_the_JavaScript_API), [MDN WebAssembly.Memory](https://developer.mozilla.org/en-US/docs/WebAssembly/Reference/JavaScript_interface/Memory)
