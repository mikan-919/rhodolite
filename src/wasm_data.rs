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
    BUFFER_CAPACITY, BUFFER_DATA, BUFFER_LEN, BUFFER_SIZE, LayoutId, Layouts, OPTIONAL_PRESENT,
    Shape, Slot,
};
use crate::wasm_runtime::{Body, Helper};
use std::collections::{BTreeMap, BTreeSet};
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
    /// 配列の並び → その `reserve` 関数(MAP-075 決定3)。glue の後ろに並ぶ
    pub reserves: BTreeMap<LayoutId, u32>,
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
            reserves: BTreeMap::new(),
        }
    }

    /// `push` が届いた配列の並びごとに `reserve` を1つ、glue の後ろへ予約する
    /// (MAP-075)。
    ///
    /// 並びは `Indices::reserve` と同じ計画順なので、同じ入力からは同じ番号。
    /// `push` を1つも持たないモジュールのバイト列はこの change の前と変わらない
    pub fn reserve_arrays(&mut self, layouts: &Layouts, base: u32, pushed: &BTreeSet<LayoutId>) {
        let mut next = base;
        for (id, layout) in layouts.planned() {
            if layout.copy || !matches!(layout.shape, Shape::Array { .. }) {
                continue;
            }
            if !pushed.contains(&id) {
                continue;
            }
            self.reserves.insert(id, next);
            next += 1;
        }
    }

    /// 予約した glue の本数。後ろへ番号を積む側が使う
    pub fn glue_count(&self) -> u32 {
        self.glue.len() as u32 * 3
    }

    /// 予約した配列 `reserve` の本数
    pub fn reserve_count(&self) -> u32 {
        self.reserves.len() as u32
    }

    pub fn of(&self, layout: LayoutId) -> Glue {
        self.glue[&layout]
    }

    /// その配列の並びの `reserve` 関数
    pub fn reserve_of(&self, layout: LayoutId) -> u32 {
        self.reserves[&layout]
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

fn wide(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 3,
        memory_index: 0,
    }
}

/// 配列と `str` の帳簿の場所
pub fn buffer_data() -> MemArg {
    word(BUFFER_DATA)
}

pub fn buffer_len() -> MemArg {
    word(BUFFER_LEN)
}

pub fn buffer_capacity() -> MemArg {
    word(BUFFER_CAPACITY)
}

/// 式1つが使える作業用の局所の数。入れ子になっても踏まないよう、要る式ごとに
/// 別の区画を配る。いちばん多く要るのは `for`(対象・buffer・長さ・添字・
/// 要素・取り出した根)
pub const SCRATCH: u32 = 6;

/// enum と optional の tag はどちらも先頭に置く
pub const TAG: u32 = 0;

/// optional の tag は 1 byte、enum の tag は 4 byte
pub fn tag_byte() -> MemArg {
    byte(TAG)
}

pub fn tag_word() -> MemArg {
    word(TAG)
}

// ---------------------------------------------------------------------------
// Copy な値の読み書き(tasks 3.3・6.1)
// ---------------------------------------------------------------------------

/// Copy な並びを記憶から読んで、平らな値の並びを積む。
///
/// Copy な optional は `[tag, ...payload]` の順(design.md 決定2)。器の
/// アドレスは何度も要るので局所から取る
pub fn load_copy(b: &mut Body, layouts: &Layouts, layout: LayoutId, address: u32, offset: u32) {
    match &layouts.get(layout).shape {
        // 値を持たない
        Shape::Unit => {}
        Shape::Bool => {
            b.get(address).ins(Instruction::I32Load8U(byte(offset)));
        }
        Shape::Tag(_) => {
            b.get(address).ins(Instruction::I32Load(word(offset)));
        }
        Shape::Int => {
            b.get(address).ins(Instruction::I64Load(wide(offset)));
        }
        Shape::Optional { payload } => {
            let payload = *payload;
            b.get(address).ins(Instruction::I32Load8U(byte(offset)));
            load_copy(b, layouts, payload.layout, address, offset + payload.offset);
        }
        other => unreachable!("Copy ではない並びを読もうとしました: {other:?}"),
    }
}

