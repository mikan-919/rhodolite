// Rhodolite v1 の到達目標。
//
// このファイルが動き、要求が可視化され、提供忘れが到達経路付きで
// 報告されたら v1 は完了。言語機能はこれを動かすのに必要な分だけ実装する。
//
// 見るべき点: promote と handle が1文字も書いていないのに、
// stamp の要求が main まで届いていること。

// ---- 契約 ----

trait Database {
    fn find(&self, id: int -> User?)
    fn save(&mut self, u: User -> unit)
}

trait Clock {
    fn now(self -> int)
}

// ---- データ ----

// データを持たない enum。`Bronze` / `Gold` はここでだけ宣言される値で、
// 裸の名前として参照できる
enum Rank {
    Bronze
    Gold
}

struct User {
    id: int
    rank: Rank
    promoted_at: int
}

// ---- スロット宣言: 役割に名前を与える ----

effect db: Database
effect clock: Clock

// ---- 使用: スロット名で呼ぶ ----

fn stamp(u: &mut User) {
    u.promoted_at = clock.now()
}

// ---- 経由するだけ: 無記述 ----

fn promote(id: int -> bool) {
    let mut u = db.find(id) ?? return false
    u.rank = Gold
    stamp(&mut u)
    db.save(move u)
    true
}

fn handle(id: int -> bool) {
    promote(id)
}

// ---- 本番のハンドラ。専用構文は無い、ただの impl ----

struct Postgres {
    url: str
}

impl Postgres {
    fn new(url: str -> Postgres) {
        Postgres { url = url }
    }
}

impl Database for Postgres {
    // v1 に本物の接続は無いので空の DB として振る舞う。
    // 差し替えが動くことの検証は下の test が InMemoryDb でやる
    fn find(&self, id: int -> User?) {
        nil
    }
    fn save(&mut self, u: User -> unit) {
        let ignored = u
    }
}

struct SystemClock {}

impl Clock for SystemClock {
    // v1 に本物の時計は無い
    fn now(self -> int) {
        0
    }
}

// ---- 提供: Head + ブロック ----

fn main(-> bool) {
    let current_user_id = 1
    with db(Postgres::new("postgres://localhost/app")), clock(SystemClock {}) {
        handle(current_user_id)
    }
}

// ---- 差し替え用のハンドラ。同じ trait の別の impl でしかない ----

struct InMemoryDb {
    user: User
}

impl InMemoryDb {
    fn new(user: User -> InMemoryDb) {
        InMemoryDb { user = user }
    }
    // テストから中を覗くための関連関数。self を取らないので `::` で呼ぶ
    fn get(store: &InMemoryDb, id: int -> User?) {
        if store.user.id == id: store.user.clone() else: nil
    }
}

impl Database for InMemoryDb {
    fn find(&self, id: int -> User?) {
        if self.user.id == id: self.user.clone() else: nil
    }
    fn save(&mut self, u: User -> unit) {
        self.user = move u
    }
}

struct Frozen {
    t: int
}

impl Frozen {
    fn at(t: int -> Frozen) {
        Frozen { t = t }
    }
}

impl Clock for Frozen {
    fn now(self -> int) {
        self.t
    }
}

// ---- 差し替え: 呼ばれる側は一切変更しない ----

test "昇格すると Gold になり時刻が刻まれる" {
    let alice = User { id = 1, rank = Bronze, promoted_at = 0 }
    let mut store = InMemoryDb::new(move alice)

    with db(&mut store), clock(Frozen::at(1000)) {
        assert handle(1)
    }

    // `get` は `User?` を返す。見つからなければ Bronze の別人になるので、
    // 下の assert がそのまま「見つかったこと」も確かめる
    let u = InMemoryDb::get(store, 1) ?? User { id = 0, rank = Bronze, promoted_at = 0 }
    assert u.rank == Gold
    assert u.promoted_at == 1000
}
