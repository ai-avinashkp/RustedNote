//! Pure index arithmetic for the tab strip. The actual open documents are
//! `Entity<EditorView>` values owned by the `Workspace`; these helpers keep the
//! active-index bookkeeping correct (and unit-tested) as tabs open and close.

/// Next tab index, wrapping around.
pub fn next_index(active: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (active + 1) % len
    }
}

/// Previous tab index, wrapping around.
pub fn prev_index(active: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (active + len - 1) % len
    }
}

/// New active index after removing `removed` from a list, given the length
/// *after* removal (`new_len`). The list is never allowed to become empty by
/// the caller, so `new_len >= 1` here.
pub fn active_after_remove(active: usize, removed: usize, new_len: usize) -> usize {
    debug_assert!(new_len >= 1);
    if removed < active {
        active - 1
    } else {
        active.min(new_len - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_wraps() {
        assert_eq!(next_index(2, 3), 0);
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(prev_index(0, 3), 2);
        assert_eq!(prev_index(2, 3), 1);
    }

    #[test]
    fn close_before_active_shifts_left() {
        // [a b (c)] active=2, remove index 0 -> [b c], active should be 1
        assert_eq!(active_after_remove(2, 0, 2), 1);
    }

    #[test]
    fn close_active_last_clamps() {
        // [a b (c)] active=2, remove index 2 -> [a b], active clamps to 1
        assert_eq!(active_after_remove(2, 2, 2), 1);
    }

    #[test]
    fn close_after_active_keeps_active() {
        // [(a) b c] active=0, remove index 2 -> [a b], active stays 0
        assert_eq!(active_after_remove(0, 2, 2), 0);
    }
}
