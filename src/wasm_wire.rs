//! ABI v1 の正準 bytes(design.md 決定8、rhodolite-wasm-abi spec)。
//!
//! ホストとやり取りするのは**内部の並びではなく直列化した bytes**。だから
//! 記憶の詰め方を後から変えても ABI の版は上がらないし、壊れた別名グラフが
//! 単独所有を破ることもない。
//!
//! wire 形式は宣言の型で自己完結する:
//!
//! ```text
//! unit      0 byte
//! bool      1 byte(0 か 1 だけ)
//! int       リトルエンディアン i64
//! str       u32 byte 長 + UTF-8 bytes
//! struct    宣言順のフィールド
//! enum      u32 tag + 活きている payload
//! optional  u8 tag + 有ればその中身
//! 配列      u32 要素数 + 要素
//! ```
//!
//! 並び1つにつき3本を生成する。`size` は符号化に要る byte 数、`encode` は
//! 書き出し、`decode` は読み込み。`decode` は**本体が走る前に**境界と正準性を
//! 全部見て、少しでも外れたら trap する。巻き戻しは無いので、途中で落ちた値が
//! Rhodolite 側へ出ることはない。

use crate::hir;
use crate::wasm_data::Indices;
use crate::wasm_layout::{
    BUFFER_CAPACITY, BUFFER_DATA, BUFFER_LEN, LayoutId, Layouts, Shape, Slot,
};
use crate::wasm_runtime::{Body, HEADER, Helper};
use std::collections::BTreeMap;
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

/// 並び1つ分の直列化関数。番号は本体を出す前に確定している
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wire {
    /// `size(ptr) -> i32` — 符号化に要る byte 数
    pub size: u32,
    /// `encode(ptr, cursor) -> i32` — 書き出して次の cursor を返す
    pub encode: u32,
    /// `decode(cursor, end, dst) -> i32` — 読み込んで次の cursor を返す
    pub decode: u32,
}

/// 公開署名から辿れる並びだけに番号を振った表。
///
/// 内部でしか使わない型は載せない。非公開の宣言はホストとの契約ではないので、
/// 無関係な編集がメタデータを動かしてはいけない(design.md 決定9)
#[derive(Default)]
pub struct Wires {
    entries: BTreeMap<LayoutId, Wire>,
    /// 予約した順。メタデータの型 ID もこの順に振る
    order: Vec<LayoutId>,
    /// UTF-8 の検証。型に依らないので1本だけ
    utf8: u32,
}

impl Wires {
    /// 根から辿れる並びに、`base` から順に番号を振る。
    ///
    /// 子を訪ねる前に番号を押さえるので、再帰する `indirect` 型でも閉じる
    pub fn reserve(layouts: &Layouts, roots: &[LayoutId], base: u32) -> Wires {
        let mut wires = Wires {
            utf8: base,
            ..Wires::default()
        };
        let mut next = base + 1;
        for root in roots {
            wires.walk(layouts, *root, &mut next);
        }
        wires
    }

    fn walk(&mut self, layouts: &Layouts, layout: LayoutId, next: &mut u32) {
        if self.entries.contains_key(&layout) {
            return;
        }
        self.entries.insert(
            layout,
            Wire {
                size: *next,
                encode: *next + 1,
                decode: *next + 2,
            },
        );
        self.order.push(layout);
        *next += 3;

        match &layouts.get(layout).shape {
            Shape::Unit | Shape::Bool | Shape::Int | Shape::Tag(_) | Shape::Str => {}
            Shape::Optional { payload } => self.walk(layouts, payload.layout, next),
            Shape::Struct { fields, .. } => {
                for slot in fields {
                    self.walk(layouts, slot.layout, next);
                }
            }
            Shape::Enum { variants, .. } => {
                for slots in variants {
                    for slot in slots {
                        self.walk(layouts, slot.layout, next);
                    }
                }
            }
            Shape::Array { element, .. } => self.walk(layouts, *element, next),
        }
    }

    pub fn of(&self, layout: LayoutId) -> Wire {
        self.entries[&layout]
    }
}

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

