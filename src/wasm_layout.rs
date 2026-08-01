//! 所有データの線形メモリ上の並び(design.md 決定3)。
//!
//! ここが答えるのは「この型は何バイトで、中身はどこにあるか」だけ。割り当ても
//! 命令も出さないし、名前解決・型推論・借用検査もやり直さない。入力は検査を
//! 通った `hir::Type` と宣言の ID で、同じ入力からは必ず同じ並びが出る。
//!
//! 並びはコンパイラの内部事情なので、ホストへは公開しない。境界を渡るのは
//! `wasm_abi` が直列化した bytes のほう(design.md 決定8)。

// ponytail: 並びを読むのは後続スライスの `wasm_runtime` / `wasm_data`。それらが
// 入るまでは本体から呼ばれない定数と型が残るので、ここだけ許す。消し忘れると
// 本物の死にコードを隠すので、data 下ろしが入ったらこの許可を外すこと
#![allow(dead_code)]

use crate::hir;
use std::collections::BTreeMap;

/// 実行時のアドレス幅。線形メモリは 32bit なので `i32` 1つ
pub const ADDR_SIZE: u32 = 4;

/// 揃えの上限。これより大きい揃えは要らないし、要求もしない
pub const MAX_ALIGN: u32 = 8;

/// `str` と `[T]` が根に持つ `{u32 data, u32 len, u32 capacity}`
pub const BUFFER_SIZE: u32 = 12;
pub const BUFFER_ALIGN: u32 = 4;
pub const BUFFER_DATA: u32 = 0;
pub const BUFFER_LEN: u32 = 4;
pub const BUFFER_CAPACITY: u32 = 8;

/// optional の tag。0 が空、1 が有り
pub const OPTIONAL_EMPTY: u32 = 0;
pub const OPTIONAL_PRESENT: u32 = 1;

/// 計画した並びの ID。番号は要求された順に振るので決定的
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct LayoutId(u32);

impl LayoutId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// 32bit の記憶に収まらなかった、という報告。呼ぶ側が span を足して診断にする
#[derive(Debug, PartialEq, Eq)]
pub struct Overflow(pub String);

/// 記憶1つ分の大きさと揃え。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Extent {
    pub size: u32,
    pub align: u32,
}

impl Extent {
    /// 実行時表現を持たない値(`unit`)
    pub const ZERO: Extent = Extent { size: 0, align: 1 };

    fn scalar(size: u32, align: u32) -> Extent {
        Extent { size, align }
    }
}

/// 記憶の1区画。struct のフィールド、enum の payload 位置がこれになる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Slot {
    /// 根からの byte offset
    pub offset: u32,
    /// `indirect` なら**指した先**の並び。直接なら区画そのものの並び
    pub layout: LayoutId,
    /// `indirect` の区画は `u32` の子アドレス1つ。ここが再帰の層を切る
    /// (design.md 決定3)
    pub indirect: bool,
    /// アドレス 0 に意味があるか。`indirect next: Node?` の 0 は `nil` で、
    /// optional でない `indirect` の 0 は不正
    pub nullable: bool,
}

impl Slot {
    /// 区画そのものが占める大きさ。`indirect` はアドレス1つぶん
    fn extent(&self, layouts: &Layouts) -> Extent {
        if self.indirect {
            Extent::scalar(ADDR_SIZE, ADDR_SIZE)
        } else {
            layouts.extent(self.layout)
        }
    }
}

/// 型ごとの中身の形。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Shape {
    /// 実行時表現を持たない
    Unit,
    /// 1 byte の 0/1
    Bool,
    /// 8 byte の符号付き整数
    Int,
    /// payload を持たない enum。宣言順の `u32` タグだけ
    Tag(hir::EnumId),
    /// `{u8 tag, padding, payload}`
    Optional { payload: Slot },
    /// 宣言順のフィールドを直に並べる
    Struct {
        id: hir::StructId,
        fields: Vec<Slot>,
    },
    /// `{u32 variant, padding, いちばん大きい payload}`。初期化されるのは
    /// そのとき活きている variant の payload だけ
    Enum {
        id: hir::EnumId,
        /// 宣言順の variant ごとの payload 区画
        variants: Vec<Vec<Slot>>,
    },
    /// `{u32 data, u32 len, u32 capacity}` + 別割り当ての UTF-8 bytes
    Str,
    /// `{u32 data, u32 len, u32 capacity}` + 別割り当ての要素列
    Array { element: LayoutId, stride: u32 },
}

