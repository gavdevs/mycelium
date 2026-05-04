pub trait Greeter {
    fn greet(&self, name: &str) -> String;
}

pub struct FormalGreeter {
    prefix: String,
}

impl Greeter for FormalGreeter {
    fn greet(&self, name: &str) -> String {
        format!("{}{}", self.prefix, name)
    }
}