/// wire は詰め物を置かないので、読み書きは揃っていないものとして扱う
fn loose(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 0,
        memory_index: 0,
    }
}

// ---------------------------------------------------------------------------
// 生成
// ---------------------------------------------------------------------------

/// 予約した順に、UTF-8 検証と並びごとの size / encode / decode を出す
pub fn wire_functions(
    program: &hir::Program,
    layouts: &Layouts,
    wires: &Wires,
    indices: &Indices,
) -> Vec<Helper> {
    let mut helpers = vec![Helper {
        params: vec![ValType::I32, ValType::I32],
        results: vec![],
        body: utf8_ok(),
    }];
    for layout in &wires.order {
        helpers.push(Helper {
            params: vec![ValType::I32],
            results: vec![ValType::I32],
            body: size_of(layouts, wires, *layout),
        });
        helpers.push(Helper {
            params: vec![ValType::I32, ValType::I32],
            results: vec![ValType::I32],
            body: encode(layouts, wires, *layout),
        });
        helpers.push(Helper {
            params: vec![ValType::I32, ValType::I32, ValType::I32],
            results: vec![ValType::I32],
            body: decode(program, layouts, wires, indices, *layout),
        });
    }
    helpers
}

/// `size(ptr) -> i32`。可変長は中身を歩いて数える
fn size_of(layouts: &Layouts, wires: &Wires, layout: LayoutId) -> wasm_encoder::Function {
    // 1: 合計, 2: 添字/子, 3: 上限
    let mut b = Body::with_locals(vec![(3, ValType::I32)]);
    match &layouts.get(layout).shape {
        Shape::Unit => {
            b.num(0);
        }
        Shape::Bool => {
            b.num(1);
        }
        Shape::Int => {
            b.num(8);
        }
        Shape::Tag(_) => {
            b.num(4);
        }
        Shape::Str => {
            b.num(4)
                .get(0)
                .ins(Instruction::I32Load(word(BUFFER_LEN)))
                .ins(Instruction::I32Add);
        }
        Shape::Optional { payload } => {
            let payload = *payload;
            b.num(1).set(1);
            b.get(0).ins(Instruction::I32Load8U(byte(0)));
            b.ins(Instruction::If(BlockType::Empty));
            b.get(1);
            b.get(0).offset(payload.offset);
            b.ins(Instruction::Call(wires.of(payload.layout).size));
            b.ins(Instruction::I32Add).set(1);
            b.ins(Instruction::End);
            b.get(1);
        }
        Shape::Struct { fields, .. } => {
            b.num(0).set(1);
            for slot in fields {
                slot_size(&mut b, wires, slot);
            }
            b.get(1);
        }
        Shape::Enum { variants, .. } => {
            b.num(4).set(1);
            b.get(0).ins(Instruction::I32Load(word(0))).set(3);
            for (tag, slots) in variants.iter().enumerate() {
                if slots.is_empty() {
                    continue;
                }
                b.get(3).num(tag as u32).ins(Instruction::I32Eq);
                b.ins(Instruction::If(BlockType::Empty));
                for slot in slots {
                    slot_size(&mut b, wires, slot);
                }
                b.ins(Instruction::End);
            }
            b.get(1);
        }
        Shape::Array { element, stride } => {
            let (element, stride) = (*element, *stride);
            b.num(4).set(1);
            b.num(0).set(2);
            b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN))).set(3);
            b.ins(Instruction::Block(BlockType::Empty));
            b.ins(Instruction::Loop(BlockType::Empty));
            b.get(2).get(3).ins(Instruction::I32GeU);
            b.ins(Instruction::BrIf(1));
            b.get(1);
            element_address(&mut b, stride);
            b.ins(Instruction::Call(wires.of(element).size));
            b.ins(Instruction::I32Add).set(1);
            b.get(2).num(1).ins(Instruction::I32Add).set(2);
            b.ins(Instruction::Br(0));
            b.ins(Instruction::End);
            b.ins(Instruction::End);
            b.get(1);
        }
    }
    b.finish()
}