/// 型1つ分の計画。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    pub extent: Extent,
    pub shape: Shape,
    /// 暗黙に複製してよい値か。所有の受け渡しが要るのは false のほうだけ
    pub copy: bool,
}

/// 到達した型の並びを、要求された順に溜める。
///
/// 同じ型は必ず同じ `LayoutId` になる。鍵は**浅い**型(`Struct(id)` など)なので
/// 再帰型でも鍵そのものは有限。循環は `indirect` が切るが、万一直接の循環が
/// 来ても ID を先に予約しておくので無限に潜らない(design.md 決定5と同じ手)
#[derive(Default)]
pub struct Layouts {
    entries: Vec<Option<Layout>>,
    ids: BTreeMap<Key, LayoutId>,
}

/// 並びの同一性。`hir::Type` から借用を落として正規化したもの
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Key {
    Unit,
    Bool,
    Int,
    Str,
    Struct(hir::StructId),
    Enum(hir::EnumId),
    Array(Box<Key>),
    Optional(Box<Key>),
}

impl Layouts {
    /// 型1つ分の並びを計画する。すでに計画済みなら同じ ID を返す。
    ///
    /// 借用は「検査済みの場所を指す `i32`」なので並びを持たない。ここへ来る前に
    /// 剥がしておくこと
    pub fn plan(&mut self, program: &hir::Program, ty: &hir::Type) -> Result<LayoutId, Overflow> {
        debug_assert!(ty.reference.is_none(), "借用に記憶の並びは無い");
        let key = self.key(program, ty)?;
        self.plan_key(program, &key)
    }

    pub fn get(&self, id: LayoutId) -> &Layout {
        self.entries[id.index()]
            .as_ref()
            .expect("計画の途中の並びを読んだ")
    }

    pub fn extent(&self, id: LayoutId) -> Extent {
        self.get(id).extent
    }

