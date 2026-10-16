//! Standalone indexed-pixel brush engine. Each pixel is one palette index.
use std::collections::VecDeque;

const MAX_BRUSH_SIZE: u32 = 64;
const MAX_HISTORY_BYTES: usize = 32 * 1024 * 1024;
const MAX_HISTORY_STROKES: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Circle,
    Square,
    Diamond,
}

struct Stroke {
    before: Vec<u8>,
    last: Option<(i32, i32)>,
}

/// Keep dimensions and pixel-buffer length unchanged after construction.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    undo: VecDeque<Vec<u8>>,
    redo: Vec<Vec<u8>>,
    stroke: Option<Stroke>,
}

impl Canvas {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> anyhow::Result<Self> {
        anyhow::ensure!(width > 0 && height > 0, "canvas dimensions must be nonzero");
        let expected = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .ok_or_else(|| anyhow::anyhow!("canvas dimensions overflow"))?;
        anyhow::ensure!(
            pixels.len() == expected,
            "pixel count does not match canvas dimensions"
        );
        Ok(Self {
            width,
            height,
            pixels,
            undo: VecDeque::new(),
            redo: Vec::new(),
            stroke: None,
        })
    }

    /// Starting again commits any pending stroke and resets its interpolation anchor.
    pub fn begin_stroke(&mut self) {
        self.end_stroke();
        self.stroke = Some(Stroke {
            before: self.pixels.clone(),
            last: None,
        });
    }

    /// Paint through this point. Size is clamped to 1..=64.
    /// If needed, implicitly begin a stroke; call `end_stroke` on pointer release.
    pub fn stroke_to(&mut self, x: i32, y: i32, size: u32, shape: Shape, color: u8) {
        if self.stroke.is_none() {
            self.begin_stroke();
        }
        let size = size.clamp(1, MAX_BRUSH_SIZE);
        let stroke = self.stroke.as_mut().expect("stroke was initialized");
        let previous = stroke.last.replace((x, y));
        if let Some(start) = previous {
            self.line(start, (x, y), size, shape, color);
        } else {
            self.stamp(i64::from(x), i64::from(y), size, shape, color);
        }
    }

    /// Return whether the completed stroke actually changed the image.
    pub fn end_stroke(&mut self) -> bool {
        let Some(stroke) = self.stroke.take() else {
            return false;
        };
        if stroke.before == self.pixels {
            return false;
        }
        self.redo.clear();
        // A single oversized snapshot cannot fit the retained-history budget.
        if stroke.before.len() <= MAX_HISTORY_BYTES {
            self.undo.push_back(stroke.before);
        }
        self.trim_history();
        true
    }