/// 区画1つ分の byte 数を合計(局所1)へ足す
fn slot_size(b: &mut Body, wires: &Wires, slot: &Slot) {
    let size = wires.of(slot.layout).size;
    if !slot.indirect {
        b.get(1);
        b.get(0).offset(slot.offset);
        b.ins(Instruction::Call(size));
        b.ins(Instruction::I32Add).set(1);
        return;
    }
    // `indirect` の子は別の割り当て。optional なら 0 が `nil`
    b.get(0).offset(slot.offset).load().set(2);
    if slot.nullable {
        b.get(1).num(1).ins(Instruction::I32Add).set(1);
        b.get(2);
        b.ins(Instruction::If(BlockType::Empty));
        b.get(1).get(2).ins(Instruction::Call(size));
        b.ins(Instruction::I32Add).set(1);
        b.ins(Instruction::End);
        return;
    }
    b.get(1).get(2).ins(Instruction::Call(size));
    b.ins(Instruction::I32Add).set(1);
}

/// 配列の `添字`(局所2)番目の要素アドレスを積む。根は局所0
fn element_address(b: &mut Body, stride: u32) {
    b.get(0).ins(Instruction::I32Load(word(BUFFER_DATA)));
    b.get(2).num(stride).ins(Instruction::I32Mul);
    b.ins(Instruction::I32Add);
}

/// `encode(ptr, cursor) -> i32`
fn encode(layouts: &Layouts, wires: &Wires, layout: LayoutId) -> wasm_encoder::Function {
    // 2: 作業, 3: 添字, 4: 上限
    let mut b = Body::with_locals(vec![(3, ValType::I32)]);
    match &layouts.get(layout).shape {
        Shape::Unit => {}
        Shape::Bool => {
            b.get(1).get(0).ins(Instruction::I32Load8U(byte(0)));
            b.ins(Instruction::I32Store8(byte(0)));
            b.get(1).num(1).ins(Instruction::I32Add).set(1);
        }
        Shape::Int => {
            b.get(1).get(0).ins(Instruction::I64Load(wide(0)));
            b.ins(Instruction::I64Store(loose(0)));
            b.get(1).num(8).ins(Instruction::I32Add).set(1);
        }
        Shape::Tag(_) => {
            b.get(1).get(0).ins(Instruction::I32Load(word(0)));
            b.ins(Instruction::I32Store(loose(0)));
            b.get(1).num(4).ins(Instruction::I32Add).set(1);
        }
        Shape::Str => {
            b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN))).set(2);
            b.get(1).get(2).ins(Instruction::I32Store(loose(0)));
            b.get(1).num(4).ins(Instruction::I32Add).set(1);
            b.get(1)
                .get(0)
                .ins(Instruction::I32Load(word(BUFFER_DATA)))
                .get(2);
            b.ins(Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            });
            b.get(1).get(2).ins(Instruction::I32Add).set(1);
        }
        Shape::Optional { payload } => {
            let payload = *payload;
            b.get(0).ins(Instruction::I32Load8U(byte(0))).set(2);
            b.get(1).get(2).ins(Instruction::I32Store8(byte(0)));
            b.get(1).num(1).ins(Instruction::I32Add).set(1);
            b.get(2);
            b.ins(Instruction::If(BlockType::Empty));
            b.get(0).offset(payload.offset).get(1);
            b.ins(Instruction::Call(wires.of(payload.layout).encode))
                .set(1);
            b.ins(Instruction::End);
        }
        Shape::Struct { fields, .. } => {
            for slot in fields {
                slot_encode(&mut b, wires, slot);
            }
        }
        Shape::Enum { variants, .. } => {
            b.get(0).ins(Instruction::I32Load(word(0))).set(4);
            b.get(1).get(4).ins(Instruction::I32Store(loose(0)));
            b.get(1).num(4).ins(Instruction::I32Add).set(1);
            for (tag, slots) in variants.iter().enumerate() {
                if slots.is_empty() {
                    continue;
                }
                b.get(4).num(tag as u32).ins(Instruction::I32Eq);
                b.ins(Instruction::If(BlockType::Empty));
                for slot in slots {
                    slot_encode(&mut b, wires, slot);
                }
                b.ins(Instruction::End);
            }
        }
        Shape::Array { element, stride } => {
            let (element, stride) = (*element, *stride);
            b.get(0).ins(Instruction::I32Load(word(BUFFER_LEN))).set(4);
            b.get(1).get(4).ins(Instruction::I32Store(loose(0)));
            b.get(1).num(4).ins(Instruction::I32Add).set(1);
            b.num(0).set(3);
            b.ins(Instruction::Block(BlockType::Empty));
            b.ins(Instruction::Loop(BlockType::Empty));
            b.get(3).get(4).ins(Instruction::I32GeU);
            b.ins(Instruction::BrIf(1));
            b.get(0).ins(Instruction::I32Load(word(BUFFER_DATA)));
            b.get(3).num(stride).ins(Instruction::I32Mul);
            b.ins(Instruction::I32Add);
            b.get(1);
            b.ins(Instruction::Call(wires.of(element).encode)).set(1);
            b.get(3).num(1).ins(Instruction::I32Add).set(3);
            b.ins(Instruction::Br(0));
            b.ins(Instruction::End);
            b.ins(Instruction::End);
        }
    }
    b.get(1);
    b.finish()
}

