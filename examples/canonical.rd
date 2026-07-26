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

// ---- 提供: Head + ブロック ----

fn main() {
    db(Postgres::new("postgres://localhost/app")), clock(system_clock): {
        handle(current_user_id)
    }
}

// ---- 差し替え: 呼ばれる側は一切変更しない ----

test "昇格すると Gold になり時刻が刻まれる" {
    let store = InMemoryDb::new([alice])

    db(store), clock(Frozen::at(1000)): {
        assert handle(alice.id)

        let u = InMemoryDb::get(store, alice.id)
        assert u.rank == Gold
        assert u.promoted_at == 1000
    }
}
