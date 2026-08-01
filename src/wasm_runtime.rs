//! 生成モジュールへ埋め込む所有権ランタイム(design.md 決定4)。
//!
//! import は1つも要らない。線形メモリ・ブロックヘッダ・空きリスト・
//! `memory.grow` まで全部この場で Core Wasm として書き出す。ここが持つのは
//! 「割り当てと解放」だけで、型ごとの clone/drop は `wasm_data` の仕事。
//!
//! メモリの前置き(決定的):
//!
//! ```text
//! 0..4   アドレス 0 は無効。誰も使わない
//! 4..8   空きリストの先頭ブロック(0 なら空)
//! 8..12  heap の終端。ここまでが allocator の管理下
//! 12..16 最後に予約した ABI 受け渡し領域(tasks 8.2)
//! 16..   静的データ(文字列リテラルなど)
//! ```
//!
//! heap はその後ろを 8 に揃えたところから始まる。ブロックは
//! `{u32 payload の大きさ, u32 空きリストの次}` の 8 byte ヘッダを持ち、
//! payload はその直後。ヘッダが 8 byte で heap の始まりが 8 に揃うので、
//! payload は常に 8 に揃う ― 揃えの上限が 8 である限り、割り当てごとに
//! 揃えを計算しなくてよい(design.md 決定3)。

// ponytail: ランタイムを実際にモジュールへ載せるのは、所有データが到達したとき
// だけ。その下ろしが入るまでは単体テストからしか呼ばれないので、ここだけ許す。
// data 下ろしが入ったらこの許可を外すこと
#![allow(dead_code)]

use crate::wasm_layout::{Overflow, align_to};
use wasm_encoder::{
    BlockType, ConstExpr, DataSection, Function, Instruction, MemArg, MemorySection, MemoryType,
    ValType,
};

/// ブロックヘッダの大きさ。payload の揃えもこれで決まる
pub const HEADER: u32 = 8;

/// payload の最小。これ未満に分割するとヘッダのほうが大きくなる
pub const MIN_PAYLOAD: u32 = 8;

/// 線形メモリ1ページ
pub const PAGE: u32 = 65536;

/// 前置きの語。どれも `u32`
const FREE_HEAD: u32 = 4;
const HEAP_LIMIT: u32 = 8;
pub const EXCHANGE: u32 = 12;

/// 静的データが始まる位置
pub const PREFIX_END: u32 = 16;

/// 32bit に収まるページ数の上限。ここまでなら `pages << 16` が u32 で回らない
const MAX_PAGES: u32 = 0xFFFF;
const MAX_HEAP_END: u32 = MAX_PAGES << 16;

/// ランタイムが出す関数の本数
pub const COUNT: u32 = 4;

/// `base` を先頭にした関数番号。並びは固定
pub const ALLOC: u32 = 0;
pub const FREE: u32 = 1;
const FREE_INSERT: u32 = 2;
const EXTEND: u32 = 3;

/// 4 byte 揃えの load/store。前置きもヘッダも `u32` しか置かない
const WORD: MemArg = MemArg {
    offset: 0,
    align: 2,
    memory_index: 0,
};

/// 出す関数1本。型の内部化は組み立て側(`wasm.rs`)がまとめて行う
pub struct Helper {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
    pub body: Function,
}

/// 静的データと heap の始まりを決めた、モジュール1つ分のランタイム。
pub struct Runtime {
    /// 前置きを含む、そのままメモリの先頭へ置く bytes
    data: Vec<u8>,
    heap_base: u32,
}

impl Runtime {
    /// 静的データ(文字列リテラルなど)を前置きの後ろへ置く。
    ///
    /// `heap_base` はその終わりを 8 に揃えたところ。前置きの `HEAP_LIMIT` に
    /// 同じ値を焼き込むので、start section なしで初期状態が決まる
    pub fn new(static_data: &[u8]) -> Result<Runtime, Overflow> {
        let mut data = vec![0u8; PREFIX_END as usize];
        data.extend_from_slice(static_data);
        let heap_base = align_to(u32::try_from(data.len()).unwrap_or(u32::MAX), HEADER)?;
        // 空きリストは空、heap の終端は heap の始まりと同じ。最初の割り当てで
        // `extend` が1ページぶんまとめて空きブロックにする
        data[FREE_HEAD as usize..][..4].copy_from_slice(&0u32.to_le_bytes());
        data[HEAP_LIMIT as usize..][..4].copy_from_slice(&heap_base.to_le_bytes());
        Ok(Runtime { data, heap_base })
    }

