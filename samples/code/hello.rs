//! Sample Rust file — syntax highlighting demo.
use std::collections::HashMap;

const MAX: u32 = 100; // a constant

#[derive(Debug, Clone)]
struct Point {
    x: f64,
    y: f64,
}

impl Point {
    /// Euclidean distance to another point.
    fn dist(&self, other: &Point) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

fn main() {
    let mut counts: HashMap<String, i32> = HashMap::new();
    counts.insert("answer".to_string(), 42);
    for (key, value) in &counts {
        println!("{key} = {value}"); // string interpolation
    }
    let origin = Point { x: 0.0, y: 0.0 };
    let p = Point { x: 3.0, y: 4.0 };
    assert_eq!(p.dist(&origin), 5.0);
    if MAX > 10 {
        eprintln!("max is large");
    }
}
