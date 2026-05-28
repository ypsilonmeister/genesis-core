use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

fn path_to_port(path: impl AsRef<Path>) -> u16 {
    let mut hasher = DefaultHasher::new();
    path.as_ref().to_string_lossy().hash(&mut hasher);
    let hash = hasher.finish();
    (49152 + (hash % 16384)) as u16
}

fn main() {
    let paths = vec![
        "/tmp/genesis-core/normalizer.sock",
        "/tmp/genesis-core/math_expander.sock",
        "/tmp/genesis-core/advanced_math.sock",
        "/tmp/genesis-core/tokenizer_v2.sock",
        "/tmp/genesis-core/parser.sock",
        "/tmp/genesis-core/evaluator.sock",
    ];
    for p in paths {
        println!("{} -> {}", p, path_to_port(p));
    }
}
