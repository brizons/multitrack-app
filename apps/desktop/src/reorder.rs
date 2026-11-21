//! Reordering a list by dragging a row (L42): where a dragged row lands,
//! from the row the pointer is over and which half of it. The screens draw
//! an insertion line at that place and move the row there on drop.

/// Which half of a row the pointer is over while dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    /// The upper half: the dragged row would go before this one.
    Upper,
    /// The lower half: the dragged row would go after this one.
    Lower,
}

/// Which half of a row that starts at `top` and is `height` tall the
/// pointer at `y` is in (all in the same units, such as pixels).
pub fn half_at(top: f32, height: f32, y: f32) -> Half {
    if y < top + height / 2.0 {
        Half::Upper
    } else {
        Half::Lower
    }
}

/// The gap a dragged row would land in, with the pointer over row `over`:
/// 0 is before the first row, the row count after the last. The insertion
/// line is drawn there.
pub fn drop_slot(over: usize, half: Half) -> usize {
    match half {
        Half::Upper => over,
        Half::Lower => over + 1,
    }
}

/// The index the row at `from` ends up at when dropped in gap `slot` of a
/// list of `len` rows, as `move_item(from, to)` takes it; `None` when the
/// drop changes nothing (the gap right before or after the row itself) or
/// either is outside the list.
pub fn drop_index(from: usize, slot: usize, len: usize) -> Option<usize> {
    if from >= len || slot > len || slot == from || slot == from + 1 {
        return None;
    }
    // Taking the row out first shifts the gaps after it up by one.
    Some(if slot > from { slot - 1 } else { slot })
}
