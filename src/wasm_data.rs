//! 所有データの下ろし(design.md 決定5・6・7)。
//!
//! ここが持つのは「値を組み立てる」「所有を渡す」「掃除する」ための命令の
//! 断片と、並びごとに生成する clone/drop/等値の glue。所有と借用の判断そのもの
//! は所有権検査が済ませてあるので、ここで組み直さない。
//!
//! glue の約束は「**その値が持っているものを解放する。値そのものが載っている
//! 記憶は解放しない**」。だから同じ関数が、単独の割り当ての根としても、
//! struct の中に直に置かれたフィールドとしても使える(design.md 決定5)。
//! 根を返すのは持ち主の仕事。
//!
//! 割り当てと解放は `wasm_runtime`、記憶の並びは `wasm_layout` が決める。

use crate::wasm_layout::{
    BUFFER_CAPACITY, BUFFER_DATA, BUFFER_LEN, BUFFER_SIZE, LayoutId, Layouts, Shape, Slot,
};
use crate::wasm_runtime::{Body, Helper};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

/// 並び1つ分の生成関数。番号は本体を出す前に確定している(design.md 決定5)。
///
/// - `drop(ptr)` — `ptr` が持っている記憶を解放する。`ptr` 自身は解放しない
/// - `clone(src, dst)` — 割り当て済みの `dst` へ深く写す
/// - `eq(a, b) -> i32` — 構造的に等しいか
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

/// `u32` の load/store。根の帳簿も子アドレスも 4 byte 揃え
fn word(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

fn byte(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 0,
        memory_index: 0,
    }
}

/// 式1つが使える作業用の局所の数。入れ子になっても踏まないよう、要る式ごとに
/// 別の区画を配る
pub const SCRATCH: u32 = 3;

// ---------------------------------------------------------------------------
// Copy な値の読み書き(tasks 3.3)
// ---------------------------------------------------------------------------

/// Copy な並びを記憶から読む。stack のアドレスを、平らな値の並びへ置き換える
pub fn load_copy(b: &mut Body, layouts: &Layouts, layout: LayoutId, offset: u32) {
    match &layouts.get(layout).shape {
        // 値を持たないので、アドレスも要らない
        Shape::Unit => b.ins(Instruction::Drop),
        Shape::Bool => b.ins(Instruction::I32Load8U(byte(offset))),
        Shape::Tag(_) => b.ins(Instruction::I32Load(word(offset))),
        Shape::Int => b.ins(Instruction::I64Load(MemArg {
            offset: u64::from(offset),
            align: 3,
            memory_index: 0,
        })),
        other => unreachable!("Copy ではない並びを読もうとしました: {other:?}"),
    };
}

/// Copy な並びを記憶へ書く。アドレスと値をこの順に積んでおくこと
pub fn store_copy(b: &mut Body, layouts: &Layouts, layout: LayoutId, offset: u32) {
    match &layouts.get(layout).shape {
        // アドレスだけが積まれている。書くものが無いので落とす
        Shape::Unit => b.ins(Instruction::Drop),
        Shape::Bool => b.ins(Instruction::I32Store8(byte(offset))),
        Shape::Tag(_) => b.ins(Instruction::I32Store(word(offset))),
        Shape::Int => b.ins(Instruction::I64Store(MemArg {
            offset: u64::from(offset),
            align: 3,
            memory_index: 0,
        })),
        other => unreachable!("Copy ではない並びを書こうとしました: {other:?}"),
    };
}

// ---------------------------------------------------------------------------
// 値の組み立てと受け渡し
// ---------------------------------------------------------------------------

/// 静的データの UTF-8 から、所有する `str` の根を1つ作る(tasks 4.1)。
///
/// 帳簿と中身は別の割り当てにする。だから同じリテラルから作った文字列どうしも
/// 記憶を共有せず、片方を drop してももう片方は生きている
pub fn literal_str(b: &mut Body, indices: &Indices, scratch: u32, offset: u32, len: u32) {
    let (root, buffer) = (scratch, scratch + 1);
    b.num(BUFFER_SIZE).ins(Instruction::Call(indices.alloc));
    b.set(root);
    b.num(len).ins(Instruction::Call(indices.alloc)).set(buffer);

    b.get(buffer).num(offset).num(len);
    b.ins(Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });

    b.get(root)
        .get(buffer)
        .ins(Instruction::I32Store(word(BUFFER_DATA)));
    b.get(root)
        .num(len)
        .ins(Instruction::I32Store(word(BUFFER_LEN)));
    b.get(root)
        .num(len)
        .ins(Instruction::I32Store(word(BUFFER_CAPACITY)));
    b.get(root);
}

