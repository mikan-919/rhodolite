struct User { name: str, score: int, alias: str? }
struct Node { value: int, indirect next: Node? }

enum Action {
    Skip
    Rename(str)
    Add(int)
}

fn node_total(node: Node -> int) {
    let value = node.value
    let next = move node.next ?? return value
    value + node_total(move next)
}

fn main(-> int) {
    let mut users = [
        User { name = "Ada", score = 1, alias = nil },
        User { name = "Lin", score = 2, alias = "L" }
    ]
    let before = users.clone()

    let mut next_score = 11
    for user in &mut users {
        user.score = next_score
        user.alias = "visited"
        next_score = next_score + 1
    }

    let mut shared_total = 0
    for user in users {
        if (user.alias ?? "missing") == "visited" {
            shared_total = shared_total + user.score
        }
    }

    let action = Action::Rename("Grace")
    let action_copy = action.clone()
    let action_score = match action {
        Action::Rename(name) if name == "Grace": 100
        Action::Skip: 0
        _: 1
    }
    let consumed_score = match move action_copy {
        Action::Rename(name): if name == "Grace": 10 else: 0
        Action::Add(amount): amount
        Action::Skip: 0
    }

    let maybe: User? = User { name = "Optional", score = 7, alias = nil }
    let optional_score = maybe.?score ?? 0
    let label: str? = "owned"
    let label_score = if (move label ?? "fallback") == "owned": 1 else: 0

    let tail = Node { value = 3, next = nil }
    let head = Node { value = 4, next = move tail }
    let recursive_score = node_total(move head)

    let mut consumed_total = 0
    for user in move users {
        consumed_total = consumed_total + user.score
    }

    let expected = [
        User { name = "Ada", score = 1, alias = nil },
        User { name = "Lin", score = 2, alias = "L" }
    ]
    let unchanged = if before == expected: 1 else: 0

    shared_total * 100000
        + consumed_total * 1000
        + action_score * 100
        + consumed_score * 10
        + optional_score
        + label_score
        + recursive_score
        + unchanged
}
