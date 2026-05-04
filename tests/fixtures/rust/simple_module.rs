pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

fn double(x: u32) -> u32 {
    add(x, x)
}

pub const ANSWER: u32 = 42;