/// 所有する値の根を1つ確保する。中身はまだ何も入っていない
pub fn alloc_root(b: &mut Body, indices: &Indices, layouts: &Layouts, layout: LayoutId) {
    b.num(layouts.extent(layout).size)
        .ins(Instruction::Call(indices.alloc));
}

/// 一時的な根に載っている所有値を、器の中の区画へそのまま移す(tasks 3.3)。
///
/// 写すのは記憶の中身だけ。中身が指している buffer や子の所有はそのまま移る
/// ので、空になった一時的な根だけを返す
pub fn relocate_into(
    b: &mut Body,
    indices: &Indices,
    layouts: &Layouts,
    layout: LayoutId,
    container: u32,
    offset: u32,
    temporary: u32,
) {
    b.set(temporary);
    b.get(container).offset(offset);
    b.get(temporary);
    b.num(layouts.extent(layout).size);
    b.ins(Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });
    b.get(temporary).ins(Instruction::Call(indices.free));
}

/// 器の中の区画を、独立した根へ取り出す(tasks 5.4)。
///
/// 直接置かれたフィールドを消費するときに使う。器そのものの後始末は、計画が
/// 出した残余の破棄が受け持つ
pub fn extract_root(
    b: &mut Body,
    indices: &Indices,
    layouts: &Layouts,
    layout: LayoutId,
    source: u32,
    offset: u32,
    fresh: u32,
) {
    b.set(source);
    alloc_root(b, indices, layouts, layout);
    b.set(fresh);
    b.get(fresh);
    b.get(source).offset(offset);
    b.num(layouts.extent(layout).size);
    b.ins(Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });
    b.get(fresh);
}

/// 所有する値を根ごと落とす。glue は中身しか解放しないので、根はここで返す
pub fn drop_root(b: &mut Body, indices: &Indices, glue: Glue, address: u32) {
    b.get(address).ins(Instruction::Call(glue.drop));
    b.get(address).ins(Instruction::Call(indices.free));
}

/// 初期化フラグで守った破棄(tasks 3.4)。
///
/// 経路によっては move 済みかもしれない。計画は「持っていれば落とす」までしか
/// 言えないので、実際に持っているかは実行時のフラグが答える
pub fn drop_guarded(b: &mut Body, indices: &Indices, glue: Glue, address: u32, flag: u32) {
    b.get(flag);
    b.ins(Instruction::If(BlockType::Empty));
    drop_root(b, indices, glue, address);
    b.num(0).set(flag);
    b.ins(Instruction::End);
}

/// 束縛が所有を得た。フラグは値が完成した**後**に立てる
pub fn mark_initialized(b: &mut Body, flag: u32) {
    b.num(1).set(flag);
}

/// 所有を持ち出した。アドレスはそのままだが、もうここの持ち物ではない
pub fn mark_moved(b: &mut Body, flag: u32) {
    b.num(0).set(flag);
}

