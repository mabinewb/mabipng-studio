//! Pixel-aligned operations on indexed images, without changing palette indices.
use crate::brush::{Canvas, Shape};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Brush,
    Select,
    Fill,
    Line,
    Rectangle,
    Ellipse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    pub fn between(a: (i32, i32), b: (i32, i32), width: u32, height: u32) -> Self {
        let x = a.0.min(b.0).clamp(0, width as i32 - 1);
        let y = a.1.min(b.1).clamp(0, height as i32 - 1);
        Self {
            x,
            y,
            width: (a.0.max(b.0).clamp(x, width as i32 - 1) - x + 1) as u32,
            height: (a.1.max(b.1).clamp(y, height as i32 - 1) - y + 1) as u32,
        }
    }
    pub fn contains(self, p: (i32, i32)) -> bool {
        p.0 >= self.x
            && p.1 >= self.y
            && p.0 < self.x + self.width as i32
            && p.1 < self.y + self.height as i32
    }
}

#[derive(Clone)]
pub struct Fragment {
    pub region: Region,
    pub pixels: Vec<u8>,
    pub palette: Vec<[u8; 4]>,
}

pub fn copy(canvas: &Canvas, region: Region, palette: Vec<[u8; 4]>) -> Fragment {
    let mut pixels = Vec::new();
    for y in region.y..region.y + region.height as i32 {
        for x in region.x..region.x + region.width as i32 {
            pixels.push(canvas.pixels[y as usize * canvas.width as usize + x as usize]);
        }
    }
    Fragment {
        region,
        pixels,
        palette,
    }
}

pub fn paste(canvas: &mut Canvas, fragment: &Fragment, at: (i32, i32), palette: &[[u8; 4]]) {
    let mapping: Vec<u8> = fragment
        .palette
        .iter()
        .enumerate()
        .map(|(i, color)| {
            if fragment.palette == palette {
                return i as u8;
            }
            palette
                .iter()
                .enumerate()
                .min_by_key(|(_, target)| {
                    color
                        .iter()
                        .zip(target.iter())
                        .map(|(&a, &b)| (i32::from(a) - i32::from(b)).pow(2))
                        .sum::<i32>()
                })
                .map_or(0, |(index, _)| index as u8)
        })
        .collect();
    for y in 0..fragment.region.height {
        for x in 0..fragment.region.width {
            let px = at.0 + x as i32;
            let py = at.1 + y as i32;
            if px >= 0 && py >= 0 && px < canvas.width as i32 && py < canvas.height as i32 {
                canvas.pixels[py as usize * canvas.width as usize + px as usize] =
                    mapping[fragment.pixels[(y * fragment.region.width + x) as usize] as usize];
            }
        }
    }
}

pub fn clear(canvas: &mut Canvas, region: Region, color: u8) {
    for y in region.y.max(0)..(region.y + region.height as i32).min(canvas.height as i32) {
        for x in region.x.max(0)..(region.x + region.width as i32).min(canvas.width as i32) {
            canvas.pixels[y as usize * canvas.width as usize + x as usize] = color;
        }
    }
}

pub fn fill(canvas: &mut Canvas, point: (i32, i32), color: u8, global: bool) {
    let (x, y) = point;
    if x < 0 || y < 0 || x >= canvas.width as i32 || y >= canvas.height as i32 {
        return;
    }
    let start = y as usize * canvas.width as usize + x as usize;
    let old = canvas.pixels[start];
    if old == color {
        return;
    }
    if global {
        for pixel in &mut canvas.pixels {
            if *pixel == old {
                *pixel = color;
            }
        }
        return;
    }
    let width = canvas.width as usize;
    let mut stack = vec![start];
    canvas.pixels[start] = color;
    while let Some(i) = stack.pop() {
        for next in [
            i.checked_sub(width),
            (i + width < canvas.pixels.len()).then_some(i + width),
            (i % width > 0).then(|| i - 1),
            (i % width + 1 < width).then_some(i + 1),
        ]
        .into_iter()
        .flatten()
        {
            if canvas.pixels[next] == old {
                canvas.pixels[next] = color;
                stack.push(next);
            }
        }
    }
}

pub fn figure(
    canvas: &mut Canvas,
    tool: Tool,
    a: (i32, i32),
    b: (i32, i32),
    color: u8,
    filled: bool,
    thickness: u32,
) {
    if tool == Tool::Line {
        canvas.begin_stroke();
        canvas.stroke_to(a.0, a.1, thickness, Shape::Square, color);
        canvas.stroke_to(b.0, b.1, thickness, Shape::Square, color);
        canvas.end_stroke();
        return;
    }
    let r = Region::between(a, b, canvas.width, canvas.height);
    let t = thickness.min(r.width.div_ceil(2)).min(r.height.div_ceil(2));
    let inside = |x: f64, y: f64, rx: f64, ry: f64| {
        rx > 0.0 && ry > 0.0 && (x / rx).powi(2) + (y / ry).powi(2) <= 1.0
    };
    for y in 0..r.height {
        for x in 0..r.width {
            let draw = if tool == Tool::Rectangle {
                filled || x < t || y < t || x >= r.width - t || y >= r.height - t
            } else {
                let rx = r.width as f64 / 2.0;
                let ry = r.height as f64 / 2.0;
                let dx = x as f64 + 0.5 - rx;
                let dy = y as f64 + 0.5 - ry;
                inside(dx, dy, rx, ry) && (filled || !inside(dx, dy, rx - t as f64, ry - t as f64))
            };
            if draw {
                canvas.pixels[(r.y as u32 + y) as usize * canvas.width as usize
                    + (r.x as u32 + x) as usize] = color;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fill_is_connected_or_global() {
        let mut c = Canvas::new(3, 2, vec![0, 1, 0, 0, 1, 0]).unwrap();
        fill(&mut c, (0, 0), 2, false);
        assert_eq!(c.pixels, [2, 1, 0, 2, 1, 0]);
        fill(&mut c, (2, 0), 3, true);
        assert_eq!(c.pixels, [2, 1, 3, 2, 1, 3]);
    }
    #[test]
    fn selection_overlap_and_clipping_preserve_indices() {
        let mut c = Canvas::new(4, 1, vec![0, 1, 2, 3]).unwrap();
        let colors = vec![[0; 4], [1; 4], [2; 4], [3; 4]];
        let f = copy(
            &c,
            Region {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
            },
            colors.clone(),
        );
        clear(&mut c, f.region, 0);
        paste(&mut c, &f, (2, 0), &colors);
        assert_eq!(c.pixels, [0, 0, 1, 2]);
        paste(&mut c, &f, (-1, 0), &colors);
        assert_eq!(c.pixels[0], 2);
    }
    #[test]
    fn figures_have_pixel_aligned_edges() {
        let mut c = Canvas::new(5, 5, vec![0; 25]).unwrap();
        figure(&mut c, Tool::Rectangle, (0, 0), (4, 4), 1, false, 1);
        assert_eq!(c.pixels.iter().filter(|&&p| p == 1).count(), 16);
        assert_eq!(c.pixels[12], 0);
        figure(&mut c, Tool::Ellipse, (0, 0), (4, 4), 2, true, 1);
        assert_eq!(c.pixels[12], 2);
        assert_eq!(c.pixels[0], 1);
    }
}