    /// 計画した順の全体。関数番号を振る側が決定的に歩くための入口
    pub fn planned(&self) -> impl Iterator<Item = (LayoutId, &Layout)> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (LayoutId(index as u32), entry.as_ref().expect("計画済み")))
    }

    fn key(&self, program: &hir::Program, ty: &hir::Type) -> Result<Key, Overflow> {
        let inner = match &ty.kind {
            hir::TypeKind::Builtin(hir::Builtin::Unit) => Key::Unit,
            hir::TypeKind::Builtin(hir::Builtin::Bool) => Key::Bool,
            hir::TypeKind::Builtin(hir::Builtin::Int) => Key::Int,
            hir::TypeKind::Builtin(hir::Builtin::Str) => Key::Str,
            hir::TypeKind::Struct(id) => Key::Struct(*id),
            hir::TypeKind::Enum(id) => Key::Enum(*id),
            hir::TypeKind::Array(element) => Key::Array(Box::new(self.key(program, element)?)),
            // poison は診断を伴うときだけ存在する。生成まで来ないはず
            hir::TypeKind::Poison => {
                return Err(Overflow("診断済みの型に記憶の並びはありません".into()));
            }
        };
        Ok(if ty.optional {
            Key::Optional(Box::new(inner))
        } else {
            inner
        })
    }

    fn plan_key(&mut self, program: &hir::Program, key: &Key) -> Result<LayoutId, Overflow> {
        if let Some(found) = self.ids.get(key) {
            return Ok(*found);
        }
        // 子を見に行く前に番号を押さえる。循環しても同じ ID へ戻るだけで済む
        let id = LayoutId(self.entries.len() as u32);
        self.entries.push(None);
        self.ids.insert(key.clone(), id);

        let layout = self.compute(program, key)?;
        self.entries[id.index()] = Some(layout);
        Ok(id)
    }

    fn compute(&mut self, program: &hir::Program, key: &Key) -> Result<Layout, Overflow> {
        Ok(match key {
            Key::Unit => Layout {
                extent: Extent::ZERO,
                shape: Shape::Unit,
                copy: true,
            },
            Key::Bool => Layout {
                extent: Extent::scalar(1, 1),
                shape: Shape::Bool,
                copy: true,
            },
            Key::Int => Layout {
                extent: Extent::scalar(8, 8),
                shape: Shape::Int,
                copy: true,
            },
            Key::Str => Layout {
                extent: Extent::scalar(BUFFER_SIZE, BUFFER_ALIGN),
                shape: Shape::Str,
                copy: false,
            },
            Key::Array(element) => {
                let element = self.plan_key(program, element)?;
                // 要素は自分の揃えで刻む。読み書きが offset を掛け算で出せる
                let extent = self.extent(element);
                let stride = align_to(extent.size, extent.align)?;
                Layout {
                    extent: Extent::scalar(BUFFER_SIZE, BUFFER_ALIGN),
                    shape: Shape::Array { element, stride },
                    copy: false,
                }
            }
            Key::Optional(inner) => {
                let payload = self.plan_key(program, inner)?;
                let payload_extent = self.extent(payload);
                let align = payload_extent.align.max(1);
                let offset = align_to(1, align)?;
                let size = align_to(checked_add(offset, payload_extent.size)?, align)?;
                Layout {
                    extent: Extent { size, align },
                    shape: Shape::Optional {
                        payload: Slot {
                            offset,
                            layout: payload,
                            indirect: false,
                            nullable: false,
                        },
                    },
                    copy: self.get(payload).copy,
                }
            }
            Key::Struct(id) => {
                let mut cursor = Cursor::default();
                let mut fields = Vec::new();
                for field in &program.structs[*id].fields {
                    let decl = &program.fields[*field];
                    let slot = self.slot(program, &decl.ty, decl.indirect)?;
                    fields.push(cursor.place(self, slot)?);
                }
                Layout {
                    extent: cursor.finish()?,
                    shape: Shape::Struct { id: *id, fields },
                    copy: false,
                }
            }
            Key::Enum(id) => self.enum_layout(program, *id)?,
        })
    }

    /// enum は payload の有無で形が変わる。payload なしはタグだけの Copy 値
    fn enum_layout(&mut self, program: &hir::Program, id: hir::EnumId) -> Result<Layout, Overflow> {
        let decl = &program.enums[id];
        if decl
            .variants
            .iter()
            .all(|v| program.variants[*v].payload.is_empty())
        {
            return Ok(Layout {
                extent: Extent::scalar(4, 4),
                shape: Shape::Tag(id),
                copy: true,
            });
        }

        // payload はいちばん大きいものに合わせて1区画。活きている variant の
        // ぶんだけが初期化される
        let mut variants = Vec::new();
        let mut payload = Extent::ZERO;
        for variant in &decl.variants {
            let mut cursor = Cursor::default();
            let mut slots = Vec::new();
            for position in &program.variants[*variant].payload {
                let slot = self.slot(program, &position.ty, position.indirect)?;
                slots.push(cursor.place(self, slot)?);
            }
            let extent = cursor.finish()?;
            payload = Extent {
                size: payload.size.max(extent.size),
                align: payload.align.max(extent.align),
            };
            variants.push(slots);
        }

        let align = payload.align.max(4);
        let offset = align_to(4, payload.align.max(1))?;
        let size = align_to(checked_add(offset, payload.size)?, align)?;
        // payload 区画の先頭を揃えたので、各 variant の offset をそこへ寄せる
        for slots in &mut variants {
            for slot in slots {
                slot.offset = checked_add(slot.offset, offset)?;
            }
        }
        Ok(Layout {
            extent: Extent { size, align },
            shape: Shape::Enum { id, variants },
            copy: false,
        })
    }

    /// フィールド/payload 位置1つ。`indirect` の先も並びは要る(glue が潜る)。
    ///
    /// `indirect` が指すのは optional を剥がした**本体**。`Node?` の並びを間に
    /// 挟まないので、`Node` を計画している最中でもその大きさを読まずに済む
    fn slot(
        &mut self,
        program: &hir::Program,
        ty: &hir::Type,
        indirect: bool,
    ) -> Result<Slot, Overflow> {
        if indirect {
            let mut pointee = ty.clone();
            pointee.optional = false;
            return Ok(Slot {
                offset: 0,
                layout: self.plan(program, &pointee)?,
                indirect: true,
                nullable: ty.optional,
            });
        }
        Ok(Slot {
            offset: 0,
            layout: self.plan(program, ty)?,
            indirect: false,
            nullable: false,
        })
    }
}

