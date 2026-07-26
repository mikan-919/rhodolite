// Rhodolite v1 の到達目標。
//
// このファイルが動き、要求が可視化され、提供忘れが到達経路付きで
// 報告されたら v1 は完了。言語機能はこれを動かすのに必要な分だけ実装する。
//
// 見るべき点: promote と handle が1文字も書いていないのに、
// stamp の要求が main まで届いていること。

// ---- 契約 ----

trait Database {
    fn find(self, id: UserId -> User?)
    fn save(self, u: User -> unit)
}

trait Clock {
    fn now(self -> Time)
}

// ---- データ ----

struct User {
    id: UserId
    rank: Rank
    promoted_at: Time
}

// v1 に enum は無い。フィールド0個の struct を名札として使う
struct Bronze {}
struct Gold {}

// ---- スロット宣言: 役割に名前を与える ----

effect db: Database
effect clock: Clock

// ---- 使用: スロット名で呼ぶ ----

fn stamp(u: User) {
    u.promoted_at = clock.now()
    db.save(u)
}

// ---- 経由するだけ: 無記述 ----

fn promote(id: UserId -> bool) {
    let u = db.find(id) ?? return false
    u.rank = Gold
    stamp(u)
    true
}

fn handle(id: UserId -> bool) {
    promote(id)
}

// ---- 本番のハンドラ。専用構文は無い、ただの impl ----

struct Postgres {
    url: Str
}

impl Postgres {
    fn new(url: Str -> Postgres) {
        Postgres { url = url }
    }
}

impl Database for Postgres {
    // v1 に本物の接続は無いので空の DB として振る舞う。
    // 差し替えが動くことの検証は下の test が InMemoryDb でやる
    fn find(self, id: UserId -> User?) {
        nil
    }
    fn save(self, u: User -> unit) {
        let ignored = u
    }
}

struct SystemClock {}

impl Clock for SystemClock {
    // v1 に本物の時計は無い
    fn now(self -> Time) {
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
    users: Users
}

impl InMemoryDb {
    fn new(users: Users -> InMemoryDb) {
        InMemoryDb { users = users }
    }
    // テストから中を覗くための関連関数。self を取らないので `::` で呼ぶ
    fn get(store: InMemoryDb, id: UserId -> User?) {
        store.find(id)
    }
}

impl Database for InMemoryDb {
    fn find(self, id: UserId -> User?) {
        for u in self.users {
            if u.id == id: return u
        }
        nil
    }
    fn save(self, u: User -> unit) {
        // 配列が同じ実体を持っているので、変更はもう見えている
        let ignored = u
    }
}

struct Frozen {
    t: Time
}

impl Frozen {
    fn at(t: Time -> Frozen) {
        Frozen { t = t }
    }
}

impl Clock for Frozen {
    fn now(self -> Time) {
        self.t
    }
}

// ---- 差し替え: 呼ばれる側は一切変更しない ----

test "昇格すると Gold になり時刻が刻まれる" {
    let alice = User { id = 1, rank = Bronze, promoted_at = 0 }
    let store = InMemoryDb::new([alice])

    with db(store), clock(Frozen::at(1000)) {
        assert handle(alice.id)

        let u = InMemoryDb::get(store, alice.id)
        assert u.rank == Gold
        assert u.promoted_at == 1000
    }
}
