//! 所有データの下ろし(design.md 決定5・6・7)。
//!
//! ここが持つのは「値を積む」「場所を作る」「所有を渡す」「掃除する」ための
//! 命令の断片と、並びごとに生成する clone/drop/等値の glue。所有と借用の
//! 判断そのものは所有権検査が済ませてあるので、ここで組み直さない。
//!
//! 割り当てと解放は `wasm_runtime`、記憶の並びは `wasm_layout` が決める。

use crate::wasm_layout::{
    BUFFER_CAPACITY, BUFFER_DATA, BUFFER_LEN, BUFFER_SIZE, LayoutId, Layouts, Shape,
};
use crate::wasm_runtime::Helper;
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

/// 並び1つ分の生成関数。番号は本体を出す前に確定している(design.md 決定5)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Glue {
    pub drop: u32,
    pub clone: u32,
    pub eq: u32,
}

/// 生成モジュールの中で、ランタイムと glue がどこに居るか。
pub struct Indices {
    pub alloc: u32,
    pub free: u32,
    /// 所有する並び → その glue。Copy の並びは持たない
    pub glue: BTreeMap<LayoutId, Glue>,
}

impl Indices {
    /// 所有する並びごとに drop / clone / eq を1つずつ、計画順に予約する。
    ///
    /// 本体より先に番号を決めておくので、再帰する型の glue も自分自身を
    /// 呼べる(design.md 決定5)
    pub fn reserve(layouts: &Layouts, base: u32) -> Indices {
        let mut glue = BTreeMap::new();
        let mut next = base;
        for (id, layout) in layouts.planned() {
            if layout.copy {
                continue;
            }
            glue.insert(
                id,
                Glue {
                    drop: next,
                    clone: next + 1,
                    eq: next + 2,
                },
            );
            next += 3;
        }
        Indices {
            alloc: 0,
            free: 0,
            glue,
        }
    }

    pub fn of(&self, layout: LayoutId) -> Glue {
        self.glue[&layout]
    }
}

/// `u32` の load/store。根の帳簿はどれも 4 byte 揃え
fn word(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

/// 本体1つが使う作業用の局所。所有データを組み立てる途中の値を置く
pub const SCRATCH: u32 = 2;

// ---------------------------------------------------------------------------
// 値と場所
// ---------------------------------------------------------------------------

/// 静的データの UTF-8 から、所有する `str` の根を1つ作る(tasks 4.1)。
///
/// 帳簿と中身は別の割り当てにする。だから同じリテラルから作った文字列どうしも
/// 記憶を共有せず、片方を drop してももう片方は生きている
pub fn literal_str(f: &mut Function, indices: &Indices, scratch: u32, offset: u32, len: u32) {
    let (root, buffer) = (scratch, scratch + 1);
    f.instruction(&Instruction::I32Const(BUFFER_SIZE as i32));
    f.instruction(&Instruction::Call(indices.alloc));
    f.instruction(&Instruction::LocalSet(root));
    f.instruction(&Instruction::I32Const(len as i32));
    f.instruction(&Instruction::Call(indices.alloc));
    f.instruction(&Instruction::LocalSet(buffer));

    f.instruction(&Instruction::LocalGet(buffer));
    f.instruction(&Instruction::I32Const(offset as i32));
    f.instruction(&Instruction::I32Const(len as i32));
    f.instruction(&Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });

    for (at, value) in [
        (BUFFER_DATA, buffer),
        (BUFFER_LEN, u32::MAX),
        (BUFFER_CAPACITY, u32::MAX),
    ] {
        f.instruction(&Instruction::LocalGet(root));
        if value == u32::MAX {
            f.instruction(&Instruction::I32Const(len as i32));
        } else {
            f.instruction(&Instruction::LocalGet(value));
        }
        f.instruction(&Instruction::I32Store(word(at)));
    }
    f.instruction(&Instruction::LocalGet(root));
}

/// 初期化フラグで守った破棄(tasks 3.4)。
///
/// 経路によっては move 済みかもしれない。計画は「持っていれば落とす」までしか
/// 言えないので、実際に持っているかは実行時のフラグが答える
pub fn drop_guarded(f: &mut Function, glue: Glue, address: u32, flag: u32) {
    f.instruction(&Instruction::LocalGet(flag));
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(address));
    f.instruction(&Instruction::Call(glue.drop));
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(flag));
    f.instruction(&Instruction::End);
}

/// 束縛が所有を得た。フラグは値が完成した**後**に立てる
pub fn mark_initialized(f: &mut Function, flag: u32) {
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::LocalSet(flag));
}

/// 所有を持ち出した。アドレスはそのままだが、もうここの持ち物ではない
pub fn mark_moved(f: &mut Function, flag: u32) {
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(flag));
}

// ---------------------------------------------------------------------------
// 並びごとの glue
// ---------------------------------------------------------------------------