/// 宣言順に区画を積む。詰め物は各区画の揃えぶんだけ
#[derive(Default)]
struct Cursor {
    size: u32,
    align: u32,
}

impl Cursor {
    fn place(&mut self, layouts: &Layouts, mut slot: Slot) -> Result<Slot, Overflow> {
        let extent = slot.extent(layouts);
        let align = extent.align.max(1);
        slot.offset = align_to(self.size, align)?;
        self.size = checked_add(slot.offset, extent.size)?;
        self.align = self.align.max(align);
        Ok(slot)
    }

    fn finish(self) -> Result<Extent, Overflow> {
        let align = self.align.max(1);
        Ok(Extent {
            size: align_to(self.size, align)?,
            align,
        })
    }
}

/// `value` を `align` の倍数まで押し上げる。32bit を越えたら報告する
pub fn align_to(value: u32, align: u32) -> Result<u32, Overflow> {
    debug_assert!(align.is_power_of_two() && align <= MAX_ALIGN, "揃えが不正");
    let aligned = (u64::from(value) + u64::from(align) - 1) & !(u64::from(align) - 1);
    narrow(aligned)
}

pub fn checked_add(left: u32, right: u32) -> Result<u32, Overflow> {
    narrow(u64::from(left) + u64::from(right))
}

/// 要素数 × 刻み。配列の割り当てと添字がここを通る
pub fn checked_mul(count: u32, stride: u32) -> Result<u32, Overflow> {
    narrow(u64::from(count) * u64::from(stride))
}

