//! Embedded Natural Earth 1:50m coastlines and land borders (public domain),
//! detailed enough to stay crisp when the map is zoomed in.
//!
//! Format of the `.bin` files, little-endian: for each polyline a `u16` point
//! count, then that many `(i16 lon×100, i16 lat×100)` pairs.

use std::sync::OnceLock;

use ratatui::style::Color;
use ratatui::widgets::canvas::{Line, Painter, Shape};

pub struct Polyline {
    /// min lon, min lat, max lon, max lat
    bbox: [f64; 4],
    points: Vec<(f64, f64)>,
}

fn parse(bytes: &[u8]) -> Vec<Polyline> {
    let mut lines = Vec::new();
    let mut rest = bytes;
    let int = |b: &[u8]| i16::from_le_bytes([b[0], b[1]]) as f64 / 100.0;
    while rest.len() >= 2 {
        let n = u16::from_le_bytes([rest[0], rest[1]]) as usize;
        let body = &rest[2..2 + n * 4];
        rest = &rest[2 + n * 4..];
        let points: Vec<(f64, f64)> = body
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (int(&p[0..2]), int(&p[2..4])))
            .collect();
        let mut bbox = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for &(x, y) in &points {
            bbox = [
                bbox[0].min(x),
                bbox[1].min(y),
                bbox[2].max(x),
                bbox[3].max(y),
            ];
        }
        lines.push(Polyline { bbox, points });
    }
    lines
}

pub fn coastlines() -> &'static [Polyline] {
    static DATA: OnceLock<Vec<Polyline>> = OnceLock::new();
    DATA.get_or_init(|| parse(include_bytes!("../assets/coast.bin")))
}

pub fn borders() -> &'static [Polyline] {
    static DATA: OnceLock<Vec<Polyline>> = OnceLock::new();
    DATA.get_or_init(|| parse(include_bytes!("../assets/borders.bin")))
}

/// Canvas shape drawing a set of polylines, skipping ones outside the view.
pub struct Lines {
    pub lines: &'static [Polyline],
    pub color: Color,
}

impl Shape for Lines {
    fn draw(&self, painter: &mut Painter) {
        let (&[x0, x1], &[y0, y1]) = painter.bounds();
        for line in self.lines {
            let [lx0, ly0, lx1, ly1] = line.bbox;
            if lx1 < x0 || lx0 > x1 || ly1 < y0 || ly0 > y1 {
                continue;
            }
            for w in line.points.windows(2) {
                let ((ax, ay), (bx, by)) = (w[0], w[1]);
                Line::new(ax, ay, bx, by, self.color).draw(painter);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_maps_parse() {
        let coast = coastlines();
        assert!(coast.len() > 1000);
        assert!(coast.iter().all(|l| l.points.len() >= 2));
        assert!(
            coast
                .iter()
                .all(|l| (-180.0..=180.0).contains(&l.bbox[0])
                    && (-90.0..=90.0).contains(&l.bbox[3]))
        );
        assert!(borders().len() > 100);
    }
}