/// 区画1つを書き出して cursor(局所1)を進める
fn slot_encode(b: &mut Body, wires: &Wires, slot: &Slot) {
    let encode = wires.of(slot.layout).encode;
    if !slot.indirect {
        b.get(0).offset(slot.offset).get(1);
        b.ins(Instruction::Call(encode)).set(1);
        return;
    }
    b.get(0).offset(slot.offset).load().set(2);
    if slot.nullable {
        // 子アドレスの有無がそのまま optional の tag
        b.get(1);
        b.get(2).ins(Instruction::I32Eqz).ins(Instruction::I32Eqz);
        b.ins(Instruction::I32Store8(byte(0)));
        b.get(1).num(1).ins(Instruction::I32Add).set(1);
        b.get(2);
        b.ins(Instruction::If(BlockType::Empty));
        b.get(2).get(1).ins(Instruction::Call(encode)).set(1);
        b.ins(Instruction::End);
        return;
    }
    b.get(2).get(1).ins(Instruction::Call(encode)).set(1);
}

/// `decode(cursor, end, dst) -> i32`。境界と正準性を見てから書く
fn decode(
    program: &hir::Program,
    layouts: &Layouts,
    wires: &Wires,
    indices: &Indices,
    layout: LayoutId,
) -> wasm_encoder::Function {
    // 3: 作業, 4: 添字, 5: 上限
    let mut b = Body::with_locals(vec![(3, ValType::I32)]);
    match &layouts.get(layout).shape {
        Shape::Unit => {}
        Shape::Bool => {
            need(&mut b, 1);
            b.get(0).ins(Instruction::I32Load8U(byte(0))).set(3);
            // 0 か 1 以外は正準な Boolean ではない
            b.get(3).num(1).ins(Instruction::I32GtU);
            b.trap_if();
            b.get(2).get(3).ins(Instruction::I32Store8(byte(0)));
            b.get(0).num(1).ins(Instruction::I32Add).set(0);
        }
        Shape::Int => {
            need(&mut b, 8);
            b.get(2).get(0).ins(Instruction::I64Load(loose(0)));
            b.ins(Instruction::I64Store(wide(0)));
            b.get(0).num(8).ins(Instruction::I32Add).set(0);
        }
        Shape::Tag(id) => {
            let count = program.enums[*id].variants.len() as u32;
            need(&mut b, 4);
            b.get(0).ins(Instruction::I32Load(loose(0))).set(3);
            b.get(3).num(count).ins(Instruction::I32GeU);
            b.trap_if();
            b.get(2).get(3).ins(Instruction::I32Store(word(0)));
            b.get(0).num(4).ins(Instruction::I32Add).set(0);
        }
        Shape::Str => {
            need(&mut b, 4);
            b.get(0).ins(Instruction::I32Load(loose(0))).set(3);
            b.get(0).num(4).ins(Instruction::I32Add).set(0);
            need_dynamic(&mut b, 3);
            b.get(0).get(3).ins(Instruction::Call(wires.utf8));

            b.get(3).ins(Instruction::Call(indices.alloc)).set(5);
            b.get(5).get(0).get(3);
            b.ins(Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            });
            b.get(2)
                .get(5)
                .ins(Instruction::I32Store(word(BUFFER_DATA)));
            b.get(2).get(3).ins(Instruction::I32Store(word(BUFFER_LEN)));
            b.get(2)
                .get(3)
                .ins(Instruction::I32Store(word(BUFFER_CAPACITY)));
            b.get(0).get(3).ins(Instruction::I32Add).set(0);
        }
        Shape::Optional { payload } => {
            let payload = *payload;
            need(&mut b, 1);
            b.get(0).ins(Instruction::I32Load8U(byte(0))).set(3);
            b.get(3).num(1).ins(Instruction::I32GtU);
            b.trap_if();
            b.get(2).get(3).ins(Instruction::I32Store8(byte(0)));
            b.get(0).num(1).ins(Instruction::I32Add).set(0);
            b.get(3);
            b.ins(Instruction::If(BlockType::Empty));
            b.get(0).get(1).get(2).offset(payload.offset);
            b.ins(Instruction::Call(wires.of(payload.layout).decode))
                .set(0);
            b.ins(Instruction::End);
        }
        Shape::Struct { fields, .. } => {
            for slot in fields {
                slot_decode(&mut b, wires, indices, layouts, slot);
            }
        }
        Shape::Enum { variants, .. } => {
            need(&mut b, 4);
            b.get(0).ins(Instruction::I32Load(loose(0))).set(3);
            b.get(3).num(variants.len() as u32).ins(Instruction::I32GeU);
            b.trap_if();
            b.get(2).get(3).ins(Instruction::I32Store(word(0)));
            b.get(0).num(4).ins(Instruction::I32Add).set(0);
            for (tag, slots) in variants.iter().enumerate() {
                if slots.is_empty() {
                    continue;
                }
                b.get(3).num(tag as u32).ins(Instruction::I32Eq);
                b.ins(Instruction::If(BlockType::Empty));
                for slot in slots {
                    slot_decode(&mut b, wires, indices, layouts, slot);
                }
                b.ins(Instruction::End);
            }
        }
        Shape::Array { element, stride } => {
            let (element, stride) = (*element, *stride);
            need(&mut b, 4);
            b.get(0).ins(Instruction::I32Load(loose(0))).set(3);
            b.get(0).num(4).ins(Instruction::I32Add).set(0);
            // 要素の刻み × 個数が 32bit を越えるなら、その配列は存在しない
            if stride != 0 {
                b.get(3).num(u32::MAX / stride).ins(Instruction::I32GtU);
                b.trap_if();
            }
            b.get(3).num(stride).ins(Instruction::I32Mul).set(5);
            b.get(5).ins(Instruction::Call(indices.alloc)).set(5);
            b.get(2)
                .get(5)
                .ins(Instruction::I32Store(word(BUFFER_DATA)));
            b.get(2).get(3).ins(Instruction::I32Store(word(BUFFER_LEN)));
            b.get(2)
                .get(3)
                .ins(Instruction::I32Store(word(BUFFER_CAPACITY)));

            b.num(0).set(4);
            b.ins(Instruction::Block(BlockType::Empty));
            b.ins(Instruction::Loop(BlockType::Empty));
            b.get(4).get(3).ins(Instruction::I32GeU);
            b.ins(Instruction::BrIf(1));
            b.get(0).get(1);
            b.get(2).ins(Instruction::I32Load(word(BUFFER_DATA)));
            b.get(4).num(stride).ins(Instruction::I32Mul);
            b.ins(Instruction::I32Add);
            b.ins(Instruction::Call(wires.of(element).decode)).set(0);
            b.get(4).num(1).ins(Instruction::I32Add).set(4);
            b.ins(Instruction::Br(0));
            b.ins(Instruction::End);
            b.ins(Instruction::End);
        }
    }
    b.get(0);
    b.finish()
}

