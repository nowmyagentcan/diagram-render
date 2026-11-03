//! Freehand strokes as filled outlines: perfect-freehand 1.2.0's `getStroke` (MIT,
//! github.com/steveruizok/perfect-freehand, `packages/perfect-freehand/src`) with the options and
//! path data of Excalidraw 0.18.1's `getFreeDrawSvgPath` (`renderer/renderElement.ts`, MIT).

use std::f64::consts::PI;

type V = (f64, f64);

const RATE_OF_PRESSURE_CHANGE: f64 = 0.275;
const FIXED_PI: f64 = PI + 0.0001;

fn add(a: V, b: V) -> V {
    (a.0 + b.0, a.1 + b.1)
}

fn sub(a: V, b: V) -> V {
    (a.0 - b.0, a.1 - b.1)
}

fn mul(a: V, n: f64) -> V {
    (a.0 * n, a.1 * n)
}

fn per(a: V) -> V {
    (a.1, -a.0)
}

fn dpr(a: V, b: V) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

fn dist2(a: V, b: V) -> f64 {
    let d = sub(a, b);
    d.0 * d.0 + d.1 * d.1
}

fn uni(a: V) -> V {
    let l = a.0.hypot(a.1);
    (a.0 / l, a.1 / l)
}

fn lrp(a: V, b: V, t: f64) -> V {
    add(a, mul(sub(b, a), t))
}

fn rot_around(a: V, c: V, r: f64) -> V {
    let (s, co) = r.sin_cos();
    let (px, py) = (a.0 - c.0, a.1 - c.1);
    (px * co - py * s + c.0, px * s + py * co + c.1)
}

struct StrokePoint {
    point: V,
    pressure: f64,
    vector: V,
    distance: f64,
    running: f64,
}

/// Excalidraw's options: size 4.25 × the stroke width, thinning 0.6, streamline 0.5, smoothing
/// 0.5, easeOutSine, no tapers, caps on, and `last` off: restored scenes have no last point.
struct Options {
    size: f64,
    simulate: bool,
}

const THINNING: f64 = 0.6;
const SMOOTHING: f64 = 0.5;
const STREAMLINE: f64 = 0.5;

fn ease(t: f64) -> f64 {
    (t * PI / 2.0).sin()
}

fn radius(size: f64, pressure: f64) -> f64 {
    size * ease(0.5 - THINNING * (0.5 - pressure))
}

/// `getStrokePoints`: each point `(x, y, pressure)`; a pressure below 0 is none given.
fn stroke_points(input: &[(f64, f64, f64)], o: &Options) -> Vec<StrokePoint> {
    if input.is_empty() {
        return vec![];
    }
    let t = 0.15 + (1.0 - STREAMLINE) * 0.85;
    let mut pts = input.to_vec();
    if pts.len() == 2 {
        let last = pts[1];
        pts.truncate(1);
        for i in 1..5 {
            let f = f64::from(i) / 4.0;
            let (a, b) = (pts[0], last);
            // perfect-freehand's lrp keeps x and y only, so these have no pressure.
            pts.push((a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, -1.0));
        }
    }
    if pts.len() == 1 {
        let p = pts[0];
        pts.push((p.0 + 1.0, p.1 + 1.0, p.2));
    }
    let given = |p: f64, otherwise: f64| if p >= 0.0 { p } else { otherwise };
    let mut out = vec![StrokePoint {
        point: (pts[0].0, pts[0].1),
        pressure: given(pts[0].2, 0.25),
        vector: (1.0, 1.0),
        distance: 0.0,
        running: 0.0,
    }];
    let mut reached = false;
    let mut running = 0.0;
    let max = pts.len() - 1;
    for (i, &p) in pts.iter().enumerate().skip(1) {
        let prev = out.last().unwrap().point;
        let point = lrp(prev, (p.0, p.1), t);
        if prev == point {
            continue;
        }
        let distance = (point.1 - prev.1).hypot(point.0 - prev.0);
        running += distance;
        if i < max && !reached {
            if running < o.size {
                continue;
            }
            reached = true;
        }
        out.push(StrokePoint {
            point,
            pressure: given(p.2, 0.5),
            vector: uni(sub(prev, point)),
            distance,
            running,
        });
    }
    out[0].vector = out.get(1).map_or((0.0, 0.0), |p| p.vector);
    out
}

