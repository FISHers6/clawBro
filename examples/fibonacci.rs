fn fibonacci(n: usize) -> Vec<u64> {
    let mut seq = Vec::with_capacity(n);

    for i in 0..n {
        match i {
            0 => seq.push(0),
            1 => seq.push(1),
            _ => {
                let next = seq[i - 1] + seq[i - 2];
                seq.push(next);
            }
        }
    }

    seq
}

fn main() {
    let n = 10;
    let seq = fibonacci(n);
    println!("前 {} 项斐波那契数列: {:?}", n, seq);
}
