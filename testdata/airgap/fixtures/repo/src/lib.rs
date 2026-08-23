//! A minimal crate. `cargo xtask airgap`'s "edit a file and run a test"
//! step (task unit `J5` step 3) works against a copy of this file, not
//! against the darkharness workspace's own source.

/// Adds two numbers.
pub fn add(left: i32, right: i32) -> i32 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_two_numbers() {
        assert_eq!(add(2, 2), 4);
    }
}
