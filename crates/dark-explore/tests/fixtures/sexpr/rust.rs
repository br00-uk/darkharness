use std::fmt;

/// Adds one.
pub fn add_one(x: i32) -> i32 {
    helper(x) + 1
}

fn helper(x: i32) -> i32 {
    x
}

pub trait Greeter {
    fn greet(&self) -> String;
}