/// 所有する並びごとに drop / clone / eq を出す。並びは `Indices::reserve` と同じ順
pub fn glue_functions(layouts: &Layouts, indices: &Indices) -> Vec<Helper> {
    let mut helpers = Vec::new();
    for (id, layout) in layouts.planned() {
        if layout.copy {
            continue;
        }
        match &layout.shape {
            Shape::Str => {
                helpers.push(one(vec![ValType::I32], vec![], str_drop(indices)));
                helpers.push(one(
                    vec![ValType::I32],
                    vec![ValType::I32],
                    str_clone(indices),
                ));
                helpers.push(one(
                    vec![ValType::I32, ValType::I32],
                    vec![ValType::I32],
                    str_eq(),
                ));
            }
            // 対応検査が先に止めるので、ここへ来たら検査の抜け
            other => unreachable!("glue を出せない並びです: {other:?} ({id:?})"),
        }
    }
    helpers
}

fn one(params: Vec<ValType>, results: Vec<ValType>, body: Function) -> Helper {
    Helper {
        params,
        results,
        body,
    }
}

/// `drop_str(ptr)`。中身の buffer を先に返してから帳簿を返す
fn str_drop(indices: &Indices) -> Function {
    let mut f = Function::new([]);
    f.instruction(&Instruction::LocalGet(0));
    f.instruction(&Instruction::I32Load(word(BUFFER_DATA)));
    f.instruction(&Instruction::Call(indices.free));
    f.instruction(&Instruction::LocalGet(0));
    f.instruction(&Instruction::Call(indices.free));
    f.instruction(&Instruction::End);
    f
}

/// `clone_str(ptr) -> i32`。中身まで写すので、元と複製は記憶を共有しない
fn str_clone(indices: &Indices) -> Function {
    // 1: root, 2: buffer, 3: len
    let mut f = Function::new([(3, ValType::I32)]);
    f.instruction(&Instruction::LocalGet(0));
    f.instruction(&Instruction::I32Load(word(BUFFER_LEN)));
    f.instruction(&Instruction::LocalSet(3));

    f.instruction(&Instruction::I32Const(BUFFER_SIZE as i32));
    f.instruction(&Instruction::Call(indices.alloc));
    f.instruction(&Instruction::LocalSet(1));
    f.instruction(&Instruction::LocalGet(3));
    f.instruction(&Instruction::Call(indices.alloc));
    f.instruction(&Instruction::LocalSet(2));

    f.instruction(&Instruction::LocalGet(2));
    f.instruction(&Instruction::LocalGet(0));
    f.instruction(&Instruction::I32Load(word(BUFFER_DATA)));
    f.instruction(&Instruction::LocalGet(3));
    f.instruction(&Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });

    for (at, value) in [(BUFFER_DATA, 2), (BUFFER_LEN, 3), (BUFFER_CAPACITY, 3)] {
        f.instruction(&Instruction::LocalGet(1));
        f.instruction(&Instruction::LocalGet(value));
        f.instruction(&Instruction::I32Store(word(at)));
    }
    f.instruction(&Instruction::LocalGet(1));
    f.instruction(&Instruction::End);
    f
}

/// `eq_str(a, b) -> i32`。長さが違えばそこで、違う byte を見つけたらそこで打ち切る
fn str_eq() -> Function {
    // 2: len, 3: index
    let mut f = Function::new([(2, ValType::I32)]);
    f.instruction(&Instruction::LocalGet(0));
    f.instruction(&Instruction::I32Load(word(BUFFER_LEN)));
    f.instruction(&Instruction::LocalSet(2));
    f.instruction(&Instruction::LocalGet(2));
    f.instruction(&Instruction::LocalGet(1));
    f.instruction(&Instruction::I32Load(word(BUFFER_LEN)));
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::Return);
    f.instruction(&Instruction::End);

    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalSet(3));
    f.instruction(&Instruction::Block(BlockType::Empty));
    f.instruction(&Instruction::Loop(BlockType::Empty));
    f.instruction(&Instruction::LocalGet(3));
    f.instruction(&Instruction::LocalGet(2));
    f.instruction(&Instruction::I32GeU);
    f.instruction(&Instruction::BrIf(1));
    for side in [0, 1] {
        f.instruction(&Instruction::LocalGet(side));
        f.instruction(&Instruction::I32Load(word(BUFFER_DATA)));
        f.instruction(&Instruction::LocalGet(3));
        f.instruction(&Instruction::I32Add);
        f.instruction(&Instruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
    }
    f.instruction(&Instruction::I32Ne);
    f.instruction(&Instruction::If(BlockType::Empty));
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::Return);
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::LocalGet(3));
    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::I32Add);
    f.instruction(&Instruction::LocalSet(3));
    f.instruction(&Instruction::Br(0));
    f.instruction(&Instruction::End);
    f.instruction(&Instruction::End);

    f.instruction(&Instruction::I32Const(1));
    f.instruction(&Instruction::End);
    f
}
