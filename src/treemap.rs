//! Squarified treemap layout (Bruls, Huizing, van Wijk 2000).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn inset(&self, top: f64, side: f64) -> Rect {
        Rect {
            x: self.x + side,
            y: self.y + top,
            w: (self.w - 2.0 * side).max(0.0),
            h: (self.h - top - side).max(0.0),
        }
    }
}

/// Lay out `values` (sorted largest first, all > 0) inside `rect`.
/// Returns one rect per value, in the same order.
pub fn squarify(values: &[f64], rect: Rect) -> Vec<Rect> {
    let total: f64 = values.iter().sum();
    let mut out = Vec::with_capacity(values.len());
    if total <= 0.0 || rect.w <= 0.0 || rect.h <= 0.0 {
        out.resize(values.len(), Rect { w: 0.0, h: 0.0, ..rect });
        return out;
    }
    let scale = rect.w * rect.h / total;
    let areas: Vec<f64> = values.iter().map(|v| v * scale).collect();

    let mut free = rect;
    let mut start = 0;
    while start < areas.len() {
        let side = free.w.min(free.h);
        // Grow the row while it improves the worst aspect ratio.
        let mut end = start + 1;
        while end < areas.len() && worst(&areas[start..=end], side) <= worst(&areas[start..end], side) {
            end += 1;
        }
        let row = &areas[start..end];
        let row_sum: f64 = row.iter().sum();
        if free.w >= free.h {
            // Column on the left.
            let cw = if free.h > 0.0 { row_sum / free.h } else { 0.0 };
            let mut y = free.y;
            for a in row {
                let h = if cw > 0.0 { a / cw } else { 0.0 };
                out.push(Rect { x: free.x, y, w: cw, h });
                y += h;
            }
            free = Rect { x: free.x + cw, w: (free.w - cw).max(0.0), ..free };
        } else {
            // Row on top.
            let rh = if free.w > 0.0 { row_sum / free.w } else { 0.0 };
            let mut x = free.x;
            for a in row {
                let w = if rh > 0.0 { a / rh } else { 0.0 };
                out.push(Rect { x, y: free.y, w, h: rh });
                x += w;
            }
            free = Rect { y: free.y + rh, h: (free.h - rh).max(0.0), ..free };
        }
        start = end;
    }
    out
}

fn worst(row: &[f64], side: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    let (min, max) = row.iter().fold((f64::MAX, 0.0f64), |(lo, hi), &a| (lo.min(a), hi.max(a)));
    let s2 = side * side;
    let sum2 = sum * sum;
    (s2 * max / sum2).max(sum2 / (s2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect { x: 10.0, y: 20.0, w: 600.0, h: 400.0 };

    fn overlap(a: &Rect, b: &Rect) -> f64 {
        let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
        let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }

    #[test]
    fn areas_proportional_disjoint_and_inside() {
        let values = [60.0, 60.0, 40.0, 30.0, 20.0, 20.0, 10.0, 5.0, 1.0];
        let rects = squarify(&values, R);
        let total: f64 = values.iter().sum();
        let mut area_sum = 0.0;
        for (v, r) in values.iter().zip(&rects) {
            let expect = v / total * R.w * R.h;
            assert!((r.w * r.h - expect).abs() < 1e-6 * R.w * R.h, "{r:?} vs {expect}");
            assert!(r.x >= R.x - 1e-9 && r.y >= R.y - 1e-9);
            assert!(r.x + r.w <= R.x + R.w + 1e-6 && r.y + r.h <= R.y + R.h + 1e-6);
            area_sum += r.w * r.h;
        }
        assert!((area_sum - R.w * R.h).abs() < 1e-6);
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                assert!(overlap(&rects[i], &rects[j]) < 1e-6, "{i} overlaps {j}");
            }
        }
    }

    #[test]
    fn reasonably_square() {
        let values: Vec<f64> = (1..=30).rev().map(f64::from).collect();
        for r in squarify(&values, R) {
            let ratio = r.w.max(r.h) / r.w.min(r.h);
            assert!(ratio < 4.0, "{r:?}");
        }
    }

    #[test]
    fn degenerate_inputs() {
        assert!(squarify(&[], R).is_empty());
        assert_eq!(squarify(&[5.0], R), vec![R]);
        let zero = Rect { w: 0.0, ..R };
        assert_eq!(squarify(&[1.0, 2.0], zero).len(), 2);
    }
}