/// 区画1つを読み込んで cursor(局所0)を進める
fn slot_decode(b: &mut Body, wires: &Wires, indices: &Indices, layouts: &Layouts, slot: &Slot) {
    let decode = wires.of(slot.layout).decode;
    if !slot.indirect {
        b.get(0).get(1).get(2).offset(slot.offset);
        b.ins(Instruction::Call(decode)).set(0);
        return;
    }
    let size = layouts.extent(slot.layout).size;
    if slot.nullable {
        need(b, 1);
        b.get(0).ins(Instruction::I32Load8U(byte(0))).set(3);
        b.get(3).num(1).ins(Instruction::I32GtU);
        b.trap_if();
        b.get(0).num(1).ins(Instruction::I32Add).set(0);
        b.get(3);
        b.ins(Instruction::If(BlockType::Empty));
        b.num(size).ins(Instruction::Call(indices.alloc)).set(3);
        b.get(2).offset(slot.offset).get(3).store();
        b.get(0).get(1).get(3);
        b.ins(Instruction::Call(decode)).set(0);
        b.ins(Instruction::Else);
        b.get(2).offset(slot.offset).num(0).store();
        b.ins(Instruction::End);
        return;
    }
    b.num(size).ins(Instruction::Call(indices.alloc)).set(3);
    b.get(2).offset(slot.offset).get(3).store();
    b.get(0).get(1).get(3);
    b.ins(Instruction::Call(decode)).set(0);
}

