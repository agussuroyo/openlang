//! Where a floating window sits. Pure geometry in logical screen points.

use serde::{Deserialize, Serialize};

pub const MARGIN: f64 = 12.0;
const SNAP: f64 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Start,
    Middle,
    End,
}

impl Edge {
    fn fraction(self) -> f64 {
        match self {
            Self::Start => 0.0,
            Self::Middle => 0.5,
            Self::End => 1.0,
        }
    }

    fn of(position: f64) -> Self {
        match position {
            p if p < 1.0 / 3.0 => Self::Start,
            p if p > 2.0 / 3.0 => Self::End,
            _ => Self::Middle,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Place {
    Preset {
        h: Edge,
        v: Edge,
        /// The display it was chosen on; the main one when absent or unplugged.
        monitor: Option<String>,
    },
    /// The point that stays put when the window changes size, and which part of the window it is.
    Custom { x: f64, y: f64, h: Edge, v: Edge },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub name: Option<String>,
    pub area: Rect,
}

impl Place {
    pub fn preset(h: Edge, v: Edge) -> Self {
        Self::Preset {
            h,
            v,
            monitor: None,
        }
    }

    pub fn edges(&self) -> (Edge, Edge) {
        match self {
            Self::Preset { h, v, .. } | Self::Custom { h, v, .. } => (*h, *v),
        }
    }

    /// `monitors` starts with the main display. A custom place on an unplugged display falls back
    /// to the same corner of the main one.
    pub fn resolve(&self, (w, h): (f64, f64), monitors: &[Monitor]) -> Option<Rect> {
        let main = monitors.first()?;
        let (eh, ev) = self.edges();
        let at = |m: &Monitor| {
            let free = |start: f64, room: f64, size: f64, e: Edge| {
                start + MARGIN + (room - size - 2.0 * MARGIN) * e.fraction()
            };
            Rect {
                x: free(m.area.x, m.area.w, w, eh),
                y: free(m.area.y, m.area.h, h, ev),
                w,
                h,
            }
        };
        Some(match self {
            Self::Preset { monitor, .. } => {
                at(monitors.iter().find(|m| m.name == *monitor).unwrap_or(main))
            }
            Self::Custom { x, y, .. } => {
                let rect = Rect {
                    x: x - w * eh.fraction(),
                    y: y - h * ev.fraction(),
                    w,
                    h,
                };
                match monitors.iter().find(|m| m.area.contains(*x, *y)) {
                    Some(m) => clamp(rect, &m.area),
                    None => at(main),
                }
            }
        })
    }

    /// Remembers where a window was dropped.
    pub fn dropped(rect: Rect, monitors: &[Monitor]) -> Option<Self> {
        let m = nearest(rect, monitors)?;
        let r = clamp(rect, &m.area);
        let h = Edge::of((r.x + r.w / 2.0 - m.area.x) / m.area.w);
        let v = Edge::of((r.y + r.h / 2.0 - m.area.y) / m.area.h);
        Some(Self::Custom {
            x: r.x + r.w * h.fraction(),
            y: r.y + r.h * v.fraction(),
            h,
            v,
        })
    }
}

fn clamp(r: Rect, area: &Rect) -> Rect {
    let keep = |p: f64, size: f64, start: f64, room: f64| p.min(start + room - size).max(start);
    Rect {
        x: keep(r.x, r.w, area.x, area.w),
        y: keep(r.y, r.h, area.y, area.h),
        ..r
    }
}

pub fn nearest(r: Rect, monitors: &[Monitor]) -> Option<&Monitor> {
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    monitors.iter().find(|m| m.area.contains(x, y)).or_else(|| {
        let far = |m: &Monitor| {
            let (dx, dy) = (x - m.area.x - m.area.w / 2.0, y - m.area.y - m.area.h / 2.0);
            dx * dx + dy * dy
        };
        monitors.iter().min_by(|a, b| far(a).total_cmp(&far(b)))
    })
}

/// Pulls a dragged window onto the margins and centre lines when it comes close.
pub fn snap(r: Rect, monitors: &[Monitor]) -> Rect {
    let Some(m) = nearest(r, monitors) else {
        return r;
    };
    let pull = |p: f64, start: f64, room: f64, size: f64| {
        [
            start + MARGIN,
            start + (room - size) / 2.0,
            start + room - size - MARGIN,
        ]
        .into_iter()
        .find(|t| (p - t).abs() < SNAP)
        .unwrap_or(p)
    };
    Rect {
        x: pull(r.x, m.area.x, m.area.w, r.w),
        y: pull(r.y, m.area.y, m.area.h, r.h),
        ..r
    }
}

/// Moves `r` beside `other` when they would cover each other: above it, or below near the top.
pub fn beside(r: Rect, other: Rect, monitors: &[Monitor]) -> Rect {
    if !r.overlaps(&other) {
        return r;
    }
    let above = other.y - r.h;
    match nearest(r, monitors) {
        Some(m) if above < m.area.y => Rect {
            y: other.y + other.h,
            ..r
        },
        _ => Rect { y: above, ..r },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screens() -> Vec<Monitor> {
        vec![
            Monitor {
                name: Some("a".into()),
                area: Rect {
                    x: 0.0,
                    y: 25.0,
                    w: 1500.0,
                    h: 900.0,
                },
            },
            Monitor {
                name: Some("b".into()),
                area: Rect {
                    x: 1500.0,
                    y: -200.0,
                    w: 2500.0,
                    h: 1400.0,
                },
            },
        ]
    }

    #[test]
    fn presets_stay_inside_their_display() {
        for (i, name) in ["a", "b"].iter().enumerate() {
            for h in [Edge::Start, Edge::Middle, Edge::End] {
                for v in [Edge::Start, Edge::Middle, Edge::End] {
                    let p = Place::Preset {
                        h,
                        v,
                        monitor: Some((*name).to_owned()),
                    };
                    let r = p.resolve((480.0, 120.0), &screens()).unwrap();
                    assert_eq!(clamp(r, &screens()[i].area), r);
                }
            }
        }
    }

    #[test]
    fn growing_keeps_the_outer_edge_in_place() {
        let drop = Rect {
            x: 1300.0,
            y: 40.0,
            w: 190.0,
            h: 50.0,
        };
        let p = Place::dropped(drop, &screens()).unwrap();
        assert_eq!(p.resolve((190.0, 50.0), &screens()).unwrap(), drop);
        let big = p.resolve((360.0, 80.0), &screens()).unwrap();
        assert_eq!(big.x + big.w, drop.x + drop.w);
        assert_eq!(big.y, drop.y);
    }

    #[test]
    fn an_unplugged_display_never_hides_a_window() {
        let p = Place::dropped(
            Rect {
                x: 3000.0,
                y: 900.0,
                w: 400.0,
                h: 100.0,
            },
            &screens(),
        )
        .unwrap();
        let one = &screens()[..1];
        let r = p.resolve((400.0, 100.0), one).unwrap();
        assert_eq!(clamp(r, &one[0].area), r);
        let q = Place::Preset {
            h: Edge::End,
            v: Edge::End,
            monitor: Some("b".into()),
        };
        let r = q.resolve((400.0, 100.0), one).unwrap();
        assert_eq!(clamp(r, &one[0].area), r);
    }

    #[test]
    fn snaps_only_when_close() {
        let near = Rect {
            x: 20.0,
            y: 400.0,
            w: 400.0,
            h: 100.0,
        };
        assert_eq!(snap(near, &screens()).x, MARGIN);
        let far = Rect {
            x: 60.0,
            y: 300.0,
            w: 400.0,
            h: 100.0,
        };
        assert_eq!(snap(far, &screens()), far);
    }

    #[test]
    fn controls_never_cover_the_captions() {
        for v in [Edge::Start, Edge::Middle, Edge::End] {
            let p = Place::preset(Edge::Middle, v);
            let caption = p.resolve((480.0, 120.0), &screens()).unwrap();
            let island = p.resolve((200.0, 50.0), &screens()).unwrap();
            let moved = beside(island, caption, &screens());
            assert!(!moved.overlaps(&caption));
            assert_eq!(clamp(moved, &screens()[0].area), moved);
        }
    }
}
