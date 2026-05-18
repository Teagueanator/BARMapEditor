//! Team-id assignment helpers for the F8 start-position editor.
//!
//! ## ADR-023 (legacy)
//!
//! BAR's `teams[]` mapinfo table indexes by integer team id; the
//! per-side convention is even ids on one side and odd ids on the
//! other. When the user placed a position under N-way symmetry, the
//! editor assigned ids interleaved across parities so mirror
//! counterparts ended up on opposite sides automatically.
//!
//! ## ADR-032 (B6)
//!
//! Position identity is no longer a flat `team_id` — it's
//! `(ally_group_id, index_within_group)`. The `assign_team_ids`
//! helper survives because the same parity logic is useful **within a
//! single ally group**: when the user places N mirror positions in
//! one group, the within-group parity-alternating layout still maps
//! cleanly onto BAR's lobby-side `script.txt` slot assignment.
//! Callers pass the group's existing within-group "logical ids"
//! (typically just `0..len()` cast to `u8`) as `used`.
//!
//! These functions are pure — they work over `&[u8]` of already-used
//! ids and return new ids. The F8 logic uses them when a single
//! click expands into N symmetry-replicated positions inside one
//! group.

/// Lowest team id ≥ 0 with the given parity that is not in `used`.
/// Returns 254 / 255 if the whole space is exhausted (BAR's practical
/// upper bound is much lower).
pub fn next_unused_id(used: &[u8], even: bool) -> u8 {
    let start: u32 = if even { 0 } else { 1 };
    let mut candidate = start;
    while candidate <= 255 {
        let id = candidate as u8;
        if !used.contains(&id) {
            return id;
        }
        candidate += 2;
    }
    if even { 254 } else { 255 }
}

/// Assign `n` fresh ids to a symmetry group. The original (centers[0])
/// takes the lowest unused even id; mirror counterparts alternate odd /
/// even from there. Returns ids in the same order as the caller's
/// `centers` Vec so each can be paired with its world position.
pub fn assign_team_ids(used: &[u8], n: usize) -> Vec<u8> {
    let mut used = used.to_vec();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let want_even = i % 2 == 0;
        let id = next_unused_id(&used, want_even);
        used.push(id);
        out.push(id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_unused_even_starts_at_zero() {
        assert_eq!(next_unused_id(&[], true), 0);
        assert_eq!(next_unused_id(&[0], true), 2);
        assert_eq!(next_unused_id(&[0, 2, 4], true), 6);
    }

    #[test]
    fn next_unused_odd_starts_at_one() {
        assert_eq!(next_unused_id(&[], false), 1);
        assert_eq!(next_unused_id(&[1, 3], false), 5);
    }

    #[test]
    fn assign_pair_for_horizontal_mirror_yields_zero_one() {
        let ids = assign_team_ids(&[], 2);
        assert_eq!(ids, vec![0u8, 1]);
    }

    #[test]
    fn assign_quad_alternates_parity() {
        // Original even, mirror odd, mirror even, mirror odd.
        let ids = assign_team_ids(&[], 4);
        assert_eq!(ids, vec![0u8, 1, 2, 3]);
    }

    #[test]
    fn assign_skips_already_used() {
        // Already have team 0; placing a mirror pair should yield (2, 1).
        let ids = assign_team_ids(&[0], 2);
        assert_eq!(ids, vec![2u8, 1]);
    }

    #[test]
    fn assign_for_three_player_rotational() {
        // fold=3 → 3 new positions. Even, odd, even.
        let ids = assign_team_ids(&[], 3);
        assert_eq!(ids, vec![0u8, 1, 2]);
    }

    #[test]
    fn assign_into_partially_filled_set() {
        let ids = assign_team_ids(&[0, 1, 2, 3], 2);
        assert_eq!(ids, vec![4u8, 5]);
    }
}
