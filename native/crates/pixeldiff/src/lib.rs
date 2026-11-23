//! Screenshot comparison: a port of pixelmatch (ISC, mapbox/pixelmatch v5) that skips anti-aliased pixels.

use image::RgbaImage;

pub struct DiffResult {
    /// Pixels that differ and are not anti-aliasing.
    pub different: u64,
    pub anti_aliased: u64,
    /// Red where pixels differ, yellow where only anti-aliasing differs, faded original elsewhere.
    pub diff_image: RgbaImage,
}

/// `threshold` is pixelmatch's 0..1 color tolerance (0.1 by default there).
pub fn diff(a: &RgbaImage, b: &RgbaImage, threshold: f64) -> anyhow::Result<DiffResult> {
    anyhow::ensure!(a.dimensions() == b.dimensions(), "image sizes differ: {:?} vs {:?}", a.dimensions(), b.dimensions());
    let (width, height) = a.dimensions();
    let max_delta = 35215.0 * threshold * threshold;
    let mut diff_image = RgbaImage::new(width, height);
    let mut different = 0;
    let mut anti_aliased = 0;
    for y in 0..height {
        for x in 0..width {
            let delta = color_delta(a.get_pixel(x, y).0, b.get_pixel(x, y).0, false);
            if delta.abs() > max_delta {
                if is_anti_aliased(a, x, y, b) || is_anti_aliased(b, x, y, a) {
                    anti_aliased += 1;
                    diff_image.put_pixel(x, y, image::Rgba([255, 255, 0, 255]));
                } else {
                    different += 1;
                    diff_image.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
                }
            } else {
                let gray = blend(gray_of(a.get_pixel(x, y).0), 0.1) as u8;
                diff_image.put_pixel(x, y, image::Rgba([gray, gray, gray, 255]));
            }
        }
    }
    Ok(DiffResult { different, anti_aliased, diff_image })
}

/// A pixel is anti-aliased when its 3x3 neighborhood holds both a darkest and a brightest sibling
/// that each sit in a flat region (more than 2 identical neighbors) in both images.
fn is_anti_aliased(image: &RgbaImage, x: u32, y: u32, other: &RgbaImage) -> bool {
    let (width, height) = image.dimensions();
    let (x0, y0) = (x.saturating_sub(1), y.saturating_sub(1));
    let (x2, y2) = ((x + 1).min(width - 1), (y + 1).min(height - 1));
    let mut zeroes = if x == x0 || x == x2 || y == y0 || y == y2 { 1 } else { 0 };
    let (mut min, mut max) = (0.0, 0.0);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (0, 0, 0, 0);
    let center = image.get_pixel(x, y).0;
    for adjacent_x in x0..=x2 {
        for adjacent_y in y0..=y2 {
            if adjacent_x == x && adjacent_y == y {
                continue;
            }
            let delta = color_delta(center, image.get_pixel(adjacent_x, adjacent_y).0, true);
            if delta == 0.0 {
                zeroes += 1;
                if zeroes > 2 {
                    return false;
                }
            } else if delta < min {
                min = delta;
                (min_x, min_y) = (adjacent_x, adjacent_y);
            } else if delta > max {
                max = delta;
                (max_x, max_y) = (adjacent_x, adjacent_y);
            }
        }
    }
    if min == 0.0 || max == 0.0 {
        return false;
    }
    (has_many_siblings(image, min_x, min_y) && has_many_siblings(other, min_x, min_y))
        || (has_many_siblings(image, max_x, max_y) && has_many_siblings(other, max_x, max_y))
}

fn has_many_siblings(image: &RgbaImage, x: u32, y: u32) -> bool {
    let (width, height) = image.dimensions();
    let (x0, y0) = (x.saturating_sub(1), y.saturating_sub(1));
    let (x2, y2) = ((x + 1).min(width - 1), (y + 1).min(height - 1));
    let mut zeroes = if x == x0 || x == x2 || y == y0 || y == y2 { 1 } else { 0 };
    let center = image.get_pixel(x, y).0;
    for adjacent_x in x0..=x2 {
        for adjacent_y in y0..=y2 {
            if adjacent_x == x && adjacent_y == y {
                continue;
            }
            if image.get_pixel(adjacent_x, adjacent_y).0 == center {
                zeroes += 1;
                if zeroes > 2 {
                    return true;
                }
            }
        }
    }
    false
}

/// Squared YIQ distance (Kotsarenko and Ramos), signed by brightness; `y_only` returns the brightness difference.
fn color_delta(a: [u8; 4], b: [u8; 4], y_only: bool) -> f64 {
    let [r1, g1, b1] = blend_white(a);
    let [r2, g2, b2] = blend_white(b);
    let y = rgb2y(r1, g1, b1) - rgb2y(r2, g2, b2);
    if y_only {
        return y;
    }
    let i = rgb2i(r1, g1, b1) - rgb2i(r2, g2, b2);
    let q = rgb2q(r1, g1, b1) - rgb2q(r2, g2, b2);
    let delta = 0.5053 * y * y + 0.299 * i * i + 0.1957 * q * q;
    if y > 0.0 { -delta } else { delta }
}

fn blend_white([r, g, b, a]: [u8; 4]) -> [f64; 3] {
    let alpha = a as f64 / 255.0;
    [blend(r as f64, alpha), blend(g as f64, alpha), blend(b as f64, alpha)]
}

fn blend(channel: f64, alpha: f64) -> f64 {
    255.0 + (channel - 255.0) * alpha
}

fn gray_of(pixel: [u8; 4]) -> f64 {
    let [r, g, b] = blend_white(pixel);
    rgb2y(r, g, b)
}

fn rgb2y(r: f64, g: f64, b: f64) -> f64 {
    r * 0.29889531 + g * 0.58662247 + b * 0.11448223
}
fn rgb2i(r: f64, g: f64, b: f64) -> f64 {
    r * 0.59597799 - g * 0.2741761 - b * 0.32180189
}
fn rgb2q(r: f64, g: f64, b: f64) -> f64 {
    r * 0.21147017 - g * 0.52261711 + b * 0.31114694
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, image::Rgba(color))
    }

    #[test]
    fn identical_images_have_no_difference() {
        let image = flat(8, 8, [30, 30, 30, 255]);
        let result = diff(&image, &image, 0.1).unwrap();
        assert_eq!((result.different, result.anti_aliased), (0, 0));
    }

    #[test]
    fn a_solid_block_change_counts_as_different() {
        let a = flat(10, 10, [0, 0, 0, 255]);
        let mut b = a.clone();
        for x in 3..6 {
            for y in 3..6 {
                b.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let result = diff(&a, &b, 0.1).unwrap();
        assert_eq!(result.different, 9);
    }

    #[test]
    fn an_edge_pixel_between_flat_regions_is_anti_aliasing() {
        // Left half black, right half white; the column between them is gray in one image and black in the other.
        let mut a = flat(9, 9, [255, 255, 255, 255]);
        for x in 0..4 {
            for y in 0..9 {
                a.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let mut b = a.clone();
        for y in 0..9 {
            a.put_pixel(4, y, image::Rgba([128, 128, 128, 255]));
            b.put_pixel(4, y, image::Rgba([0, 0, 0, 255]));
        }
        let result = diff(&a, &b, 0.1).unwrap();
        assert_eq!(result.different, 0);
        assert_eq!(result.anti_aliased, 9);
    }
}
