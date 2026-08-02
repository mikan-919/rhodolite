struct Cell { value: int, note: str }

enum Selection {
    None
    Some(Cell)
}

fn main(-> int) {
    let mut selected = Selection::Some(Cell { value = 2, note = "old" })
    let mutation = match &mut selected {
        Selection::Some(cell) {
            cell.value = cell.value * 5
            cell.note = "changed"
            cell.value
        }
        Selection::None: 0
    }

    let mut cells = [
        Cell { value = 1, note = "a" },
        Cell { value = 2, note = "b" }
    ]
    let mut next_value = mutation + 1
    for cell in &mut cells {
        cell.value = next_value
        cell.note = "visited"
        next_value = next_value + 1
    }

    let mut final_total = 0
    for cell in cells {
        if cell.note == "visited" {
            final_total = final_total + cell.value
        }
    }
    final_total
}
