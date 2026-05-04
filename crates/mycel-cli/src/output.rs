use mycel_core::*;
use serde::Serialize;

#[derive(Serialize)]
pub struct FindHit<'a> {
    pub symbol: &'a Symbol,
    pub score: f32,
}

pub fn print_symbols(syms: &[Symbol], json: bool) {
    if json {
        println!("{}", serde_json::to_string(syms).unwrap());
    } else {
        for s in syms {
            println!("{}  {:?}  {}:{}-{}",
                s.qualified_name.as_str(), s.kind,
                s.file_path, s.start_line, s.end_line);
            println!("    {}", s.signature.as_str());
        }
    }
}

pub fn print_find(hits: &[(Symbol, f32)], json: bool) {
    if json {
        let v: Vec<_> = hits.iter().map(|(s, sc)| FindHit { symbol: s, score: *sc }).collect();
        println!("{}", serde_json::to_string(&v).unwrap());
    } else {
        for (s, score) in hits {
            println!("{:.3}  {}", score, s.qualified_name.as_str());
            println!("    {}:{}-{}  {}", s.file_path, s.start_line, s.end_line, s.signature.as_str());
        }
    }
}