/// 平らな値の並びを記憶へ書く。
///
/// `stash` はその並びと同じ型の局所。stack の上から順に受けてから書くので、
/// 器のアドレスと値の順を気にしなくてよい
pub fn store_copy(
    b: &mut Body,
    layouts: &Layouts,
    layout: LayoutId,
    address: u32,
    offset: u32,
    stash: &[u32],
) {
    for slot in stash.iter().rev() {
        b.set(*slot);
    }
    store_stashed(b, layouts, layout, address, offset, stash);
}

fn store_stashed(
    b: &mut Body,
    layouts: &Layouts,
    layout: LayoutId,
    address: u32,
    offset: u32,
    stash: &[u32],
) {
    match &layouts.get(layout).shape {
        Shape::Unit => {}
        Shape::Bool => {
            b.get(address)
                .get(stash[0])
                .ins(Instruction::I32Store8(byte(offset)));
        }
        Shape::Tag(_) => {
            b.get(address)
                .get(stash[0])
                .ins(Instruction::I32Store(word(offset)));
        }
        Shape::Int => {
            b.get(address)
                .get(stash[0])
                .ins(Instruction::I64Store(wide(offset)));
        }
        Shape::Optional { payload } => {
            let payload = *payload;
            b.get(address)
                .get(stash[0])
                .ins(Instruction::I32Store8(byte(offset)));
            store_stashed(
                b,
                layouts,
                payload.layout,
                address,
                offset + payload.offset,
                &stash[1..],
            );
        }
        other => unreachable!("Copy ではない並びを書こうとしました: {other:?}"),
    }
}