fn narrow(value: u64) -> Result<u32, Overflow> {
    u32::try_from(value)
        .map_err(|_| Overflow(format!("{value} byte は 32bit の記憶に収まりません")))
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// ソースから型検査済みの HIR を作る。並びの検査に要るのは宣言だけ
    fn program_of(src: &str) -> hir::Program {
        let parsed = crate::parse::parse(&crate::lex::join(crate::lex::lex(src).unwrap()))
            .expect("パースできるはず");
        crate::typecheck::check_and_lower(&parsed).expect("型検査を通るはず")
    }

    fn named(program: &hir::Program, name: &str) -> hir::Type {
        if let Some((id, _)) = program.structs.iter().find(|(_, d)| d.name == name) {
            return hir::Type {
                reference: None,
                kind: hir::TypeKind::Struct(id),
                optional: false,
            };
        }
        let (id, _) = program
            .enums
            .iter()
            .find(|(_, d)| d.name == name)
            .expect("その宣言がない");
        hir::Type {
            reference: None,
            kind: hir::TypeKind::Enum(id),
            optional: false,
        }
    }

    fn optional(mut ty: hir::Type) -> hir::Type {
        ty.optional = true;
        ty
    }

    fn plan(program: &hir::Program, ty: &hir::Type) -> (Layouts, LayoutId) {
        let mut layouts = Layouts::default();
        let id = layouts.plan(program, ty).expect("計画できるはず");
        (layouts, id)
    }

    // -----------------------------------------------------------------------
    // scalar・タグ・Copy optional(tasks 1.2)
    // -----------------------------------------------------------------------

    #[test]
    fn scalarの大きさと揃えは固定() {
        let program = hir::Program::default();
        for (builtin, size, align, copy) in [
            (hir::Builtin::Unit, 0, 1, true),
            (hir::Builtin::Bool, 1, 1, true),
            (hir::Builtin::Int, 8, 8, true),
            (hir::Builtin::Str, BUFFER_SIZE, BUFFER_ALIGN, false),
        ] {
            let (layouts, id) = plan(&program, &hir::Type::builtin(builtin));
            assert_eq!(layouts.extent(id), Extent { size, align }, "{builtin:?}");
            assert_eq!(layouts.get(id).copy, copy, "{builtin:?}");
        }
    }

    #[test]
    fn payloadを持たないenumはタグだけのcopy値() {
        let program = program_of("enum Color { Red\n Green\n Blue }\nfn main() { assert true }\n");
        let (layouts, id) = plan(&program, &named(&program, "Color"));
        assert_eq!(layouts.extent(id), Extent { size: 4, align: 4 });
        assert!(layouts.get(id).copy);
        assert!(matches!(layouts.get(id).shape, Shape::Tag(_)));
    }

    /// `{u8 tag, padding, payload}`。payload の揃えが詰め物を決める
    #[test]
    fn copy_optionalはタグと詰め物と本体になる() {
        let program = hir::Program::default();
        for (builtin, offset, size, align) in [
            (hir::Builtin::Unit, 1, 1, 1),
            (hir::Builtin::Bool, 1, 2, 1),
            (hir::Builtin::Int, 8, 16, 8),
        ] {
            let (layouts, id) = plan(&program, &optional(hir::Type::builtin(builtin)));
            assert_eq!(layouts.extent(id), Extent { size, align }, "{builtin:?}");
            assert!(layouts.get(id).copy, "{builtin:?}");
            let Shape::Optional { payload } = layouts.get(id).shape else {
                panic!("optional ではない");
            };
            assert_eq!(payload.offset, offset, "{builtin:?}");
        }
    }

    // -----------------------------------------------------------------------
    // 複合型(tasks 1.3)
    // -----------------------------------------------------------------------

    /// フィールドは宣言順。並べ替えないので、詰め物はソースの書き方に従う
    #[test]
    fn structのフィールドは宣言順に詰め物付きで並ぶ() {
        let program = program_of(
            "struct Mixed { flag: bool\n count: int\n other: bool }\n\
             fn main() { assert true }\n",
        );
        let (layouts, id) = plan(&program, &named(&program, "Mixed"));
        let Shape::Struct { fields, .. } = &layouts.get(id).shape else {
            panic!("struct ではない");
        };
        assert_eq!(
            fields.iter().map(|f| f.offset).collect::<Vec<_>>(),
            [0, 8, 16]
        );
        assert_eq!(layouts.extent(id), Extent { size: 24, align: 8 });
        assert!(!layouts.get(id).copy);
    }

    #[test]
    fn フィールドを持たないstructは場所を取らない() {
        let program = program_of("struct Marker {}\nfn main() { assert true }\n");
        let (layouts, id) = plan(&program, &named(&program, "Marker"));
        assert_eq!(layouts.extent(id), Extent { size: 0, align: 1 });
    }

    /// payload を持つ enum は `{u32 variant, padding, 最大の payload}`
    #[test]
    fn payload付きenumは最大のvariantに合わせる() {
        let program = program_of(
            "enum Shape { Dot\n Line(int)\n Box(int, bool) }\n\
             fn main() { assert true }\n",
        );
        let (layouts, id) = plan(&program, &named(&program, "Shape"));
        assert_eq!(layouts.extent(id), Extent { size: 24, align: 8 });
        assert!(!layouts.get(id).copy);
        let Shape::Enum { variants, .. } = &layouts.get(id).shape else {
            panic!("enum ではない");
        };
        // Dot は payload なし、Line は int 1つ、Box は int + bool
        assert_eq!(variants[0], []);
        assert_eq!(
            variants[1].iter().map(|s| s.offset).collect::<Vec<_>>(),
            [8]
        );
        assert_eq!(
            variants[2].iter().map(|s| s.offset).collect::<Vec<_>>(),
            [8, 16]
        );
    }

    /// `[T]` は根が固定長の帳簿。要素は別割り当てに刻みで並ぶ
    #[test]
    fn 配列の根は固定長で要素は刻みで並ぶ() {
        let program = program_of("struct Pair { a: int\n b: bool }\nfn main() { assert true }\n");
        let element = named(&program, "Pair");
        let array = hir::Type {
            reference: None,
            kind: hir::TypeKind::Array(Box::new(element)),
            optional: false,
        };
        let (layouts, id) = plan(&program, &array);
        assert_eq!(
            layouts.extent(id),
            Extent {
                size: BUFFER_SIZE,
                align: BUFFER_ALIGN
            }
        );
        let Shape::Array { stride, .. } = layouts.get(id).shape else {
            panic!("配列ではない");
        };
        assert_eq!(stride, 16);
    }

    /// optional は入れ子にできる。外側の tag は内側の並びの上に載る
    #[test]
    fn 入れ子のoptionalは外側のタグを重ねる() {
        let program = program_of("struct User { id: int }\nfn main() { assert true }\n");
        let (layouts, id) = plan(&program, &optional(named(&program, "User")));
        assert_eq!(layouts.extent(id), Extent { size: 16, align: 8 });
        let Shape::Optional { payload } = layouts.get(id).shape else {
            panic!("optional ではない");
        };
        assert_eq!(payload.offset, 8);
        assert!(!layouts.get(id).copy);
    }

    /// `indirect` はアドレス1つ。だから再帰しても根の大きさは有限
    #[test]
    fn indirectは子のアドレス1つになる() {
        let program = program_of(
            "struct Node { value: int\n indirect next: Node? }\n\
             fn main() { assert true }\n",
        );
        let (layouts, id) = plan(&program, &named(&program, "Node"));
        assert_eq!(layouts.extent(id), Extent { size: 16, align: 8 });
        let Shape::Struct { fields, .. } = &layouts.get(id).shape else {
            panic!("struct ではない");
        };
        assert_eq!(fields[1].offset, 8);
        assert!(fields[1].indirect);
        // `Node?` の 0 は `nil`。指す先は optional を剥がした Node 自身
        assert!(fields[1].nullable);
        assert_eq!(fields[1].layout, id);
    }

    #[test]
    fn 再帰enumもindirectで閉じる() {
        let program = program_of(
            "enum List { Nil\n Cons(int, indirect List) }\n\
             fn main() { assert true }\n",
        );
        let (layouts, id) = plan(&program, &named(&program, "List"));
        let Shape::Enum { variants, .. } = &layouts.get(id).shape else {
            panic!("enum ではない");
        };
        assert!(variants[1][1].indirect);
        // 子の並びは自分自身。予約した ID へ戻ってくる
        assert_eq!(variants[1][1].layout, id);
    }

    // -----------------------------------------------------------------------
    // 決定性と 32bit の限界(tasks 1.4)
    // -----------------------------------------------------------------------

    /// 同じ型は必ず同じ ID。歩く順が違っても計画の中身は変わらない
    #[test]
    fn 同じ型は同じidになる() {
        let program =
            program_of("struct User { id: int\n name: str }\nfn main() { assert true }\n");
        let mut layouts = Layouts::default();
        let user = named(&program, "User");
        let first = layouts.plan(&program, &user).unwrap();
        let second = layouts.plan(&program, &user).unwrap();
        assert_eq!(first, second);
        // 中身から先に頼んでも、User の ID は増えない
        let int = layouts
            .plan(&program, &hir::Type::builtin(hir::Builtin::Int))
            .unwrap();
        assert_ne!(int, first);
        assert_eq!(layouts.plan(&program, &user).unwrap(), first);
    }

    /// 計画の並びは要求順。二度同じ順で頼めば同じ表になる
    #[test]
    fn 計画の走査順は決定的() {
        let program = program_of(
            "struct User { id: int\n name: str }\n\
             fn main() { assert true }\n",
        );
        let shapes = |()| {
            let (layouts, _) = plan(&program, &named(&program, "User"));
            layouts
                .planned()
                .map(|(id, layout)| (id, layout.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(shapes(()), shapes(()));
    }

    #[test]
    fn 揃えと加算は32bitを越えたら報告する() {
        assert_eq!(align_to(8, 8), Ok(8));
        assert_eq!(align_to(9, 8), Ok(16));
        assert!(align_to(u32::MAX, 8).is_err());
        assert!(checked_add(u32::MAX, 1).is_err());
        assert_eq!(checked_mul(3, 16), Ok(48));
        assert!(checked_mul(u32::MAX, 2).is_err());
    }
}