/// 区画1つを落とす。`address` の局所に器の根が入っている前提。
///
/// 直接置かれた子は中身だけを落とし、`indirect` の子は根ごと返す。器そのものは
/// 触らない
pub fn drop_slot(
    b: &mut Body,
    indices: &Indices,
    layouts: &Layouts,
    slot: &Slot,
    address: u32,
    scratch: u32,
) {
    let child = layouts.get(slot.layout);
    if slot.indirect {
        b.get(address).offset(slot.offset).load().tee(scratch);
        // 0 は `nil` か持ち出し済み。どちらにせよ返すものが無い
        b.ins(Instruction::If(BlockType::Empty));
        if child.copy {
            b.get(scratch).ins(Instruction::Call(indices.free));
        } else {
            drop_root(b, indices, indices.of(slot.layout), scratch);
        }
        b.ins(Instruction::End);
        return;
    }
    if child.copy {
        return;
    }
    b.get(address).offset(slot.offset);
    b.ins(Instruction::Call(indices.of(slot.layout).drop));
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
        let (drop, clone, eq) = match &layout.shape {
            Shape::Str => (str_drop(indices), str_clone(indices), str_eq()),
            Shape::Struct { fields, .. } => (
                compound_drop(indices, layouts, fields),
                compound_clone(indices, layouts, id, fields),
                compound_eq(indices, layouts, fields),
            ),
            // 対応検査が先に止めるので、ここへ来たら検査の抜け
            other => unreachable!("glue を出せない並びです: {other:?} ({id:?})"),
        };
        helpers.push(one(vec![ValType::I32], vec![], drop));
        helpers.push(one(vec![ValType::I32, ValType::I32], vec![], clone));
        helpers.push(one(
            vec![ValType::I32, ValType::I32],
            vec![ValType::I32],
            eq,
        ));
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

/// `drop_str(ptr)`。中身の buffer を返す。帳簿そのものは持ち主が返す
fn str_drop(indices: &Indices) -> Function {
    let mut b = Body::new(0);
    b.get(0)
        .ins(Instruction::I32Load(word(BUFFER_DATA)))
        .ins(Instruction::Call(indices.free));
    b.finish()
}

/// `clone_str(src, dst)`。中身まで写すので、元と複製は記憶を共有しない
fn str_clone(indices: &Indices) -> Function {
    // 2: buffer, 3: len
    let mut b = Body::new(2);
    b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN))).set(3);
    b.get(3).ins(Instruction::Call(indices.alloc)).set(2);

    b.get(2)
        .get(0)
        .ins(Instruction::I32Load(word(BUFFER_DATA)))
        .get(3);
    b.ins(Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });

    b.get(1)
        .get(2)
        .ins(Instruction::I32Store(word(BUFFER_DATA)));
    b.get(1).get(3).ins(Instruction::I32Store(word(BUFFER_LEN)));
    b.get(1)
        .get(3)
        .ins(Instruction::I32Store(word(BUFFER_CAPACITY)));
    b.finish()
}

/// `eq_str(a, b) -> i32`。長さが違えばそこで、違う byte を見つけたらそこで打ち切る
fn str_eq() -> Function {
    // 2: len, 3: index
    let mut b = Body::new(2);
    b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN))).set(2);
    b.get(2)
        .get(1)
        .ins(Instruction::I32Load(word(BUFFER_LEN)))
        .ins(Instruction::I32Ne);
    b.ins(Instruction::If(BlockType::Empty));
    b.num(0).ins(Instruction::Return);
    b.ins(Instruction::End);

    b.num(0).set(3);
    b.ins(Instruction::Block(BlockType::Empty));
    b.ins(Instruction::Loop(BlockType::Empty));
    b.get(3).get(2).ins(Instruction::I32GeU);
    b.ins(Instruction::BrIf(1));
    for side in [0, 1] {
        b.get(side)
            .ins(Instruction::I32Load(word(BUFFER_DATA)))
            .get(3)
            .ins(Instruction::I32Add)
            .ins(Instruction::I32Load8U(byte(0)));
    }
    b.ins(Instruction::I32Ne);
    b.ins(Instruction::If(BlockType::Empty));
    b.num(0).ins(Instruction::Return);
    b.ins(Instruction::End);
    b.get(3).num(1).ins(Instruction::I32Add).set(3);
    b.ins(Instruction::Br(0));
    b.ins(Instruction::End);
    b.ins(Instruction::End);

    b.num(1);
    b.finish()
}

/// 直接置かれた子は中身だけ、`indirect` の子は根ごと落とす
fn compound_drop(indices: &Indices, layouts: &Layouts, slots: &[Slot]) -> Function {
    // 1: 子のアドレス
    let mut b = Body::new(1);
    for slot in slots {
        drop_slot(&mut b, indices, layouts, slot, 0, 1);
    }
    b.finish()
}