    pub fn heap_base(&self) -> u32 {
        self.heap_base
    }

    /// 静的データが載る最小ページ数。heap は必要になってから伸ばす
    fn pages(&self) -> u64 {
        u64::from(self.heap_base.div_ceil(PAGE)).max(1)
    }

    pub fn memory_section(&self) -> MemorySection {
        let mut section = MemorySection::new();
        section.memory(MemoryType {
            minimum: self.pages(),
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        section
    }

    pub fn data_section(&self) -> DataSection {
        let mut section = DataSection::new();
        section.active(0, &ConstExpr::i32_const(0), self.data.iter().copied());
        section
    }

    /// `base` を先頭の関数番号としたときのランタイム関数。並びは `ALLOC` から
    pub fn helpers(base: u32) -> Vec<Helper> {
        vec![
            Helper {
                params: vec![ValType::I32],
                results: vec![ValType::I32],
                body: alloc(base),
            },
            Helper {
                params: vec![ValType::I32],
                results: vec![],
                body: free(base),
            },
            Helper {
                params: vec![ValType::I32],
                results: vec![],
                body: free_insert(),
            },
            Helper {
                params: vec![ValType::I32],
                results: vec![],
                body: extend(base),
            },
        ]
    }
}

// ---------------------------------------------------------------------------
// 命令の組み立て
// ---------------------------------------------------------------------------

/// 命令を並べるための薄い包み。`Function` を直に触ると読めなくなる
struct Body(Function);

impl Body {
    fn new(locals: u32) -> Body {
        Body(Function::new([(locals, ValType::I32)]))
    }

    fn ins(&mut self, instruction: Instruction<'_>) -> &mut Body {
        self.0.instruction(&instruction);
        self
    }

    fn get(&mut self, local: u32) -> &mut Body {
        self.ins(Instruction::LocalGet(local))
    }

    fn set(&mut self, local: u32) -> &mut Body {
        self.ins(Instruction::LocalSet(local))
    }

    fn num(&mut self, value: u32) -> &mut Body {
        self.ins(Instruction::I32Const(value as i32))
    }

    /// stack のアドレスから `u32` を読む
    fn load(&mut self) -> &mut Body {
        self.ins(Instruction::I32Load(WORD))
    }

    /// stack のアドレスへ `u32` を書く(アドレス、値の順に積んでおく)
    fn store(&mut self) -> &mut Body {
        self.ins(Instruction::I32Store(WORD))
    }

    /// `local` が指すブロックの「次」の場所
    fn next_of(&mut self, local: u32) -> &mut Body {
        self.get(local).num(4).ins(Instruction::I32Add)
    }

    /// `local` が指すブロックの終端(ヘッダ + payload の次)
    fn end_of(&mut self, local: u32) -> &mut Body {
        self.get(local)
            .num(HEADER)
            .ins(Instruction::I32Add)
            .get(local)
            .load()
            .ins(Instruction::I32Add)
    }

    /// 条件が真なら trap する。所有権ランタイムに巻き戻しは無い
    fn trap_if(&mut self) -> &mut Body {
        self.ins(Instruction::If(BlockType::Empty))
            .ins(Instruction::Unreachable)
            .ins(Instruction::End)
    }

    fn finish(mut self) -> Function {
        self.0.instruction(&Instruction::End);
        self.0
    }
}

/// `alloc(size) -> ptr`。first-fit で空きリストから取り、無ければ heap を伸ばす
fn alloc(base: u32) -> Function {
    // 1: prev, 2: cur, 3: rest, 4: split
    let mut b = Body::new(4);

    // 8 へ丸める前にあふれを見る。ここを抜ければ以降の加算は安全
    b.get(0).num(u32::MAX - HEADER - 7).ins(Instruction::I32GtU);
    b.trap_if();
    b.get(0).num(7).ins(Instruction::I32Add);
    b.num(!7u32).ins(Instruction::I32And).set(0);
    // 0 byte の要求にも一意なアドレスを返す。最小 payload まで押し上げる
    b.get(0).ins(Instruction::I32Eqz);
    b.ins(Instruction::If(BlockType::Empty))
        .num(MIN_PAYLOAD)
        .set(0)
        .ins(Instruction::End);

    b.ins(Instruction::Loop(BlockType::Empty));
    b.num(0).set(1);
    b.num(FREE_HEAD).load().set(2);
    b.ins(Instruction::Block(BlockType::Empty));
    b.ins(Instruction::Loop(BlockType::Empty));

    // 空きリストを使い切ったら heap を伸ばしてやり直す
    b.get(2).ins(Instruction::I32Eqz).ins(Instruction::BrIf(1));

    b.get(2).load().get(0).ins(Instruction::I32GeU);
    b.ins(Instruction::If(BlockType::Empty));
    {
        // 余りが正しいブロックになるときだけ分ける
        b.get(2).load().get(0).ins(Instruction::I32Sub).set(3);
        b.get(3)
            .num(HEADER + MIN_PAYLOAD)
            .ins(Instruction::I32GeU)
            .ins(Instruction::If(BlockType::Empty));
        {
            b.get(2)
                .num(HEADER)
                .ins(Instruction::I32Add)
                .get(0)
                .ins(Instruction::I32Add)
                .set(4);
            b.get(4).get(3).num(HEADER).ins(Instruction::I32Sub).store();
            b.next_of(4).next_of(2).load().store();
            b.get(2).get(0).store();
            // 空きリストの位置は分割した後ろ半分が引き継ぐ
            relink(&mut b, 4);
        }
        b.ins(Instruction::Else);
        {
            // 分けられないので丸ごと外す
            b.next_of(2).load().set(4);
            relink(&mut b, 4);
        }
        b.ins(Instruction::End);
        b.get(2)
            .num(HEADER)
            .ins(Instruction::I32Add)
            .ins(Instruction::Return);
    }
    b.ins(Instruction::End);

    b.get(2).set(1);
    b.next_of(2).load().set(2);
    b.ins(Instruction::Br(0));
    b.ins(Instruction::End); // 走査ループ
    b.ins(Instruction::End); // 使い切った

    b.get(0)
        .num(HEADER)
        .ins(Instruction::I32Add)
        .ins(Instruction::Call(base + EXTEND));
    b.ins(Instruction::Br(0));
    b.ins(Instruction::End); // やり直しループ

    // ループは必ず return するか頭へ戻る。ここは到達しない
    b.ins(Instruction::Unreachable);
    b.finish()
}

/// 空きリストで `cur`(local 2)が居た場所へ `local` の値を繋ぎ直す
fn relink(b: &mut Body, local: u32) {
    b.get(1).ins(Instruction::I32Eqz);
    b.ins(Instruction::If(BlockType::Empty));
    b.num(FREE_HEAD).get(local).store();
    b.ins(Instruction::Else);
    b.next_of(1).get(local).store();
    b.ins(Instruction::End);
}

/// `free(ptr)`。ヘッダへ戻してから空きリストへ返す
fn free(base: u32) -> Function {
    let mut b = Body::new(0);
    // 0 は無効アドレス。掃除は初期化フラグで守るので本来来ない
    b.get(0).ins(Instruction::I32Eqz);
    b.ins(Instruction::If(BlockType::Empty))
        .ins(Instruction::Return)
        .ins(Instruction::End);
    b.get(0)
        .num(HEADER)
        .ins(Instruction::I32Sub)
        .ins(Instruction::Call(base + FREE_INSERT));
    b.finish()
}

/// 空きリストへアドレス順に挿し、前後が隣接していれば併合する
fn free_insert() -> Function {
    // 1: prev, 2: cur
    let mut b = Body::new(2);

    b.num(0).set(1);
    b.num(FREE_HEAD).load().set(2);
    b.ins(Instruction::Block(BlockType::Empty));
    b.ins(Instruction::Loop(BlockType::Empty));
    b.get(2).ins(Instruction::I32Eqz).ins(Instruction::BrIf(1));
    b.get(2).get(0).ins(Instruction::I32GeU);
    b.ins(Instruction::BrIf(1));
    b.get(2).set(1);
    b.next_of(2).load().set(2);
    b.ins(Instruction::Br(0));
    b.ins(Instruction::End);
    b.ins(Instruction::End);

    b.next_of(0).get(2).store();
    relink(&mut b, 0);

    // 後ろと併合。`cur` が自分の終端そのものなら1つのブロックにする
    b.get(2).ins(Instruction::If(BlockType::Empty));
    {
        b.end_of(0).get(2).ins(Instruction::I32Eq);
        b.ins(Instruction::If(BlockType::Empty));
        b.get(0)
            .get(0)
            .load()
            .num(HEADER)
            .ins(Instruction::I32Add)
            .get(2)
            .load()
            .ins(Instruction::I32Add)
            .store();
        b.next_of(0).next_of(2).load().store();
        b.ins(Instruction::End);
    }
    b.ins(Instruction::End);

    // 前と併合
    b.get(1).ins(Instruction::If(BlockType::Empty));
    {
        b.end_of(1).get(0).ins(Instruction::I32Eq);
        b.ins(Instruction::If(BlockType::Empty));
        b.get(1)
            .get(1)
            .load()
            .num(HEADER)
            .ins(Instruction::I32Add)
            .get(0)
            .load()
            .ins(Instruction::I32Add)
            .store();
        b.next_of(1).next_of(0).load().store();
        b.ins(Instruction::End);
    }
    b.ins(Instruction::End);

    b.finish()
}

/// `extend(need)`。管理下の heap を最低限のページ数だけ伸ばして空きへ足す
fn extend(base: u32) -> Function {
    // 1: limit, 2: end, 3: total
    let mut b = Body::new(3);

    b.num(HEAP_LIMIT).load().set(1);
    b.get(1).get(0).ins(Instruction::I32Add).set(3);
    // 加算が 32bit で回ったら、そのアドレスは存在しない
    b.get(3).get(1).ins(Instruction::I32LtU);
    b.trap_if();
    // 4GiB は Core Wasm の線形メモリの上限。ここを越える要求は通せない
    b.get(3).num(MAX_HEAP_END).ins(Instruction::I32GtU);
    b.trap_if();

    b.ins(Instruction::MemorySize(0))
        .num(16)
        .ins(Instruction::I32Shl)
        .set(2);
    b.get(3).get(2).ins(Instruction::I32GtU);
    b.ins(Instruction::If(BlockType::Empty));
    {
        b.get(3)
            .get(2)
            .ins(Instruction::I32Sub)
            .num(PAGE - 1)
            .ins(Instruction::I32Add)
            .num(16)
            .ins(Instruction::I32ShrU)
            .ins(Instruction::MemoryGrow(0));
        b.num(u32::MAX).ins(Instruction::I32Eq);
        b.trap_if();
        b.ins(Instruction::MemorySize(0))
            .num(16)
            .ins(Instruction::I32Shl)
            .set(2);
    }
    b.ins(Instruction::End);

    // 伸ばしたぶんは端数も含めて丸ごと1つの空きブロックにする
    b.get(1)
        .get(2)
        .get(1)
        .ins(Instruction::I32Sub)
        .num(HEADER)
        .ins(Instruction::I32Sub)
        .store();
    b.num(HEAP_LIMIT).get(2).store();
    b.get(1).ins(Instruction::Call(base + FREE_INSERT));
    b.finish()
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, FunctionSection, Module, TypeSection,
    };

    /// allocator だけを載せて `alloc` / `free` / `memory` を外へ出したモジュール。
    ///
    /// 生成器の知識を共有しないエンジンで走らせるので、通ることが
    /// 「この allocator は本当に Core Wasm として正しい」の証明になる
    fn module(static_data: &[u8]) -> (Vec<u8>, u32) {
        let runtime = Runtime::new(static_data).expect("前置きは 32bit に収まる");
        let helpers = Runtime::helpers(0);

        let mut types = TypeSection::new();
        let mut functions = FunctionSection::new();
        let mut code = CodeSection::new();
        for (index, helper) in helpers.iter().enumerate() {
            types
                .ty()
                .function(helper.params.clone(), helper.results.clone());
            functions.function(index as u32);
            code.function(&helper.body);
        }

        let mut exports = ExportSection::new();
        exports.export("alloc", ExportKind::Func, ALLOC);
        exports.export("free", ExportKind::Func, FREE);
        exports.export("memory", ExportKind::Memory, 0);

        let mut module = Module::new();
        module.section(&types);
        module.section(&functions);
        module.section(&runtime.memory_section());
        module.section(&exports);
        module.section(&code);
        module.section(&runtime.data_section());
        (module.finish(), runtime.heap_base())
    }

    /// エンジンを1つ立てて、allocator を叩ける手を返す
    struct Heap {
        store: wasmi::Store<()>,
        alloc: wasmi::Func,
        free: wasmi::Func,
        memory: wasmi::Memory,
    }

    impl Heap {
        fn new() -> Heap {
            Heap::with_data(&[])
        }

        fn with_data(static_data: &[u8]) -> Heap {
            let (bytes, _) = module(static_data);
            crate::wasm::validate(&bytes).expect("検証を通るはず");
            let engine = wasmi::Engine::default();
            let compiled = wasmi::Module::new(&engine, &bytes).expect("読めるはず");
            let mut store = wasmi::Store::new(&engine, ());
            let instance = wasmi::Linker::new(&engine)
                .instantiate_and_start(&mut store, &compiled)
                .expect("立ち上がるはず");
            let alloc = instance.get_func(&store, "alloc").unwrap();
            let free = instance.get_func(&store, "free").unwrap();
            let memory = instance.get_memory(&store, "memory").unwrap();
            Heap {
                store,
                alloc,
                free,
                memory,
            }
        }

        fn alloc(&mut self, size: u32) -> u32 {
            let mut out = [wasmi::Val::I32(0)];
            self.alloc
                .call(
                    &mut self.store,
                    &[wasmi::Val::I32(size as i32)],
                    &mut out[..],
                )
                .expect("割り当てられるはず");
            match out[0] {
                wasmi::Val::I32(n) => n as u32,
                ref other => panic!("アドレスではない: {other:?}"),
            }
        }

        fn try_alloc(&mut self, size: u32) -> Result<u32, String> {
            let mut out = [wasmi::Val::I32(0)];
            self.alloc
                .call(
                    &mut self.store,
                    &[wasmi::Val::I32(size as i32)],
                    &mut out[..],
                )
                .map_err(|e| e.to_string())?;
            match out[0] {
                wasmi::Val::I32(n) => Ok(n as u32),
                ref other => panic!("アドレスではない: {other:?}"),
            }
        }

        fn free(&mut self, ptr: u32) {
            self.free
                .call(&mut self.store, &[wasmi::Val::I32(ptr as i32)], &mut [])
                .expect("解放できるはず");
        }

        fn word(&self, address: u32) -> u32 {
            let mut buf = [0u8; 4];
            self.memory
                .read(&self.store, address as usize, &mut buf)
                .expect("読めるはず");
            u32::from_le_bytes(buf)
        }

        fn write(&mut self, address: u32, bytes: &[u8]) {
            self.memory
                .write(&mut self.store, address as usize, bytes)
                .expect("書けるはず");
        }

        fn read(&self, address: u32, len: usize) -> Vec<u8> {
            let mut buf = vec![0u8; len];
            self.memory
                .read(&self.store, address as usize, &mut buf)
                .expect("読めるはず");
            buf
        }

        fn pages(&self) -> u32 {
            self.memory.size(&self.store) as u32
        }

        /// 空きリストを頭から辿った `(ブロック, payload の大きさ)` の並び
        fn free_list(&self) -> Vec<(u32, u32)> {
            let mut found = Vec::new();
            let mut cur = self.word(FREE_HEAD);
            while cur != 0 {
                found.push((cur, self.word(cur)));
                cur = self.word(cur + 4);
                assert!(found.len() < 64, "空きリストが輪になっている");
            }
            found
        }
    }

    // -----------------------------------------------------------------------
    // 前置きと初期状態(tasks 2.1・2.2)
    // -----------------------------------------------------------------------

    #[test]
    fn 前置きは焼き込んだ初期状態から始まる() {
        let heap = Heap::new();
        assert_eq!(heap.word(0), 0, "アドレス 0 は無効のまま");
        assert_eq!(heap.word(FREE_HEAD), 0, "空きリストは空");
        assert_eq!(heap.word(HEAP_LIMIT), PREFIX_END, "heap はまだ伸びていない");
    }

    /// 静的データは前置きの後ろに置かれ、heap はその先から 8 に揃えて始まる
    #[test]
    fn 静的データの後ろから8に揃えてheapが始まる() {
        let (_, heap_base) = module(b"hello");
        assert_eq!(heap_base, align_to(PREFIX_END + 5, HEADER).unwrap());
        let mut heap = Heap::with_data(b"hello");
        assert_eq!(heap.read(PREFIX_END, 5), b"hello");
        assert_eq!(heap.word(HEAP_LIMIT), heap_base);
        assert!(heap.alloc(8) >= heap_base);
    }

    /// 返るアドレスは非 0 で 8 に揃う。揃えの上限が 8 なのでこれで足りる
    #[test]
    fn 返るアドレスは非0で8に揃う() {
        let mut heap = Heap::new();
        for size in [0, 1, 7, 8, 9, 100, 1000] {
            let ptr = heap.alloc(size);
            assert_ne!(ptr, 0, "{size}");
            assert_eq!(ptr % HEADER, 0, "{size}");
        }
    }

    /// 生きている割り当ては重ならない。書いた中身が他から見えない
    #[test]
    fn 生きている割り当ては重ならない() {
        let mut heap = Heap::new();
        let ptrs: Vec<u32> = (0..8).map(|_| heap.alloc(24)).collect();
        for (index, ptr) in ptrs.iter().enumerate() {
            heap.write(*ptr, &[index as u8; 24]);
        }
        for (index, ptr) in ptrs.iter().enumerate() {
            assert_eq!(heap.read(*ptr, 24), vec![index as u8; 24], "{index}");
        }
    }

    /// 分割は「余りが正しいブロックになる」ときだけ。端数は要求へ含める
    #[test]
    fn 分割は余りが正しいブロックになるときだけ() {
        let mut heap = Heap::new();
        let big = heap.alloc(256);
        heap.free(big);
        assert_eq!(heap.free_list().len(), 1, "1つに戻る");

        // ヘッダ + 最小 payload を残せるので分かれる
        let small = heap.alloc(16);
        assert_eq!(heap.word(small - HEADER), 16);
        assert_eq!(heap.free_list().len(), 1);

        // 残りぴったりを頼むと分かれずに丸ごと出る
        let rest = heap.free_list()[0].1;
        let all = heap.alloc(rest);
        assert_eq!(heap.word(all - HEADER), rest);
        assert!(heap.free_list().is_empty(), "空きは無くなる");
    }

    // -----------------------------------------------------------------------
    // 併合と再利用(tasks 2.3)
    // -----------------------------------------------------------------------

    /// 解放はアドレス順に挿さる。順不同で返しても並びは昇順
    #[test]
    fn 解放はアドレス順に並ぶ() {
        let mut heap = Heap::new();
        let ptrs: Vec<u32> = (0..4).map(|_| heap.alloc(32)).collect();
        // 隣り合わないように1つおきに返す。併合されずに4つ並ぶ形にはしない
        heap.free(ptrs[2]);
        heap.free(ptrs[0]);
        let addresses: Vec<u32> = heap.free_list().iter().map(|(a, _)| *a).collect();
        let mut sorted = addresses.clone();
        sorted.sort_unstable();
        assert_eq!(addresses, sorted);
    }

    /// 隣り合う空きは1つに戻る。前からでも後ろからでも同じ
    #[test]
    fn 隣り合う空きは併合される() {
        for order in [[0usize, 1, 2], [2, 1, 0], [1, 0, 2], [1, 2, 0]] {
            let mut heap = Heap::new();
            let ptrs: Vec<u32> = (0..3).map(|_| heap.alloc(32)).collect();
            let before = heap.free_list().len();
            for index in order {
                heap.free(ptrs[index]);
            }
            assert_eq!(heap.free_list().len(), before, "{order:?} で断片が残った");
        }
    }

    /// 解放して同じ大きさを頼めば同じ場所が返る。使い回せている証拠
    #[test]
    fn 解放した場所は次の割り当てで再利用される() {
        let mut heap = Heap::new();
        let first = heap.alloc(64);
        heap.free(first);
        assert_eq!(heap.alloc(64), first);
    }

    /// 有界なループで割り当てと解放を繰り返してもメモリは増えない。
    ///
    /// bump allocator を却下した理由がこれ(design.md 決定4)
    #[test]
    fn 有界なループはメモリを増やさない() {
        let mut heap = Heap::new();
        heap.alloc(1000);
        let pages = heap.pages();
        for _ in 0..2000 {
            let ptr = heap.alloc(200);
            heap.free(ptr);
        }
        assert_eq!(heap.pages(), pages, "解放したぶんを使い回していない");
    }

    /// 断片化しても、併合できる形なら大きい要求がまた通る
    #[test]
    fn 併合すれば大きい要求がまた通る() {
        let mut heap = Heap::new();
        let ptrs: Vec<u32> = (0..16).map(|_| heap.alloc(64)).collect();
        let pages = heap.pages();
        for ptr in &ptrs {
            heap.free(*ptr);
        }
        // 16 × (64 + ヘッダ) を1つに戻せているので、伸ばさずに収まる
        heap.alloc(64 * 16);
        assert_eq!(heap.pages(), pages);
    }

    // -----------------------------------------------------------------------
    // 成長と失敗(tasks 2.4)
    // -----------------------------------------------------------------------

    /// 1ページに収まらない要求はページを足す。足すのは必要な最小限
    #[test]
    fn 収まらない要求はページを足す() {
        let mut heap = Heap::new();
        assert_eq!(heap.pages(), 1);
        heap.alloc(PAGE * 3);
        assert_eq!(heap.pages(), 4, "3ページぶん + 元の1ページ");
        // 伸ばした先も普通に読み書きできる
        let ptr = heap.alloc(16);
        heap.write(ptr, &[7; 16]);
        assert_eq!(heap.read(ptr, 16), vec![7; 16]);
    }

    /// 伸ばした端数は捨てずに空きへ入る
    #[test]
    fn 伸ばした端数は空きに残る() {
        let mut heap = Heap::new();
        let ptr = heap.alloc(100);
        let free = heap.free_list();
        assert_eq!(free.len(), 1);
        assert_eq!(free[0].0, ptr + 100_u32.next_multiple_of(HEADER));
        assert_eq!(heap.word(HEAP_LIMIT), PAGE);
    }

    /// 32bit に収まらない要求は trap。丸めであふれる手前も含む
    #[test]
    fn 三十二bitに収まらない要求はtrapする() {
        let mut heap = Heap::new();
        for size in [u32::MAX, u32::MAX - 8, 0xFFFF_FFF0] {
            assert!(heap.try_alloc(size).is_err(), "{size}");
        }
    }

    /// 32bit のアドレス空間を使い切る要求は、伸ばしにいく前に trap する。
    ///
    /// エンジンが 4GiB を予約できるかどうかに寄りかからない。線形メモリの
    /// 上限そのものが理由なので、どのエンジンでも同じ答えになる
    #[test]
    fn アドレス空間を越える要求は伸ばす前にtrapする() {
        let mut heap = Heap::new();
        assert!(heap.try_alloc(MAX_HEAP_END).is_err());
        // 直前まで壊れていない。普通の割り当ては続けられる
        assert_ne!(heap.alloc(16), 0);
    }

    // -----------------------------------------------------------------------
    // 成果物としての性質(tasks 2.5)
    // -----------------------------------------------------------------------

    /// ランタイムを載せても import は増えない。start section も持たない
    #[test]
    fn ランタイムはimportもstartも持たない() {
        let (bytes, _) = module(b"literal");
        crate::wasm::validate(&bytes).expect("検証を通るはず");
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            match payload.expect("読めるはず") {
                wasmparser::Payload::ImportSection(_) => panic!("import がある"),
                wasmparser::Payload::StartSection { .. } => panic!("start section がある"),
                _ => {}
            }
        }
    }

    #[test]
    fn 同じ入力からは同じbytesが出る() {
        assert_eq!(module(b"abc").0, module(b"abc").0);
    }

    /// 同じ手順を踏めば同じアドレスが返る。割り当ての順も決定的
    #[test]
    fn 割り当ての結果は決定的() {
        let trace = || {
            let mut heap = Heap::new();
            let a = heap.alloc(40);
            let b = heap.alloc(8);
            heap.free(a);
            let c = heap.alloc(24);
            vec![a, b, c, heap.free_list().len() as u32]
        };
        assert_eq!(trace(), trace());
    }
}
