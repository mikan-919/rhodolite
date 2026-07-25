// Rhodolite v1 の到達目標。
//
// このファイルが動き、要求が可視化され、提供忘れが到達経路付きで
// 報告されたら v1 は完了。言語機能はこれを動かすのに必要な分だけ実装する。
//
// 見るべき点: promote と handle が1文字も書いていないのに、
// stamp の要求が main まで届いていること。

trait Database {
    fn find(id: UserId -> User?)
    fn save(u: User -> unit)
}

trait Clock {
    fn now(-> Time)
}

// ---- 使用: trait 名で修飾して呼ぶ ----

fn stamp(u: User) {
    u.promoted_at = Clock::now()
    Database::save(u)
}

// ---- 経由するだけ: 無記述 ----

fn promote(id: UserId -> bool) {
    let u = Database::find(id) ?? return false
    u.rank = Gold
    stamp(u)
    true
}

fn handle(id: UserId -> bool) {
    promote(id)
}

// ---- 提供: Head + ブロック ----

fn main() {
    let db = Postgres::new("postgres://localhost/app")
    Database(db), Clock(system_clock): {
        handle(current_user_id)
    }
}

// ---- 差し替え: 呼ばれる側は一切変更しない ----

test "昇格すると Gold になり時刻が刻まれる" {
    let db = InMemoryDb::new([alice])
    Database(db), Clock(Frozen::at(1000)): {
        assert handle(alice.id)

        let u = InMemoryDb::get(db, alice.id)
        assert u.rank == Gold
        assert u.promoted_at == 1000
    }
}