/// Copy な区画2つを記憶の上で比べて、結果を `i32` で残す。
///
/// optional は tag を先に見て、両方 present のときだけ中身へ潜る
pub fn equal_copy(
    b: &mut Body,
    layouts: &Layouts,
    layout: LayoutId,
    left: u32,
    right: u32,
    offset: u32,
) {
    match &layouts.get(layout).shape {
        // 値を持たないので常に等しい
        Shape::Unit => {
            b.num(1);
        }
        Shape::Bool => {
            b.get(left).ins(Instruction::I32Load8U(byte(offset)));
            b.get(right).ins(Instruction::I32Load8U(byte(offset)));
            b.ins(Instruction::I32Eq);
        }
        Shape::Tag(_) => {
            b.get(left).ins(Instruction::I32Load(word(offset)));
            b.get(right).ins(Instruction::I32Load(word(offset)));
            b.ins(Instruction::I32Eq);
        }
        Shape::Int => {
            b.get(left).ins(Instruction::I64Load(wide(offset)));
            b.get(right).ins(Instruction::I64Load(wide(offset)));
            b.ins(Instruction::I64Eq);
        }
        Shape::Optional { payload } => {
            let payload = *payload;
            b.get(left).ins(Instruction::I32Load8U(byte(offset)));
            b.get(right).ins(Instruction::I32Load8U(byte(offset)));
            b.ins(Instruction::I32Eq);
            b.ins(Instruction::If(BlockType::Result(ValType::I32)));
            // tag は同じ。present なら中身も比べる
            b.get(left).ins(Instruction::I32Load8U(byte(offset)));
            b.ins(Instruction::If(BlockType::Result(ValType::I32)));
            equal_copy(
                b,
                layouts,
                payload.layout,
                left,
                right,
                offset + payload.offset,
            );
            b.ins(Instruction::Else);
            b.num(1);
            b.ins(Instruction::End);
            b.ins(Instruction::Else);
            b.num(0);
            b.ins(Instruction::End);
        }
        other => unreachable!("Copy ではない並びを比べようとしました: {other:?}"),
    }
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

/// 器の中の区画を、独立した根へ取り出す(tasks 5.4・6.5)。
///
/// 直接置かれたフィールドや enum の payload を消費するときに使う。器そのものの
/// 後始末は呼ぶ側が受け持つ
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
                struct_drop(indices, layouts, fields),
                struct_clone(indices, layouts, id, fields),
                struct_eq(indices, layouts, fields),
            ),
            // optional は tag が 1 のときだけ中身を持つ。`{u8 tag, 詰め物, payload}`
            // の payload は非 Copy(Copy なら optional 全体が Copy になる)
            Shape::Optional { payload } => {
                let slots = std::slice::from_ref(payload);
                (
                    tagged_drop(indices, layouts, &[(OPTIONAL_PRESENT, slots)], true),
                    tagged_clone(indices, layouts, id, &[(OPTIONAL_PRESENT, slots)], true),
                    tagged_eq(indices, layouts, &[(OPTIONAL_PRESENT, slots)], true),
                )
            }
            Shape::Enum { variants, .. } => {
                let arms: Vec<(u32, &[Slot])> = variants
                    .iter()
                    .enumerate()
                    .map(|(index, slots)| (index as u32, slots.as_slice()))
                    .collect();
                (
                    tagged_drop(indices, layouts, &arms, false),
                    tagged_clone(indices, layouts, id, &arms, false),
                    tagged_eq(indices, layouts, &arms, false),
                )
            }
            // 配列は `str` と同じ帳簿。違うのは中身が刻み幅で並ぶことだけ
            Shape::Array { element, stride } => (
                array_drop(indices, layouts, *element, *stride),
                array_clone(indices, layouts, *element, *stride),
                array_eq(indices, layouts, *element, *stride),
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

/// 予約した配列の並びごとに `reserve` を1つ。並びは `reserve_arrays` と同じ順
pub fn reserve_functions(layouts: &Layouts, indices: &Indices) -> Vec<Helper> {
    let mut helpers = Vec::new();
    for (id, layout) in layouts.planned() {
        if !indices.reserves.contains_key(&id) {
            continue;
        }
        let Shape::Array { stride, .. } = layout.shape else {
            unreachable!("`reserve` を予約したのは配列の並びだけです");
        };
        helpers.push(one(
            vec![ValType::I32],
            vec![],
            array_reserve(indices, stride),
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

/// tag をその場所から読む。optional は 1 byte、enum は 4 byte
fn load_tag(b: &mut Body, address: u32, narrow: bool) {
    if narrow {
        b.get(address).ins(Instruction::I32Load8U(byte(TAG)));
    } else {
        b.get(address).ins(Instruction::I32Load(word(TAG)));
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
fn struct_drop(indices: &Indices, layouts: &Layouts, slots: &[Slot]) -> Function {
    // 1: 子のアドレス
    let mut b = Body::new(1);
    drop_slots(&mut b, indices, layouts, slots, 0, 1);
    b.finish()
}

fn drop_slots(
    b: &mut Body,
    indices: &Indices,
    layouts: &Layouts,
    slots: &[Slot],
    container: u32,
    scratch: u32,
) {
    for slot in slots {
        drop_slot(b, indices, layouts, slot, container, scratch);
    }
}

/// まず浅く写してから、所有している子だけを作り直して上書きする
fn struct_clone(
    indices: &Indices,
    layouts: &Layouts,
    layout: LayoutId,
    slots: &[Slot],
) -> Function {
    // 2: 元の子, 3: 新しい子
    let mut b = Body::new(2);
    shallow_copy(&mut b, layouts, layout, 0, 1);
    clone_slots(&mut b, indices, layouts, slots, 0, 1, 2, 3);
    b.finish()
}

/// `src` の中身を `dst` へそのまま写す。tag も詰め物も含めて丸ごと
fn shallow_copy(b: &mut Body, layouts: &Layouts, layout: LayoutId, src: u32, dst: u32) {
    b.get(dst).get(src).num(layouts.extent(layout).size);
    b.ins(Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });
}

#[allow(clippy::too_many_arguments)]
fn clone_slots(
    b: &mut Body,
    indices: &Indices,
    layouts: &Layouts,
    slots: &[Slot],
    src: u32,
    dst: u32,
    old: u32,
    new: u32,
) {
    for slot in slots {
        let child = layouts.get(slot.layout);
        if slot.indirect {
            b.get(src).offset(slot.offset).load().tee(old);
            b.ins(Instruction::If(BlockType::Empty));
            alloc_root(b, indices, layouts, slot.layout);
            b.set(new);
            if child.copy {
                b.get(new).get(old).num(layouts.extent(slot.layout).size);
                b.ins(Instruction::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
            } else {
                b.get(old)
                    .get(new)
                    .ins(Instruction::Call(indices.of(slot.layout).clone));
            }
            b.get(dst)
                .offset(slot.offset)
                .get(new)
                .ins(Instruction::I32Store(word(0)));
            b.ins(Instruction::End);
            continue;
        }
        // Copy な子は浅い写しでもう正しい
        if child.copy {
            continue;
        }
        b.get(src).offset(slot.offset);
        b.get(dst).offset(slot.offset);
        b.ins(Instruction::Call(indices.of(slot.layout).clone));
    }
}

/// 宣言順に比べて、違いを見つけたところで打ち切る
fn struct_eq(indices: &Indices, layouts: &Layouts, slots: &[Slot]) -> Function {
    // 2: 左の子, 3: 右の子
    let mut b = Body::new(2);
    eq_slots(&mut b, indices, layouts, slots, 0, 1, 2, 3);
    b.num(1);
    b.finish()
}

/// 区画を順に比べ、違えばその場で `0` を返す
#[allow(clippy::too_many_arguments)]
fn eq_slots(
    b: &mut Body,
    indices: &Indices,
    layouts: &Layouts,
    slots: &[Slot],
    left: u32,
    right: u32,
    a: u32,
    c: u32,
) {
    for slot in slots {
        let child = layouts.get(slot.layout);
        if slot.indirect {
            b.get(left).offset(slot.offset).load().set(a);
            b.get(right).offset(slot.offset).load().set(c);
            // 片方だけが `nil` なら違う。両方 `nil` なら次の区画へ
            b.get(a).ins(Instruction::I32Eqz);
            b.get(c).ins(Instruction::I32Eqz);
            b.ins(Instruction::I32Ne);
            b.ins(Instruction::If(BlockType::Empty));
            b.num(0).ins(Instruction::Return);
            b.ins(Instruction::End);
            b.get(a);
            b.ins(Instruction::If(BlockType::Empty));
            if child.copy {
                equal_copy(b, layouts, slot.layout, a, c, 0);
            } else {
                b.get(a)
                    .get(c)
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
            equal_copy(b, layouts, slot.layout, left, right, slot.offset);
        } else {
            b.get(left).offset(slot.offset);
            b.get(right).offset(slot.offset);
            b.ins(Instruction::Call(indices.of(slot.layout).eq));
        }
        b.ins(Instruction::I32Eqz);
        b.ins(Instruction::If(BlockType::Empty));
        b.num(0).ins(Instruction::Return);
        b.ins(Instruction::End);
    }
}

/// tag で中身が変わる並びの破棄(tasks 6.3)。
///
/// 活きている tag の区画だけを落とす。初期化されていない payload には触らない
fn tagged_drop(
    indices: &Indices,
    layouts: &Layouts,
    arms: &[(u32, &[Slot])],
    narrow: bool,
) -> Function {
    // 1: 子のアドレス
    let mut b = Body::new(1);
    for (tag, slots) in arms {
        if slots.iter().all(|slot| trivial(layouts, slot)) {
            continue;
        }
        load_tag(&mut b, 0, narrow);
        b.num(*tag).ins(Instruction::I32Eq);
        b.ins(Instruction::If(BlockType::Empty));
        drop_slots(&mut b, indices, layouts, slots, 0, 1);
        b.ins(Instruction::End);
    }
    b.finish()
}

/// 落とすものも作り直すものも無い区画か
fn trivial(layouts: &Layouts, slot: &Slot) -> bool {
    !slot.indirect && layouts.get(slot.layout).copy
}

fn tagged_clone(
    indices: &Indices,
    layouts: &Layouts,
    layout: LayoutId,
    arms: &[(u32, &[Slot])],
    narrow: bool,
) -> Function {
    // 2: 元の子, 3: 新しい子
    let mut b = Body::new(2);
    // tag も詰め物も含めて浅く写る。活きていない payload の中身も写るが、
    // 誰も読まないので害はない
    shallow_copy(&mut b, layouts, layout, 0, 1);
    for (tag, slots) in arms {
        if slots.iter().all(|slot| trivial(layouts, slot)) {
            continue;
        }
        load_tag(&mut b, 0, narrow);
        b.num(*tag).ins(Instruction::I32Eq);
        b.ins(Instruction::If(BlockType::Empty));
        clone_slots(&mut b, indices, layouts, slots, 0, 1, 2, 3);
        b.ins(Instruction::End);
    }
    b.finish()
}

fn tagged_eq(
    indices: &Indices,
    layouts: &Layouts,
    arms: &[(u32, &[Slot])],
    narrow: bool,
) -> Function {
    // 2: 左の子, 3: 右の子
    let mut b = Body::new(2);
    load_tag(&mut b, 0, narrow);
    load_tag(&mut b, 1, narrow);
    b.ins(Instruction::I32Ne);
    b.ins(Instruction::If(BlockType::Empty));
    b.num(0).ins(Instruction::Return);
    b.ins(Instruction::End);

    for (tag, slots) in arms {
        if slots.is_empty() {
            continue;
        }
        load_tag(&mut b, 0, narrow);
        b.num(*tag).ins(Instruction::I32Eq);
        b.ins(Instruction::If(BlockType::Empty));
        eq_slots(&mut b, indices, layouts, slots, 0, 1, 2, 3);
        b.ins(Instruction::End);
    }
    b.num(1);
    b.finish()
}

// ---------------------------------------------------------------------------
// 配列(tasks 7.2)
// ---------------------------------------------------------------------------

/// 要素を先頭から歩く。`emit` は1要素ぶんの命令を出す
fn walk_elements(b: &mut Body, len: u32, index: u32, emit: impl FnOnce(&mut Body)) {
    b.num(0).set(index);
    b.ins(Instruction::Block(BlockType::Empty));
    b.ins(Instruction::Loop(BlockType::Empty));
    b.get(index).get(len).ins(Instruction::I32GeU);
    b.ins(Instruction::BrIf(1));
    emit(b);
    b.get(index).num(1).ins(Instruction::I32Add).set(index);
    b.ins(Instruction::Br(0));
    b.ins(Instruction::End);
    b.ins(Instruction::End);
}

/// `data + index * stride`。刻みが 0 の要素(`unit` など)でも同じ式で通る
pub fn element_address(b: &mut Body, data: u32, index: u32, stride: u32) {
    b.get(data);
    b.get(index);
    if stride != 1 {
        b.num(stride).ins(Instruction::I32Mul);
    }
    b.ins(Instruction::I32Add);
}

/// `drop_array(ptr)`。所有する要素を順に落としてから buffer を返す
fn array_drop(indices: &Indices, layouts: &Layouts, element: LayoutId, stride: u32) -> Function {
    // 1: data, 2: len, 3: index
    let mut b = Body::new(3);
    if !layouts.get(element).copy {
        b.get(0).ins(Instruction::I32Load(word(BUFFER_DATA))).set(1);
        b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN))).set(2);
        let drop = indices.of(element).drop;
        walk_elements(&mut b, 2, 3, |b| {
            element_address(b, 1, 3, stride);
            b.ins(Instruction::Call(drop));
        });
    }
    b.get(0)
        .ins(Instruction::I32Load(word(BUFFER_DATA)))
        .ins(Instruction::Call(indices.free));
    b.finish()
}

/// `clone_array(src, dst)`。buffer ごと浅く写してから、所有する要素を作り直す
fn array_clone(indices: &Indices, layouts: &Layouts, element: LayoutId, stride: u32) -> Function {
    // 2: len, 3: 新しい buffer, 4: index, 5: 元の buffer
    let mut b = Body::new(4);
    b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN))).set(2);
    b.get(0).ins(Instruction::I32Load(word(BUFFER_DATA))).set(5);

    // 元の割り当てが通っている以上、同じ `len * stride` は 32bit に収まる
    b.get(2).num(stride).ins(Instruction::I32Mul);
    b.ins(Instruction::Call(indices.alloc)).set(3);
    b.get(3).get(5).get(2).num(stride).ins(Instruction::I32Mul);
    b.ins(Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });

    b.get(1)
        .get(3)
        .ins(Instruction::I32Store(word(BUFFER_DATA)));
    b.get(1).get(2).ins(Instruction::I32Store(word(BUFFER_LEN)));
    b.get(1)
        .get(2)
        .ins(Instruction::I32Store(word(BUFFER_CAPACITY)));

    if !layouts.get(element).copy {
        let clone = indices.of(element).clone;
        walk_elements(&mut b, 2, 4, |b| {
            element_address(b, 5, 4, stride);
            element_address(b, 3, 4, stride);
            b.ins(Instruction::Call(clone));
        });
    }
    b.finish()
}

/// `reserve_array(ptr)`。`len` が `capacity` に届いていたら容量を伸ばす
/// (MAP-075 決定3)。
///
/// capacity 0 からの初回は 1、それ以外は現在の倍。新しい buffer は既存の
/// `alloc` から取り、生きている要素の bytes をそのまま写して古い buffer を
/// 返す — 器の中身は根のアドレスか Copy な値なので、bytes を移せば所有も
/// そのまま移る(`array_clone` と違って要素を作り直さない)。
///
/// 割り当てが足りなければ `alloc` の中の `memory.grow` 失敗が `unreachable`
/// で落ちる(ADR-0011 §3)。push 固有の失敗表現は増やさない。
///
/// ponytail: `new_capacity * stride` の桁あふれは見ていない。あふれる手前で
/// `memory.grow` が先に落ちるので、追加の検査は 32bit では観測できない
fn array_reserve(indices: &Indices, stride: u32) -> Function {
    // 1: capacity, 2: 新しい capacity, 3: 新しい buffer, 4: 元の buffer
    let mut b = Body::new(4);
    b.get(0)
        .ins(Instruction::I32Load(word(BUFFER_CAPACITY)))
        .set(1);
    b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN)));
    b.get(1).ins(Instruction::I32GeU);
    b.ins(Instruction::If(BlockType::Empty));

    b.get(1).ins(Instruction::I32Eqz);
    b.ins(Instruction::If(BlockType::Empty));
    b.num(1).set(2);
    b.ins(Instruction::Else);
    b.get(1).num(2).ins(Instruction::I32Mul).set(2);
    b.ins(Instruction::End);

    b.get(2).num(stride).ins(Instruction::I32Mul);
    b.ins(Instruction::Call(indices.alloc)).set(3);
    b.get(0).ins(Instruction::I32Load(word(BUFFER_DATA))).set(4);
    b.get(3).get(4);
    b.get(0)
        .ins(Instruction::I32Load(word(BUFFER_LEN)))
        .num(stride)
        .ins(Instruction::I32Mul);
    b.ins(Instruction::MemoryCopy {
        src_mem: 0,
        dst_mem: 0,
    });
    b.get(4).ins(Instruction::Call(indices.free));
    b.get(0)
        .get(3)
        .ins(Instruction::I32Store(word(BUFFER_DATA)));
    b.get(0)
        .get(2)
        .ins(Instruction::I32Store(word(BUFFER_CAPACITY)));

    b.ins(Instruction::End);
    b.finish()
}