/// `getStrokeOutlinePoints` with no tapers and both caps.
fn outline(points: &[StrokePoint], o: &Options) -> Vec<V> {
    let size = o.size;
    if points.is_empty() || size <= 0.0 {
        return vec![];
    }
    let total = points[points.len() - 1].running;
    let min_distance = (size * SMOOTHING).powi(2);
    let (mut left, mut right): (Vec<V>, Vec<V>) = (vec![], vec![]);
    let simulated = |acc: f64, distance: f64| {
        let sp = (distance / size).min(1.0);
        let rp = (1.0 - sp).min(1.0);
        (acc + (rp - acc) * (sp * RATE_OF_PRESSURE_CHANGE)).min(1.0)
    };
    let mut prev_pressure = points.iter().take(10).fold(points[0].pressure, |acc, p| {
        let pressure = if o.simulate {
            simulated(acc, p.distance)
        } else {
            p.pressure
        };
        (acc + pressure) / 2.0
    });
    let mut r = radius(size, points[points.len() - 1].pressure);
    let mut first_radius = None;
    let mut prev_vector = points[0].vector;
    let (mut pl, mut pr) = (points[0].point, points[0].point);
    let (mut tl, mut tr) = (pl, pr);
    let mut prev_sharp = false;
    for (i, p) in points.iter().enumerate() {
        let mut pressure = p.pressure;
        if i < points.len() - 1 && total - p.running < 3.0 {
            continue;
        }
        if o.simulate {
            pressure = simulated(prev_pressure, p.distance);
        }
        r = radius(size, pressure);
        first_radius.get_or_insert(r);
        r = r.max(0.01);
        let next_vector = points.get(i + 1).unwrap_or(p).vector;
        let next_dpr = if i < points.len() - 1 {
            dpr(p.vector, next_vector)
        } else {
            1.0
        };
        let prev_dpr = dpr(p.vector, prev_vector);
        let sharp = prev_dpr < 0.0 && !prev_sharp;
        let next_sharp = next_dpr < 0.0;
        if sharp || next_sharp {
            let offset = mul(per(prev_vector), r);
            let step = 1.0 / 13.0;
            let mut t = 0.0;
            while t <= 1.0 {
                tl = rot_around(sub(p.point, offset), p.point, FIXED_PI * t);
                left.push(tl);
                tr = rot_around(add(p.point, offset), p.point, FIXED_PI * -t);
                right.push(tr);
                t += step;
            }
            pl = tl;
            pr = tr;
            if next_sharp {
                prev_sharp = true;
            }
            continue;
        }
        prev_sharp = false;
        if i == points.len() - 1 {
            let offset = mul(per(p.vector), r);
            left.push(sub(p.point, offset));
            right.push(add(p.point, offset));
            continue;
        }
        let offset = mul(per(lrp(next_vector, p.vector, next_dpr)), r);
        tl = sub(p.point, offset);
        if i <= 1 || dist2(pl, tl) > min_distance {
            left.push(tl);
            pl = tl;
        }
        tr = add(p.point, offset);
        if i <= 1 || dist2(pr, tr) > min_distance {
            right.push(tr);
            pr = tr;
        }
        prev_pressure = pressure;
        prev_vector = p.vector;
    }
    let first = points[0].point;
    let last = if points.len() > 1 {
        points[points.len() - 1].point
    } else {
        add(first, (1.0, 1.0))
    };
    let step = 1.0 / 13.0;
    if points.len() == 1 {
        let start = add(
            first,
            mul(uni(per(sub(first, last))), -first_radius.unwrap_or(r)),
        );
        let mut dot = vec![];
        let mut t = step;
        while t <= 1.0 {
            dot.push(rot_around(start, first, FIXED_PI * 2.0 * t));
            t += step;
        }
        return dot;
    }
    let mut start_cap = vec![];
    if let Some(&r0) = right.first() {
        let mut t = step;
        while t <= 1.0 {
            start_cap.push(rot_around(r0, first, FIXED_PI * t));
            t += step;
        }
    }
    let v = points[points.len() - 1].vector;
    let direction = per((-v.0, -v.1));
    let start = add(last, mul(direction, r));
    let mut end_cap = vec![];
    let step = 1.0 / 29.0;
    let mut t = step;
    while t < 1.0 {
        end_cap.push(rot_around(start, last, FIXED_PI * 3.0 * t));
        t += step;
    }
    right.reverse();
    left.into_iter()
        .chain(end_cap)
        .chain(right)
        .chain(start_cap)
        .collect()
}

/// A number as Excalidraw's `TO_FIXED_PRECISION` leaves it: cut, not rounded, after 2 decimals.
fn cut(v: f64) -> String {
    let s = format!("{}", v + 0.0);
    match s.split_once('.') {
        Some((whole, part)) => format!("{whole}.{}", &part[..part.len().min(2)]),
        None => s,
    }
}

/// The outline of a freedraw element's points (pressures from 0 to 1, or simulated) as SVG path
/// data, in the element's own coordinates.
pub fn path(points: &[(f64, f64)], pressures: &[f64], simulate: bool, stroke_width: f64) -> String {
    let input: Vec<(f64, f64, f64)> = if points.is_empty() {
        vec![(0.0, 0.0, 0.5)]
    } else {
        (points.iter().enumerate())
            .map(|(i, &(x, y))| {
                let p = if simulate {
                    -1.0
                } else {
                    pressures.get(i).copied().unwrap_or(-1.0)
                };
                (x, y, p)
            })
            .collect()
    };
    let o = Options {
        size: stroke_width * 4.25,
        simulate,
    };
    let stroke = outline(&stroke_points(&input, &o), &o);
    let Some(&first) = stroke.first() else {
        return String::new();
    };
    let med = |a: V, b: V| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let pt = |p: V| format!("{},{}", cut(p.0), cut(p.1));
    let mut d = format!("M {} Q", pt(first));
    for (i, &p) in stroke.iter().enumerate() {
        let next = stroke.get(i + 1).copied().unwrap_or(first);
        d += &format!(" {} {}", pt(p), pt(med(p, next)));
        if i == stroke.len() - 1 {
            d += &format!(" L {} Z", pt(first));
        }
    }
    d
}
