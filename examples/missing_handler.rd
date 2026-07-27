// 提供忘れ。原因 (stamp) と発覚地点 (main) が3階層離れている。
//
// main を読んでも clock という語はどこにも出てこない。
// エラーが到達経路を持たないと直せない、という実例。

trait Clock {
    fn now(-> Time)
}

effect clock: Clock

fn stamp(u: User) {
    u.at = clock.now()
}

fn promote(u: User) {
    stamp(u)
}

fn handle(u: User) {
    promote(u)
}

fn main() {
    handle(1)
}
