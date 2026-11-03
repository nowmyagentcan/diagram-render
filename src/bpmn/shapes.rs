//! The BPMN shapes with their icons and markers, in our own drawing made to read like bpmn-js's:
//! each shape is a group moved to its place, drawn in its own coordinates.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::fmt::Write;

use roxmltree::Node;

use super::{MODEL, Rect, n};

/// A shape's colours.
pub struct Paint<'a> {
    pub fill: &'a str,
    pub stroke: &'a str,
}

/// Draws `el`, the model element `di` places at `b`. `false` when its type isn't one of the
/// flow elements drawn here.
pub fn flow_node(
    out: &mut String,
    model: &HashMap<&str, Node>,
    el: Node,
    di: Node,
    b: Rect,
    p: &Paint,
) -> bool {
    let kind = el.tag_name().name();
    let expanded = di.attribute("isExpanded") == Some("true");
    let _ = write!(out, r#"<g transform="translate({} {})">"#, n(b.x), n(b.y));
    match kind {
        "task" | "userTask" | "serviceTask" | "scriptTask" | "sendTask" | "receiveTask"
        | "manualTask" | "businessRuleTask" => {
            activity(out, b, p, 2.0, "");
            task_icon(out, kind, p);
            markers(out, el, b, p, false);
        }
        "subProcess" | "adHocSubProcess" | "transaction" => {
            let event = el.attribute("triggeredByEvent") == Some("true");
            match (kind, event) {
                ("transaction", _) => {
                    activity(out, b, p, 1.5, "");
                    let inner = Rect {
                        x: 3.0,
                        y: 3.0,
                        w: b.w - 6.0,
                        h: b.h - 6.0,
                    };
                    rect(out, inner, 7.0, "none", p.stroke, 1.5, "");
                }
                (_, true) => activity(out, b, p, 2.5, DOTTED),
                _ => activity(out, b, p, 2.0, ""),
            }
            if event && !expanded {
                let starts: Vec<Node> = (el.children())
                    .filter(|c| c.tag_name().name() == "startEvent")
                    .collect();
                if let [start] = starts[..] {
                    event_icon_small(out, start, p);
                }
            }
            markers(out, el, b, p, !expanded);
        }
        "callActivity" => {
            activity(out, b, p, 5.0, "");
            markers(out, el, b, p, !expanded);
        }
        "startEvent"
        | "endEvent"
        | "intermediateCatchEvent"
        | "intermediateThrowEvent"
        | "boundaryEvent" => event(out, el, b, p),
        "exclusiveGateway" | "parallelGateway" | "inclusiveGateway" | "complexGateway"
        | "eventBasedGateway" => gateway(out, el, di, b, p),
        "dataObjectReference" | "dataObject" | "dataInput" | "dataOutput" => {
            let object = match kind {
                "dataObjectReference" => el.attribute("dataObjectRef").and_then(|r| model.get(r)),
                _ => Some(&el),
            };
            let collection = object.and_then(|o| o.attribute("isCollection")) == Some("true")
                || el.attribute("isCollection") == Some("true");
            data_object(out, kind, collection, b, p);
        }
        "dataStoreReference" => data_store(out, b, p),
        _ => {
            out.push_str("</g>\n");
            return false;
        }
    }
    out.push_str("</g>\n");
    true
}

const DOTTED: &str = r#" stroke-dasharray="0.1 5.4" stroke-linecap="round""#;
const DASHED: &str = r#" stroke-dasharray="6 3""#;

/// `fill`, `stroke` and `stroke-width` attributes.
fn style(fill: &str, stroke: &str, width: f64) -> String {
    format!(
        r#"fill="{fill}" stroke="{stroke}" stroke-width="{}""#,
        n(width)
    )
}

pub fn rect(out: &mut String, b: Rect, r: f64, fill: &str, stroke: &str, w: f64, extra: &str) {
    let _ = write!(
        out,
        r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" {}{extra}/>"#,
        n(b.x),
        n(b.y),
        n(b.w),
        n(b.h),
        n(r),
        style(fill, stroke, w)
    );
}

pub fn path(out: &mut String, d: &str, fill: &str, stroke: &str, w: f64, extra: &str) {
    let _ = write!(out, r#"<path d="{d}" {}{extra}/>"#, style(fill, stroke, w));
}

fn circle(out: &mut String, (cx, cy): (f64, f64), r: f64, fill: &str, stroke: &str, w: f64) {
    circle_with(out, (cx, cy), r, &style(fill, stroke, w));
}

fn circle_with(out: &mut String, (cx, cy): (f64, f64), r: f64, attrs: &str) {
    let _ = write!(
        out,
        r#"<circle cx="{}" cy="{}" r="{}" {attrs}/>"#,
        n(cx),
        n(cy),
        n(r.max(0.0))
    );
}

/// The rounded box of every activity.
fn activity(out: &mut String, b: Rect, p: &Paint, width: f64, extra: &str) {
    let own = Rect {
        x: 0.0,
        y: 0.0,
        ..b
    };
    rect(out, own, 10.0, p.fill, p.stroke, width, extra);
}

/// The task type's icon in the top-left corner.
fn task_icon(out: &mut String, kind: &str, p: &Paint) {
    let (f, s) = (p.fill, p.stroke);
    match kind {
        "userTask" => {
            circle(out, (15.0, 11.5), 3.8, f, s, 1.0);
            path(out, "M8 23c0-5 3-7.5 7-7.5s7 2.5 7 7.5z", f, s, 1.0, "");
            path(out, "M12.5 16.5l2.5 3 2.5-3", "none", s, 1.0, "");
        }
        "serviceTask" => {
            // Teeth as a dashed ring, then the wheel and its hole.
            circle_with(
                out,
                (14.0, 14.0),
                6.0,
                &format!(r#"{} stroke-dasharray="2.36 2.36""#, style("none", s, 3.0)),
            );
            circle(out, (14.0, 14.0), 4.6, f, s, 1.0);
            circle(out, (14.0, 14.0), 1.8, f, s, 1.0);
        }
        "scriptTask" => {
            let scroll =
                "M10 7h11c-2.5 2-2.5 4 0 6.5s2.5 4.5 0 6.5H10c2.5-2 2.5-4 0-6.5S7.5 9 10 7z";
            path(out, scroll, f, s, 1.0, "");
            path(
                out,
                "M11.5 10.5h7M12.8 13.5h7M12.8 16.5h7",
                "none",
                s,
                1.0,
                "",
            );
        }
        "sendTask" | "receiveTask" => {
            let filled = kind == "sendTask";
            let (body, flap) = if filled { (s, f) } else { (f, s) };
            path(out, "M6 8h16v11H6z", body, s, 1.0, "");
            path(out, "M6 8l8 6 8-6", "none", flap, 1.0, "");
        }
        "manualTask" => {
            let hand = "M5.5 12.5c0-1 .6-1.8 1.6-2.2l5.4-2.3c.9-.4 1.8.7 1.1 1.5L12 12h8.6a1.2 1.2 0 0 1 0 2.4H15h6a1.2 1.2 0 0 1 0 2.4h-6 4.8a1.2 1.2 0 0 1 0 2.4H15h3a1.1 1.1 0 0 1 0 2.2H9c-2 0-3.5-1.5-3.5-3.5z";
            path(out, hand, f, s, 0.8, "");
        }
        "businessRuleTask" => {
            path(out, "M6 7h16v12H6z", f, s, 1.0, "");
            path(out, "M6 7h16v3.5H6z", s, s, 1.0, "");
            path(out, "M6 14.5h16M11 10.5v8.5", "none", s, 1.0, "");
        }
        _ => {}
    }
}

/// The markers along the bottom of an activity: loop or multi-instance, compensation, the
/// collapsed sub-process's plus and the ad-hoc tilde, side by side in the middle.
fn markers(out: &mut String, el: Node, b: Rect, p: &Paint, collapsed: bool) {
    let mut list = vec![];
    if let Some(l) = el.children().find(|c| {
        c.tag_name().namespace() == Some(MODEL)
            && c.tag_name().name().ends_with("LoopCharacteristics")
    }) {
        list.push(match l.tag_name().name() {
            "standardLoopCharacteristics" => "loop",
            _ if l.attribute("isSequential") == Some("true") => "sequential",
            _ => "parallel",
        });
    }
    if el.attribute("isForCompensation") == Some("true") {
        list.push("compensation");
    }
    if collapsed {
        list.push("plus");
    }
    if el.tag_name().name() == "adHocSubProcess" {
        list.push("adhoc");
    }
    let total = list.len() as f64 * 18.0 - 4.0;
    let s = p.stroke;
    for (i, marker) in list.iter().enumerate() {
        let x = (b.w - total) / 2.0 + i as f64 * 18.0;
        let _ = write!(
            out,
            r#"<g transform="translate({} {})">"#,
            n(x),
            n(b.h - 18.0)
        );
        match *marker {
            "loop" => {
                path(out, "M3.2 11.2A5.5 5.5 0 1 1 10.8 11.2", "none", s, 1.5, "");
                path(out, "M10.8 7.7v3.8h3.8", "none", s, 1.5, "");
            }
            "parallel" => path(out, "M3 1v12M7 1v12M11 1v12", "none", s, 2.0, ""),
            "sequential" => path(out, "M1 3h12M1 7h12M1 11h12", "none", s, 2.0, ""),
            "compensation" => path(out, "M0 7l7-5v10zM7 7l7-5v10z", p.fill, s, 1.0, ""),
            "plus" => {
                path(out, "M0 0h14v14H0z", p.fill, s, 1.0, "");
                path(out, "M7 3v8M3 7h8", "none", s, 1.5, "");
            }
            _ => path(
                out,
                "M0.5 8.5c2-4 4.5-4 6.5 0s4.5 4 6.5 0",
                "none",
                s,
                1.5,
                "",
            ),
        }
        out.push_str("</g>");
    }
}

/// An event's circles and the icon of its definition.
fn event(out: &mut String, el: Node, b: Rect, p: &Paint) {
    let kind = el.tag_name().name();
    let r = b.w.min(b.h) / 2.0;
    let c = (b.w / 2.0, b.h / 2.0);
    let dashed = el.attribute("isInterrupting") == Some("false")
        || el.attribute("cancelActivity") == Some("false");
    let dash = if dashed { DASHED } else { "" };
    match kind {
        "startEvent" => circle_with(out, c, r, &(style(p.fill, p.stroke, 2.0) + dash)),
        "endEvent" => circle(out, c, r, p.fill, p.stroke, 4.0),
        _ => {
            circle_with(out, c, r, &(style(p.fill, p.stroke, 1.5) + dash));
            circle_with(out, c, r - 3.0, &(style("none", p.stroke, 1.5) + dash));
        }
    }
    let filled = matches!(kind, "endEvent" | "intermediateThrowEvent");
    if let Some(icon) = definition(el) {
        let s = r / 18.0;
        let _ = write!(
            out,
            r#"<g transform="translate({} {}) scale({})">"#,
            n(c.0 - r),
            n(c.1 - r),
            n(s)
        );
        event_icon(out, icon, filled, p);
        out.push_str("</g>");
    }
}

/// The icon an event's definitions call for: one's own, else multiple or parallel multiple.
fn definition<'a>(el: Node<'a, '_>) -> Option<&'a str> {
    let defs: Vec<&str> = (el.children())
        .filter(|c| c.tag_name().namespace() == Some(MODEL))
        .map(|c| c.tag_name().name())
        .filter(|name| name.ends_with("EventDefinition"))
        .collect();
    match defs[..] {
        [] => None,
        [one] => Some(one),
        _ if el.attribute("parallelMultiple") == Some("true") => Some("parallelMultiple"),
        _ => Some("multiple"),
    }
}

/// The small start event a collapsed event sub-process shows in its corner.
fn event_icon_small(out: &mut String, start: Node, p: &Paint) {
    let dashed = start.attribute("isInterrupting") == Some("false");
    let dash = if dashed {
        r#" stroke-dasharray="3""#
    } else {
        ""
    };
    let width = if dashed { 1.2 } else { 1.0 };
    circle_with(
        out,
        (11.0, 11.0),
        10.0,
        &(style(p.fill, p.stroke, width) + dash),
    );
    if let Some(icon) = definition(start) {
        out.push_str(r#"<g transform="scale(0.611)">"#);
        event_icon(out, icon, false, p);
        out.push_str("</g>");
    }
}

/// An event definition's icon in a 36 × 36 box; a throwing event's is filled.
fn event_icon(out: &mut String, icon: &str, filled: bool, p: &Paint) {
    let (f, s) = (p.fill, p.stroke);
    let solid = if filled { s } else { f };
    match icon {
        "messageEventDefinition" => {
            path(out, "M9 12h18v12H9z", solid, s, 1.0, "");
            path(
                out,
                "M9 12l9 7 9-7",
                "none",
                if filled { f } else { s },
                1.0,
                "",
            );
        }
        "timerEventDefinition" => {
            circle(out, (18.0, 18.0), 10.5, f, s, 2.0);
            let mut ticks = String::new();
            for k in 0..12 {
                let a = f64::from(k) * PI / 6.0;
                let (x, y) = (a.cos(), a.sin());
                let _ = write!(
                    ticks,
                    "M{} {}L{} {}",
                    n(18.0 + 8.0 * x),
                    n(18.0 + 8.0 * y),
                    n(18.0 + 10.0 * x),
                    n(18.0 + 10.0 * y)
                );
            }
            path(out, &ticks, "none", s, 1.0, "");
            path(out, "M18 18V11M18 18l5 2.5", "none", s, 1.0, "");
        }
        "escalationEventDefinition" => path(out, "M18 9l7 17-7-6.5-7 6.5z", solid, s, 1.0, ""),
        "conditionalEventDefinition" => {
            path(out, "M11.5 9.5h13v17h-13z", f, s, 1.0, "");
            path(
                out,
                "M14 13.5h8M14 17h8M14 20.5h8M14 24h8",
                "none",
                s,
                1.0,
                "",
            );
        }
        "linkEventDefinition" => path(out, "M9.5 15h9v-5l8 8-8 8v-5h-9z", solid, s, 1.0, ""),
        "errorEventDefinition" => {
            path(
                out,
                "M10.5 26.5l4-16 5.5 8.5 5.5-9.5-4 16-5.5-8.5z",
                solid,
                s,
                1.0,
                "",
            );
        }
        "cancelEventDefinition" => {
            let cross = "M11.5 13.5l2-2 4.5 4.5 4.5-4.5 2 2-4.5 4.5 4.5 4.5-2 2-4.5-4.5-4.5 4.5-2-2 4.5-4.5z";
            path(out, cross, solid, s, 1.0, "");
        }
        "compensateEventDefinition" => {
            path(out, "M8.5 18l8.5-6v12zM17 18l8.5-6v12z", solid, s, 1.0, "");
        }
        "signalEventDefinition" => path(out, "M18 9.5l9.5 16.5h-19z", solid, s, 1.0, ""),
        "terminateEventDefinition" => circle(out, (18.0, 18.0), 9.0, s, s, 1.0),
        "multiple" => path(out, &polygon((18.0, 18.6), 9.5, 5), solid, s, 1.0, ""),
        "parallelMultiple" => {
            let plus = "M15.5 9h5v6.5H27v5h-6.5V27h-5v-6.5H9v-5h6.5z";
            path(out, plus, f, s, 1.0, "");
        }
        _ => {}
    }
}

/// A regular polygon with one corner straight up.
fn polygon((cx, cy): (f64, f64), r: f64, corners: u32) -> String {
    let mut d = String::new();
    for k in 0..corners {
        let a = -PI / 2.0 + f64::from(k) * 2.0 * PI / f64::from(corners);
        let _ = write!(
            d,
            "{}{} {}",
            if k == 0 { "M" } else { "L" },
            n(cx + r * a.cos()),
            n(cy + r * a.sin())
        );
    }
    d + "z"
}

/// A gateway's diamond and its marker, in a 50 × 50 box stretched to the shape.
fn gateway(out: &mut String, el: Node, di: Node, b: Rect, p: &Paint) {
    let (w, h) = (b.w, b.h);
    let diamond = format!(
        "M{} 0L{} {}L{} {}L0 {}z",
        n(w / 2.0),
        n(w),
        n(h / 2.0),
        n(w / 2.0),
        n(h),
        n(h / 2.0)
    );
    path(out, &diamond, p.fill, p.stroke, 2.0, "");
    let _ = write!(
        out,
        r#"<g transform="scale({} {})">"#,
        n(w / 50.0),
        n(h / 50.0)
    );
    let s = p.stroke;
    match el.tag_name().name() {
        "exclusiveGateway" if di.attribute("isMarkerVisible") == Some("true") => {
            path(out, "M17.5 17.5l15 15M32.5 17.5l-15 15", "none", s, 4.0, "");
        }
        "parallelGateway" => path(out, "M25 12.5v25M12.5 25h25", "none", s, 4.0, ""),
        "inclusiveGateway" => circle(out, (25.0, 25.0), 11.5, "none", s, 2.5),
        "complexGateway" => {
            let star = "M25 13v24M13 25h24M16.5 16.5l17 17M33.5 16.5l-17 17";
            path(out, star, "none", s, 3.0, "");
        }
        "eventBasedGateway" => {
            circle(out, (25.0, 25.0), 12.5, "none", s, 1.0);
            if el.attribute("eventGatewayType") == Some("Parallel") {
                let plus = "M23 17h4v6h6v4h-6v6h-4v-6h-6v-4h6z";
                path(out, plus, "none", s, 1.0, "");
            } else {
                if el.attribute("instantiate") != Some("true") {
                    circle(out, (25.0, 25.0), 10.0, "none", s, 1.0);
                }
                path(out, &polygon((25.0, 25.6), 6.5, 5), "none", s, 1.5, "");
            }
        }
        _ => {}
    }
    out.push_str("</g>");
}

/// A data object's folded page with its collection, input or output marker.
fn data_object(out: &mut String, kind: &str, collection: bool, b: Rect, p: &Paint) {
    let (w, h) = (b.w, b.h);
    let fold = (w * 0.28).min(h * 0.2);
    let page = format!("M0 0H{}L{} {}V{}H0z", n(w - fold), n(w), n(fold), n(h));
    path(out, &page, p.fill, p.stroke, 1.0, "");
    let corner = format!("M{} 0V{}H{}", n(w - fold), n(fold), n(w));
    path(out, &corner, "none", p.stroke, 1.0, "");
    let arrow = "M4 6h6V3l5 5-5 5v-3H4z";
    match kind {
        "dataInput" => path(out, arrow, p.fill, p.stroke, 1.0, ""),
        "dataOutput" => path(out, arrow, p.stroke, p.stroke, 1.0, ""),
        _ => {}
    }
    if collection {
        let x = w / 2.0 - 5.0;
        let bars = format!("M{} {}v10m4-10v10m4-10v10", n(x + 1.0), n(h - 13.0));
        path(out, &bars, "none", p.stroke, 2.0, "");
    }
}

/// A data store's cylinder.
fn data_store(out: &mut String, b: Rect, p: &Paint) {
    let (w, h) = (b.w, b.h);
    let e = (h * 0.12).min(8.0);
    let rx = w / 2.0;
    let body = format!(
        "M0 {e0}A{rx} {e} 0 0 1 {w} {e0}V{b0}A{rx} {e} 0 0 1 0 {b0}z",
        e0 = n(e),
        rx = n(rx),
        e = n(e),
        w = n(w),
        b0 = n(h - e)
    );
    path(out, &body, p.fill, p.stroke, 2.0, "");
    let mut rims = String::new();
    for k in 0..3 {
        let y = e + f64::from(k) * 5.0;
        let _ = write!(
            rims,
            "M0 {y}A{rx} {e} 0 0 0 {w} {y}",
            y = n(y),
            rx = n(rx),
            e = n(e),
            w = n(w)
        );
    }
    path(out, &rims, "none", p.stroke, 2.0, "");
}
