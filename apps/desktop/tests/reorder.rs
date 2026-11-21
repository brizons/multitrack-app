//! L42: where a dragged row lands, from the row under the pointer and
//! which half of it.

use openvamp_desktop::reorder::{Half, drop_index, drop_slot};

#[test]
fn the_gap_is_before_the_row_in_its_upper_half_and_after_it_in_its_lower_half() {
    assert_eq!(drop_slot(0, Half::Upper), 0);
    assert_eq!(drop_slot(0, Half::Lower), 1);
    assert_eq!(drop_slot(3, Half::Upper), 3);
    assert_eq!(drop_slot(3, Half::Lower), 4);
}

#[test]
fn a_row_dropped_in_a_gap_ends_up_at_the_index_move_item_takes() {
    // Five rows; the row at 1 dragged down.
    assert_eq!(drop_index(1, 3, 5), Some(2), "between rows 2 and 3");
    assert_eq!(drop_index(1, 5, 5), Some(4), "after the last row");
    // The row at 3 dragged up.
    assert_eq!(drop_index(3, 0, 5), Some(0), "before the first row");
    assert_eq!(drop_index(3, 2, 5), Some(2), "between rows 1 and 2");
}

#[test]
fn a_drop_right_before_or_after_the_row_itself_changes_nothing() {
    assert_eq!(drop_index(2, 2, 5), None, "before itself");
    assert_eq!(drop_index(2, 3, 5), None, "after itself");
    // As the halves give them: over itself, or over a neighbour's near half.
    assert_eq!(drop_index(2, drop_slot(2, Half::Upper), 5), None);
    assert_eq!(drop_index(2, drop_slot(2, Half::Lower), 5), None);
    assert_eq!(drop_index(2, drop_slot(1, Half::Lower), 5), None);
    assert_eq!(drop_index(2, drop_slot(3, Half::Upper), 5), None);
}

#[test]
fn a_row_or_a_gap_outside_the_list_changes_nothing() {
    assert_eq!(drop_index(5, 0, 5), None);
    assert_eq!(drop_index(0, 6, 5), None);
    assert_eq!(drop_index(0, 1, 0), None);
}

#[test]
fn the_pointer_is_in_the_upper_half_above_the_rows_middle() {
    use openvamp_desktop::reorder::half_at;
    // A row from 100 to 140: its middle is 120.
    assert_eq!(half_at(100.0, 40.0, 100.0), Half::Upper);
    assert_eq!(half_at(100.0, 40.0, 119.9), Half::Upper);
    assert_eq!(half_at(100.0, 40.0, 120.0), Half::Lower);
    assert_eq!(half_at(100.0, 40.0, 139.0), Half::Lower);
}