/// 固定長ぶんの余地があるか。足りなければ trap(局所0 が cursor、1 が上限)
fn need(b: &mut Body, bytes: u32) {
    b.get(1).get(0).ins(Instruction::I32Sub);
    b.num(bytes).ins(Instruction::I32LtU);
    b.trap_if();
}

/// 可変長ぶんの余地があるか。`length` の局所に byte 数が入っている前提
fn need_dynamic(b: &mut Body, length: u32) {
    b.get(1).get(0).ins(Instruction::I32Sub);
    b.get(length).ins(Instruction::I32LtU);
    b.trap_if();
}

// ---------------------------------------------------------------------------
// UTF-8 の検証
// ---------------------------------------------------------------------------

/// `utf8_ok(ptr, len)`。正準でない並びは trap する。
///
/// Unicode の表 3-7 そのまま。先頭 byte が続きの本数と**2 byte 目の許される
/// 範囲**を決め、3・4 byte 目は常に `80..BF`。これで overlong と surrogate も
/// 同時に弾ける
fn utf8_ok() -> wasm_encoder::Function {
    // 2: 添字, 3: 先頭 byte, 4: 続きの本数, 5: 2 byte 目の下限, 6: 上限, 7: 作業
    let mut b = Body::with_locals(vec![(6, ValType::I32)]);
    b.ins(Instruction::Block(BlockType::Empty));
    b.ins(Instruction::Loop(BlockType::Empty));
    b.get(2).get(1).ins(Instruction::I32GeU);
    b.ins(Instruction::BrIf(1));

    b.get(0).get(2).ins(Instruction::I32Add);
    b.ins(Instruction::I32Load8U(byte(0))).set(3);

    // ASCII はそのまま1 byte
    b.get(3).num(0x80).ins(Instruction::I32LtU);
    b.ins(Instruction::If(BlockType::Empty));
    b.get(2).num(1).ins(Instruction::I32Add).set(2);
    b.ins(Instruction::Br(2));
    b.ins(Instruction::End);

    // 先頭 byte から続きの本数と2 byte 目の範囲を引く
    b.num(0).set(4);
    b.num(0x80).set(5);
    b.num(0xBF).set(6);
    for (lo, hi, follow, second_lo, second_hi) in [
        (0xC2u32, 0xDFu32, 1u32, 0x80u32, 0xBFu32),
        (0xE0, 0xE0, 2, 0xA0, 0xBF),
        (0xE1, 0xEC, 2, 0x80, 0xBF),
        (0xED, 0xED, 2, 0x80, 0x9F),
        (0xEE, 0xEF, 2, 0x80, 0xBF),
        (0xF0, 0xF0, 3, 0x90, 0xBF),
        (0xF1, 0xF3, 3, 0x80, 0xBF),
        (0xF4, 0xF4, 3, 0x80, 0x8F),
    ] {
        b.get(3).num(lo).ins(Instruction::I32GeU);
        b.get(3).num(hi).ins(Instruction::I32LeU);
        b.ins(Instruction::I32And);
        b.ins(Instruction::If(BlockType::Empty));
        b.num(follow).set(4);
        b.num(second_lo).set(5);
        b.num(second_hi).set(6);
        b.ins(Instruction::End);
    }
    // どの範囲にも入らなかった先頭 byte は不正
    b.get(4).ins(Instruction::I32Eqz);
    b.trap_if();

    // 続きの byte がその場に居るか
    b.get(1).get(2).ins(Instruction::I32Sub);
    b.get(4)
        .num(1)
        .ins(Instruction::I32Add)
        .ins(Instruction::I32LtU);
    b.trap_if();

    // 2 byte 目だけは先頭 byte が決めた範囲、残りは 80..BF
    b.num(1).set(7);
    b.ins(Instruction::Block(BlockType::Empty));
    b.ins(Instruction::Loop(BlockType::Empty));
    b.get(7).get(4).ins(Instruction::I32GtU);
    b.ins(Instruction::BrIf(1));
    b.get(0).get(2).ins(Instruction::I32Add).get(7);
    b.ins(Instruction::I32Add)
        .ins(Instruction::I32Load8U(byte(0)));
    b.set(3);
    b.get(7).num(1).ins(Instruction::I32Eq);
    b.ins(Instruction::If(BlockType::Empty));
    b.get(3).get(5).ins(Instruction::I32LtU);
    b.get(3).get(6).ins(Instruction::I32GtU);
    b.ins(Instruction::I32Or);
    b.trap_if();
    b.ins(Instruction::Else);
    b.get(3).num(0x80).ins(Instruction::I32LtU);
    b.get(3).num(0xBF).ins(Instruction::I32GtU);
    b.ins(Instruction::I32Or);
    b.trap_if();
    b.ins(Instruction::End);
    b.get(7).num(1).ins(Instruction::I32Add).set(7);
    b.ins(Instruction::Br(0));
    b.ins(Instruction::End);
    b.ins(Instruction::End);

    b.get(2).get(4).ins(Instruction::I32Add);
    b.num(1).ins(Instruction::I32Add).set(2);
    b.ins(Instruction::Br(0));
    b.ins(Instruction::End);
    b.ins(Instruction::End);
    b.finish()
}