    pub fn undo(&mut self) -> bool {
        self.end_stroke();
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.pixels, previous));
        true
    }

    pub fn redo(&mut self) -> bool {
        self.end_stroke();
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo
            .push_back(std::mem::replace(&mut self.pixels, next));
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn trim_history(&mut self) {
        let mut bytes: usize = self.undo.iter().chain(self.redo.iter()).map(Vec::len).sum();
        while self.undo.len() + self.redo.len() > MAX_HISTORY_STROKES || bytes > MAX_HISTORY_BYTES {
            let removed = self.undo.pop_front().or_else(|| {
                if self.redo.is_empty() {
                    None
                } else {
                    Some(self.redo.remove(0))
                }
            });
            let Some(removed) = removed else { break };
            bytes -= removed.len();
        }
    }

    fn line(&mut self, start: (i32, i32), end: (i32, i32), size: u32, shape: Shape, color: u8) {
        let low = -i64::from(size / 2);
        let high = low + i64::from(size) - 1;
        // Clip brush centers to the canvas expanded by the asymmetric stamp extents.
        let Some(((mut x, mut y), (end_x, end_y))) = clip_segment(
            start,
            end,
            [
                -high,
                -high,
                i64::from(self.width) - 1 - low,
                i64::from(self.height) - 1 - low,
            ],
        ) else {
            return;
        };
        let dx = (end_x - x).abs();
        let sx = if x < end_x { 1 } else { -1 };
        let dy = -(end_y - y).abs();
        let sy = if y < end_y { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            self.stamp(x, y, size, shape, color);
            if x == end_x && y == end_y {
                break;
            }
            let twice_error = 2 * error;
            if twice_error >= dy {
                error += dy;
                x += sx;
            }
            if twice_error <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    fn stamp(&mut self, x: i64, y: i64, size: u32, shape: Shape, color: u8) {
        let size = i64::from(size);
        let left = x - size / 2;
        let top = y - size / 2;
        let x_start = left.max(0);
        let y_start = top.max(0);
        let x_end = (left + size).min(i64::from(self.width));
        let y_end = (top + size).min(i64::from(self.height));
        for py in y_start..y_end {
            for px in x_start..x_end {
                // Doubled pixel-center coordinates keep both odd/even masks symmetric.
                // Even sizes center half a pixel above/left of the pointer anchor.
                let dx = 2 * (px - left) + 1 - size;
                let dy = 2 * (py - top) + 1 - size;
                let inside = match shape {
                    Shape::Square => true,
                    Shape::Circle => dx * dx + dy * dy <= size * size,
                    Shape::Diamond => dx.abs() + dy.abs() <= size,
                };
                if inside {
                    self.pixels[py as usize * self.width as usize + px as usize] = color;
                }
            }
        }
    }
}

/// Slab clipping bounds work by canvas dimensions, never by pointer distance.
/// f64 exactly represents the input i32 coordinates and their differences.
fn clip_segment(
    start: (i32, i32),
    end: (i32, i32),
    bounds: [i64; 4],
) -> Option<((i64, i64), (i64, i64))> {
    let origin = [f64::from(start.0), f64::from(start.1)];
    let delta = [f64::from(end.0) - origin[0], f64::from(end.1) - origin[1]];
    let mut enter: f64 = 0.0;
    let mut exit: f64 = 1.0;
    for axis in 0..2 {
        let min = bounds[axis] as f64;
        let max = bounds[axis + 2] as f64;
        if delta[axis] == 0.0 {
            if origin[axis] < min || origin[axis] > max {
                return None;
            }
        } else {
            let a = (min - origin[axis]) / delta[axis];
            let b = (max - origin[axis]) / delta[axis];
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if enter > exit {
                return None;
            }
        }
    }
    let point = |t: f64| {
        (
            ((origin[0] + t * delta[0]).round() as i64).clamp(bounds[0], bounds[2]),
            ((origin[1] + t * delta[1]).round() as i64).clamp(bounds[1], bounds[3]),
        )
    };
    Some((point(enter), point(exit)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(width: u32, height: u32) -> Canvas {
        Canvas::new(width, height, vec![0; width as usize * height as usize]).unwrap()
    }

    fn paint(canvas: &mut Canvas, x: i32, y: i32, size: u32, shape: Shape, color: u8) -> bool {
        canvas.begin_stroke();
        canvas.stroke_to(x, y, size, shape, color);
        canvas.end_stroke()
    }

    #[test]
    fn validates_dimensions_and_pixels() {
        assert!(Canvas::new(0, 1, vec![]).is_err());
        assert!(Canvas::new(1, 0, vec![]).is_err());
        assert!(Canvas::new(2, 2, vec![0; 3]).is_err());
        assert!(Canvas::new(u32::MAX, u32::MAX, vec![]).is_err());
        assert_eq!(Canvas::new(1, 1, vec![7]).unwrap().pixels, vec![7]);
    }

    #[test]
    fn all_shapes_paint_one_pixel_and_zero_size_is_one() {
        for shape in [Shape::Circle, Shape::Square, Shape::Diamond] {
            for size in [0, 1] {
                let mut c = canvas(3, 3);
                assert!(paint(&mut c, 1, 1, size, shape, 9));
                assert_eq!(c.pixels, vec![0, 0, 0, 0, 9, 0, 0, 0, 0]);
            }
        }
    }

    #[test]
    fn footprints_are_symmetric_for_even_and_odd_sizes() {
        for (shape, size, count) in [
            (Shape::Square, 5, 25),
            (Shape::Circle, 5, 21),
            (Shape::Diamond, 5, 13),
            (Shape::Square, 4, 16),
            (Shape::Circle, 4, 12),
            (Shape::Diamond, 4, 12),
            (Shape::Circle, 2, 4),
        ] {
            let mut c = canvas(size, size);
            paint(&mut c, (size / 2) as i32, (size / 2) as i32, size, shape, 1);
            assert_eq!(c.pixels.iter().filter(|&&p| p == 1).count(), count);
            for y in 0..size as usize {
                for x in 0..size as usize {
                    let n = size as usize;
                    assert_eq!(c.pixels[y * n + x], c.pixels[y * n + n - 1 - x]);
                    assert_eq!(c.pixels[y * n + x], c.pixels[(n - 1 - y) * n + x]);
                }
            }
        }
    }

    #[test]
    fn clips_stamps_at_every_edge() {
        let mut c = canvas(5, 5);
        for (x, y) in [(0, 0), (4, 0), (0, 4), (4, 4)] {
            paint(&mut c, x, y, 3, Shape::Square, 1);
        }
        assert_eq!(c.pixels.iter().filter(|&&p| p == 1).count(), 16);
        assert_eq!(c.pixels[12], 0);
        assert!(!paint(&mut c, -100, -100, 64, Shape::Circle, 2));
    }

    #[test]
    fn interpolates_fast_moves_in_all_octants() {
        for (x, y) in [
            (15, 10),
            (10, 15),
            (2, 10),
            (10, 2),
            (15, 15),
            (2, 2),
            (2, 15),
            (15, 2),
        ] {
            let mut c = canvas(19, 19);
            c.begin_stroke();
            c.stroke_to(8, 8, 1, Shape::Circle, 1);
            c.stroke_to(x, y, 1, Shape::Circle, 1);
            assert!(c.end_stroke());
            let expected = (x - 8).abs().max((y - 8).abs()) as usize + 1;
            assert_eq!(c.pixels.iter().filter(|&&p| p == 1).count(), expected);
            assert_eq!(c.pixels[y as usize * 19 + x as usize], 1);
            let mut at = (8_i32, 8_i32);
            let mut visited = vec![at];
            while at != (x, y) {
                let next = (-1..=1)
                    .flat_map(|dy| (-1..=1).map(move |dx| (at.0 + dx, at.1 + dy)))
                    .find(|&(nx, ny)| {
                        nx >= 0
                            && ny >= 0
                            && nx < 19
                            && ny < 19
                            && c.pixels[ny as usize * 19 + nx as usize] == 1
                            && !visited.contains(&(nx, ny))
                    })
                    .expect("line has no gap");
                visited.push(next);
                at = next;
            }
        }
    }

    #[test]
    fn extreme_pointer_coordinates_are_clipped_before_rasterizing() {
        let mut c = canvas(9, 9);
        c.stroke_to(i32::MIN, 4, 1, Shape::Square, 3);
        c.stroke_to(i32::MAX, 4, 1, Shape::Square, 3);
        assert!(c.end_stroke());
        assert_eq!(&c.pixels[36..45], &[3; 9]);
        c.stroke_to(i32::MIN, i32::MIN, 1, Shape::Diamond, 5);
        c.stroke_to(i32::MAX, i32::MAX, 1, Shape::Diamond, 5);
        c.end_stroke();
        for i in 0..9 {
            assert_eq!(c.pixels[i * 9 + i], 5);
        }
        c.stroke_to(i32::MIN, i32::MAX, u32::MAX, Shape::Square, 7);
        c.stroke_to(i32::MAX, i32::MAX, u32::MAX, Shape::Square, 7);
        assert!(!c.end_stroke());
    }

    #[test]
    fn brush_overhang_outside_canvas_can_still_paint() {
        let mut c = canvas(5, 5);
        c.stroke_to(-1, i32::MIN, 3, Shape::Square, 2);
        c.stroke_to(-1, i32::MAX, 3, Shape::Square, 2);
        assert!(c.end_stroke());
        assert_eq!(c.pixels.iter().filter(|&&p| p == 2).count(), 5);
    }

    #[test]
    fn one_history_entry_per_stroke_and_no_connection_between_strokes() {
        let mut c = canvas(9, 1);
        c.stroke_to(0, 0, 1, Shape::Square, 1);
        c.stroke_to(2, 0, 1, Shape::Square, 1);
        assert!(c.end_stroke());
        paint(&mut c, 8, 0, 1, Shape::Square, 2);
        assert_eq!(c.pixels, vec![1, 1, 1, 0, 0, 0, 0, 0, 2]);
        assert!(c.can_undo());
        assert!(c.undo());
        assert!(c.undo());
        assert!(!c.undo());
        assert!(!c.can_undo());
        assert!(c.can_redo());
        assert!(c.redo());
        assert!(c.redo());
        assert!(!c.redo());
    }

    #[test]
    fn noop_preserves_redo_but_changed_stroke_clears_it() {
        let mut c = canvas(3, 1);
        paint(&mut c, 0, 0, 1, Shape::Square, 1);
        c.undo();
        assert!(!paint(&mut c, 1, 0, 1, Shape::Square, 0));
        assert!(c.can_redo());
        c.begin_stroke();
        c.stroke_to(1, 0, 1, Shape::Square, 1);
        c.stroke_to(1, 0, 1, Shape::Square, 0);
        assert!(!c.end_stroke());
        assert!(c.can_redo());
        assert!(!c.can_undo());
        assert!(paint(&mut c, 2, 0, 1, Shape::Square, 2));
        assert!(!c.can_redo());
        assert!(!c.end_stroke());
    }

    #[test]
    fn repeated_begin_and_undo_commit_pending_strokes() {
        let mut c = canvas(3, 1);
        c.stroke_to(0, 0, 1, Shape::Square, 1);
        c.begin_stroke();
        c.stroke_to(2, 0, 1, Shape::Square, 2);
        assert!(c.undo());
        assert_eq!(c.pixels, vec![1, 0, 0]);
        assert!(c.undo());
        assert_eq!(c.pixels, vec![0, 0, 0]);
    }

    #[test]
    fn limits_size_and_history_count() {
        let mut c = canvas(70, 70);
        paint(&mut c, 35, 35, u32::MAX, Shape::Square, 1);
        assert_eq!(c.pixels.iter().filter(|&&p| p == 1).count(), 64 * 64);
        let mut c = canvas(1, 1);
        for color in 1..=60 {
            paint(&mut c, 0, 0, 1, Shape::Circle, color);
        }
        assert_eq!(c.undo.len(), MAX_HISTORY_STROKES);
        for _ in 0..50 {
            assert!(c.undo());
        }
        assert_eq!(c.pixels, vec![10]);
        assert!(!c.can_undo());
        assert_eq!(c.redo.len(), MAX_HISTORY_STROKES);
    }

    #[test]
    fn history_bytes_stay_within_budget() {
        let mut c = canvas(1024, 1024);
        for color in 1..=40 {
            paint(&mut c, 0, 0, 1, Shape::Square, color);
        }
        assert_eq!(c.undo.len(), 32);
        for _ in 0..16 {
            c.undo();
        }
        let bytes: usize = c.undo.iter().chain(c.redo.iter()).map(Vec::len).sum();
        assert_eq!(bytes, MAX_HISTORY_BYTES);
        paint(&mut c, 0, 0, 1, Shape::Square, 100);
        assert!(c.redo.is_empty());
        assert!(c.undo.iter().map(Vec::len).sum::<usize>() <= MAX_HISTORY_BYTES);
    }
}
