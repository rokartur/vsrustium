//! pixeldiff <expected.png> <actual.png> [diff.png]: prints the counts, exits 1 when non-anti-aliased pixels differ.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(args.len() == 2 || args.len() == 3, "usage: pixeldiff <expected.png> <actual.png> [diff.png]");
    let expected = image::open(&args[0])?.to_rgba8();
    let actual = image::open(&args[1])?.to_rgba8();
    let result = pixeldiff::diff(&expected, &actual, 0.1)?;
    if let Some(path) = args.get(2) {
        result.diff_image.save(path)?;
    }
    let (width, height) = expected.dimensions();
    println!("different={} anti_aliased={} total={}", result.different, result.anti_aliased, width as u64 * height as u64);
    std::process::exit(if result.different == 0 { 0 } else { 1 });
}