// ---------------------------------------------------------------------------
// 受け渡し領域
// ---------------------------------------------------------------------------

/// ホストが書いた slice が、最後に予約した受け渡し領域に収まっているか。
///
/// 収まっていなければ本体へ入る前に trap する。ホストが書いてよいのはここだけ
/// で、それ以外の記憶はモジュールの持ち物(design.md のリスク欄)
pub fn check_slice(b: &mut Body, exchange: u32, pointer: u32, length: u32, scratch: u32) {
    // まだ何も予約していないなら、正しい slice はあり得ない
    b.num(exchange).load().set(scratch);
    b.get(scratch).ins(Instruction::I32Eqz);
    b.trap_if();

    // 予約した割り当ての大きさは、その block header が知っている
    b.get(pointer).get(scratch).ins(Instruction::I32LtU);
    b.trap_if();
    b.get(pointer)
        .get(length)
        .ins(Instruction::I32Add)
        .get(pointer)
        .ins(Instruction::I32LtU);
    b.trap_if();
    b.get(pointer).get(length).ins(Instruction::I32Add);
    b.get(scratch)
        .get(scratch)
        .num(HEADER)
        .ins(Instruction::I32Sub)
        .load()
        .ins(Instruction::I32Add);
    b.ins(Instruction::I32GtU);
    b.trap_if();
}