/// `eq_array(a, b) -> i32`。長さが違えばそこで、違う要素を見つけたらそこで打ち切る
fn array_eq(indices: &Indices, layouts: &Layouts, element: LayoutId, stride: u32) -> Function {
    // 2: len, 3: index, 4: 左の要素, 5: 右の要素
    let mut b = Body::new(4);
    b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN))).set(2);
    b.get(2)
        .get(1)
        .ins(Instruction::I32Load(word(BUFFER_LEN)))
        .ins(Instruction::I32Ne);
    b.ins(Instruction::If(BlockType::Empty));
    b.num(0).ins(Instruction::Return);
    b.ins(Instruction::End);

    let copy = layouts.get(element).copy;
    let eq = (!copy).then(|| indices.of(element).eq);
    walk_elements(&mut b, 2, 3, |b| {
        for (side, slot) in [(0, 4), (1, 5)] {
            b.get(side).ins(Instruction::I32Load(word(BUFFER_DATA)));
            b.get(3);
            if stride != 1 {
                b.num(stride).ins(Instruction::I32Mul);
            }
            b.ins(Instruction::I32Add).set(slot);
        }
        match eq {
            Some(eq) => {
                b.get(4).get(5).ins(Instruction::Call(eq));
            }
            None => equal_copy(b, layouts, element, 4, 5, 0),
        }
        b.ins(Instruction::I32Eqz);
        b.ins(Instruction::If(BlockType::Empty));
        b.num(0).ins(Instruction::Return);
        b.ins(Instruction::End);
    });
    b.num(1);
    b.finish()
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm_runtime::{ALLOC, COUNT, FREE, Runtime};
    use wasm_encoder::{CodeSection, ExportKind, ExportSection, FunctionSection, Module};

    /// allocator と1本の `reserve` だけを載せたモジュール。
    ///
    /// 生成器の知識を共有しないエンジンで走らせるので、容量の伸び方が
    /// 「本当に Core Wasm としてそう動く」ことの証明になる
    fn module(stride: u32) -> Vec<u8> {
        let runtime = Runtime::new(&[]).expect("前置きは 32bit に収まる");
        let indices = Indices {
            alloc: ALLOC,
            free: FREE,
            glue: BTreeMap::new(),
            reserves: BTreeMap::new(),
        };
        let helpers: Vec<Helper> = Runtime::helpers(0)
            .into_iter()
            .chain([Helper {
                params: vec![ValType::I32],
                results: Vec::new(),
                body: array_reserve(&indices, stride),
            }])
            .collect();

        let mut types = wasm_encoder::TypeSection::new();
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
        exports.export("reserve", ExportKind::Func, COUNT);
        exports.export("memory", ExportKind::Memory, 0);

        let mut module = Module::new();
        module.section(&types);
        module.section(&functions);
        module.section(&runtime.memory_section());
        module.section(&exports);
        module.section(&code);
        module.section(&runtime.data_section());
        module.finish()
    }

    /// エンジンを1つ立てて、帳簿を1つ握った手
    struct Buffer {
        store: wasmi::Store<()>,
        alloc: wasmi::Func,
        reserve: wasmi::Func,
        memory: wasmi::Memory,
        root: u32,
        stride: u32,
    }

    impl Buffer {
        /// 長さ0・容量0の配列を1つ作る。`data` は `alloc(0)` の返す一意の
        /// アドレスで、配列リテラルが空の配列に置くものと同じ形
        fn empty(stride: u32) -> Buffer {
            let bytes = module(stride);
            crate::wasm::validate(&bytes).expect("検証を通るはず");
            let engine = wasmi::Engine::default();
            let compiled = wasmi::Module::new(&engine, &bytes).expect("読めるはず");
            let mut store = wasmi::Store::new(&engine, ());
            let instance = wasmi::Linker::new(&engine)
                .instantiate_and_start(&mut store, &compiled)
                .expect("立ち上がるはず");
            let alloc = instance.get_func(&store, "alloc").unwrap();
            let reserve = instance.get_func(&store, "reserve").unwrap();
            let memory = instance.get_memory(&store, "memory").unwrap();
            let mut buffer = Buffer {
                store,
                alloc,
                reserve,
                memory,
                root: 0,
                stride,
            };
            buffer.root = buffer.alloc(12);
            let data = buffer.alloc(0);
            buffer.write(buffer.root + BUFFER_DATA, data);
            buffer.write(buffer.root + BUFFER_LEN, 0);
            buffer.write(buffer.root + BUFFER_CAPACITY, 0);
            buffer
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

        fn read(&self, at: u32) -> u32 {
            let mut word = [0u8; 4];
            self.memory
                .read(&self.store, at as usize, &mut word)
                .expect("読めるはず");
            u32::from_le_bytes(word)
        }

        fn write(&mut self, at: u32, value: u32) {
            self.memory
                .write(&mut self.store, at as usize, &value.to_le_bytes())
                .expect("書けるはず");
        }

        fn len(&self) -> u32 {
            self.read(self.root + BUFFER_LEN)
        }

        fn capacity(&self) -> u32 {
            self.read(self.root + BUFFER_CAPACITY)
        }

        fn element(&self, index: u32) -> u32 {
            self.read(self.read(self.root + BUFFER_DATA) + index * self.stride)
        }

        /// 生成器が `push` の地点で出すのと同じ順:容量を確かめ、次の席へ
        /// 要素を書き、長さを1つ進める
        fn push(&mut self, value: u32) {
            self.reserve
                .call(
                    &mut self.store,
                    &[wasmi::Val::I32(self.root as i32)],
                    &mut [],
                )
                .expect("伸ばせるはず");
            let seat = self.read(self.root + BUFFER_DATA) + self.len() * self.stride;
            self.write(seat, value);
            let len = self.len();
            self.write(self.root + BUFFER_LEN, len + 1);
        }
    }

    /// 容量は 0 → 1 → 2 → 4 と倍々に伸び、余りがあるうちは伸びない
    /// (MAP-075 決定3)
    #[test]
    fn pushの容量は倍々に伸びる() {
        let mut buffer = Buffer::empty(4);
        assert_eq!((buffer.len(), buffer.capacity()), (0, 0));

        buffer.push(10);
        assert_eq!((buffer.len(), buffer.capacity()), (1, 1));

        buffer.push(20);
        assert_eq!((buffer.len(), buffer.capacity()), (2, 2));

        buffer.push(30);
        assert_eq!((buffer.len(), buffer.capacity()), (3, 4));

        // 余りがあるので、この1つは伸ばさない
        buffer.push(40);
        assert_eq!((buffer.len(), buffer.capacity()), (4, 4));

        buffer.push(50);
        assert_eq!((buffer.len(), buffer.capacity()), (5, 8));
    }

    /// 伸ばす前の要素は、並びも中身もそのまま新しい buffer へ移る
    #[test]
    fn 容量を伸ばしても既存の要素は残る() {
        let mut buffer = Buffer::empty(4);
        for n in 1..=5u32 {
            buffer.push(n * 11);
        }
        let seen: Vec<u32> = (0..buffer.len()).map(|i| buffer.element(i)).collect();
        assert_eq!(seen, vec![11, 22, 33, 44, 55]);
    }
}