/// まず浅く写してから、所有している子だけを作り直して上書きする
fn compound_clone(
    indices: &Indices,
    layouts: &Layouts,
    layout: LayoutId,
    slots: &[Slot],
) -> Function {
    // 2: 元の子, 3: 新しい子
    let mut b = Body::new(2);
    b.get(1).get(0).num(layouts.extent(layout).size);
    b.ins(Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });

    for slot in slots {
        let child = layouts.get(slot.layout);
        if slot.indirect {
            b.get(0).offset(slot.offset).load().tee(2);
            b.ins(Instruction::If(BlockType::Empty));
            b.num(layouts.extent(slot.layout).size)
                .ins(Instruction::Call(indices.alloc))
                .set(3);
            if child.copy {
                b.get(3).get(2).num(layouts.extent(slot.layout).size);
                b.ins(Instruction::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
            } else {
                b.get(2)
                    .get(3)
                    .ins(Instruction::Call(indices.of(slot.layout).clone));
            }
            b.get(1)
                .offset(slot.offset)
                .get(3)
                .ins(Instruction::I32Store(word(0)));
            b.ins(Instruction::End);
            continue;
        }
        // Copy な子は浅い写しでもう正しい
        if child.copy {
            continue;
        }
        b.get(0).offset(slot.offset);
        b.get(1).offset(slot.offset);
        b.ins(Instruction::Call(indices.of(slot.layout).clone));
    }
    b.finish()
}

/// 宣言順に比べて、違いを見つけたところで打ち切る
fn compound_eq(indices: &Indices, layouts: &Layouts, slots: &[Slot]) -> Function {
    // 2: 左の子, 3: 右の子
    let mut b = Body::new(2);
    for slot in slots {
        let child = layouts.get(slot.layout);
        if slot.indirect {
            b.get(0).offset(slot.offset).load().set(2);
            b.get(1).offset(slot.offset).load().set(3);
            // 片方だけが `nil` なら違う。両方 `nil` なら次のフィールドへ
            b.get(2).ins(Instruction::I32Eqz);
            b.get(3).ins(Instruction::I32Eqz);
            b.ins(Instruction::I32Ne);
            b.ins(Instruction::If(BlockType::Empty));
            b.num(0).ins(Instruction::Return);
            b.ins(Instruction::End);
            b.get(2);
            b.ins(Instruction::If(BlockType::Empty));
            if child.copy {
                b.get(2);
                load_copy(&mut b, layouts, slot.layout, 0);
                b.get(3);
                load_copy(&mut b, layouts, slot.layout, 0);
                equal_flat(&mut b, layouts, slot.layout);
            } else {
                b.get(2)
                    .get(3)
                    .ins(Instruction::Call(indices.of(slot.layout).eq));
            }
            b.ins(Instruction::I32Eqz);
            b.ins(Instruction::If(BlockType::Empty));
            b.num(0).ins(Instruction::Return);
            b.ins(Instruction::End);
            b.ins(Instruction::End);
            continue;
        }
        if child.copy {
            // `unit` は値を持たないので常に等しい
            if matches!(child.shape, Shape::Unit) {
                continue;
            }
            b.get(0);
            load_copy(&mut b, layouts, slot.layout, slot.offset);
            b.get(1);
            load_copy(&mut b, layouts, slot.layout, slot.offset);
            equal_flat(&mut b, layouts, slot.layout);
        } else {
            b.get(0).offset(slot.offset);
            b.get(1).offset(slot.offset);
            b.ins(Instruction::Call(indices.of(slot.layout).eq));
        }
        b.ins(Instruction::I32Eqz);
        b.ins(Instruction::If(BlockType::Empty));
        b.num(0).ins(Instruction::Return);
        b.ins(Instruction::End);
    }
    b.num(1);
    b.finish()
}

/// 平らに積んだ Copy 値2つを比べる
fn equal_flat(b: &mut Body, layouts: &Layouts, layout: LayoutId) {
    match &layouts.get(layout).shape {
        Shape::Int => b.ins(Instruction::I64Eq),
        Shape::Bool | Shape::Tag(_) => b.ins(Instruction::I32Eq),
        other => unreachable!("平らに比べられない並びです: {other:?}"),
    };
}
