use std::{env, fs};

fn main() {
    let path = env::args_os().nth(1).expect("usage: verify-xp FINAL_DATA");
    let data = fs::read(path).expect("read final guest data");
    assert_eq!(data.len(), 2 * 1024 * 1024);
    for (chunk, bytes) in data.chunks_exact(1024).enumerate() {
        for (index, &actual) in bytes.iter().enumerate() {
            let expected = (index.wrapping_mul(37) ^ chunk ^ (chunk >> 8)) as u8 ^ 3;
            assert_eq!(actual, expected, "chunk {chunk}, byte {index}");
        }
    }
    println!("PASS XP final storage: 2097152 bytes, iteration 3");
}
