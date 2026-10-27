// Mush8 - touch.rs
// Constantin Clerc (c22dev) - 0.1
use crate::input::{self, A, B, L, MINUS, PLUS, R, X, Y};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSSet, NSString};
use objc2_ui_kit::{NSTextAlignment, UIColor, UIEvent, UIFont, UILabel, UIResponder, UITouch, UIView, UIViewAutoresizing};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::LazyLock;
use std::time::Instant;

static START: LazyLock<Instant> = LazyLock::new(Instant::now);
static LAST: AtomicU64 = AtomicU64::new(0);
static IDLE: AtomicBool = AtomicBool::new(false);

fn mark() {
    LAST.store(START.elapsed().as_millis() as u64, Relaxed);
}

pub fn idle_secs() -> f32 {
    (START.elapsed().as_millis() as u64).saturating_sub(LAST.load(Relaxed)) as f32 / 1000.0
}

pub fn set_idle(idle: bool) {
    PAD.with(|p| {
        let mut p = p.borrow_mut();
        let Some(p) = p.as_mut() else { return };
        if idle && p.shown {
            IDLE.store(true, Relaxed);
            hide(p);
        } else if !idle && IDLE.swap(false, Relaxed) {
            show_controls(p, true);
            p.shown = true;
        }
    });
}

const BUTTONS: [(&str, u64); 8] = [("A", A), ("B", B), ("X", X), ("Y", Y), ("L", L), ("R", R), ("+", PLUS), ("−", MINUS)];
const FILL: f64 = 0.35;
const RING: f64 = 0.8;
const PRESSED: f64 = 0.7;
const STICK_REACH: f64 = 2.5;

struct Control {
    bit: u64,
    center: CGPoint,
    radius: f64,
    half: f64,
    view: Retained<UIView>,
}

struct Stick {
    base: Retained<UIView>,
    knob: Retained<UIView>,
    home: CGPoint,
    origin: CGPoint,
    radius: f64,
    knob_radius: f64,
    touch: Option<usize>,
}

struct Pad {
    view: Retained<TouchView>,
    controls: Vec<Control>,
    stick: Stick,
    held: HashMap<usize, u64>,
    shown: bool,
    screen: HashSet<usize>,
}

thread_local! {
    static PAD: RefCell<Option<Pad>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NXTouchView"]
    struct TouchView;

    impl TouchView {
        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            layout();
        }

        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            handle(self, touches, Phase::Began);
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            handle(self, touches, Phase::Moved);
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            handle(self, touches, Phase::Ended);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            handle(self, touches, Phase::Ended);
        }
    }
);

#[derive(PartialEq)]
enum Phase {
    Began,
    Moved,
    Ended,
}

fn circle(mtm: MainThreadMarker, parent: &UIView, title: &str, fill: (f64, f64), ring: f64) -> Retained<UIView> {
    let v = UIView::new(mtm);
    v.setUserInteractionEnabled(false);
    v.setBackgroundColor(Some(&UIColor::colorWithWhite_alpha(fill.0, fill.1)));
    let layer = v.layer();
    layer.setBorderWidth(2.0);
    let border = unsafe { UIColor::colorWithWhite_alpha(1.0, ring).CGColor() };
    layer.setBorderColor(Some(&border));
    if !title.is_empty() {
        let l = UILabel::new(mtm);
        l.setText(Some(&NSString::from_str(title)));
        unsafe { l.setTextColor(Some(&UIColor::whiteColor())) };
        l.setShadowColor(Some(&UIColor::colorWithWhite_alpha(0.0, 0.8)));
        l.setShadowOffset(CGSize::new(0.0, 1.0));
        l.setTextAlignment(NSTextAlignment::Center);
        l.setAdjustsFontSizeToFitWidth(true);
        v.addSubview(&l);
    }
    parent.addSubview(&v);
    v
}

fn place(v: &UIView, c: CGPoint, r: f64) {
    place_capsule(v, c, r, 0.0);
}

fn place_capsule(v: &UIView, c: CGPoint, r: f64, half: f64) {
    let w = 2.0 * (r + half);
    v.setFrame(CGRect::new(CGPoint::new(c.x - r - half, c.y - r), CGSize::new(w, 2.0 * r)));
    v.layer().setCornerRadius(r);
    if let Some(l) = v.subviews().firstObject().and_then(|l| l.downcast::<UILabel>().ok()) {
        l.setFrame(CGRect::new(CGPoint::new(2.0, 0.0), CGSize::new(w - 4.0, 2.0 * r)));
        unsafe { l.setFont(Some(&UIFont::boldSystemFontOfSize((r * 0.55).clamp(12.0, 22.0)))) };
    }
}

pub fn install(parent: &UIView, mtm: MainThreadMarker) {
    let view: Retained<TouchView> = unsafe { msg_send![TouchView::alloc(mtm), initWithFrame: parent.bounds()] };
    view.setMultipleTouchEnabled(true);
    view.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
    parent.addSubview(&view);
    let controls = BUTTONS.iter().map(|&(t, bit)| Control { bit, center: CGPoint::new(0.0, 0.0), radius: 0.0, half: 0.0, view: circle(mtm, &view, t, (0.0, FILL), RING) }).collect();
    let base = circle(mtm, &view, "", (0.0, 0.25), 0.6);
    let knob = circle(mtm, &view, "", (1.0, 0.6), RING);
    let zero = CGPoint::new(0.0, 0.0);
    let stick = Stick { base, knob, home: zero, origin: zero, radius: 0.0, knob_radius: 0.0, touch: None };
    let p = Pad { view, controls, stick, held: HashMap::new(), shown: true, screen: HashSet::new() };
    show_controls(&p, false);
    PAD.with(|pad| *pad.borrow_mut() = Some(Pad { shown: false, ..p }));
}

fn show_controls(p: &Pad, visible: bool) {
    for v in p.controls.iter().map(|c| &c.view).chain([&p.stick.base, &p.stick.knob]) {
        v.setHidden(!visible);
    }
}

pub fn set_visible(visible: bool) {
    IDLE.store(false, Relaxed);
    if visible {
        mark();
    }
    PAD.with(|p| {
        let mut p = p.borrow_mut();
        let Some(p) = p.as_mut() else { return };
        if visible {
            show_controls(p, true);
            p.shown = true;
        } else {
            hide(p);
        }
    });
}

fn hide(p: &mut Pad) {
    show_controls(p, false);
    p.shown = false;
    p.held.clear();
    p.stick.touch = None;
    publish(p);
}

fn layout() {
    PAD.with(|p| {
        let mut p = p.borrow_mut();
        let Some(p) = p.as_mut() else { return };
        let b = p.view.bounds().size;
        let inset = p.view.safeAreaInsets();
        let (left, right) = (inset.left, b.width - inset.right);
        let u = b.height.min(b.width) / 10.0;
        let face = CGPoint::new(right - 3.0 * u, b.height - 3.2 * u);
        let at = |dx: f64, dy: f64| CGPoint::new(face.x + dx, face.y + dy);
        let spots = [
            (at(1.35 * u, 0.0), 0.75 * u, 0.0),
            (at(0.0, 1.35 * u), 0.75 * u, 0.0),
            (at(0.0, -1.35 * u), 0.75 * u, 0.0),
            (at(-1.35 * u, 0.0), 0.75 * u, 0.0),
            (CGPoint::new(left + 2.6 * u, 1.2 * u), 0.75 * u, 1.0 * u),
            (CGPoint::new(right - 2.6 * u, 1.2 * u), 0.75 * u, 1.0 * u),
            (CGPoint::new(b.width / 2.0 + 1.2 * u, 0.9 * u), 0.5 * u, 0.0),
            (CGPoint::new(b.width / 2.0 - 1.2 * u, 0.9 * u), 0.5 * u, 0.0),
        ];
        for (c, (center, radius, half)) in p.controls.iter_mut().zip(spots) {
            c.center = center;
            c.radius = radius;
            c.half = half;
            place_capsule(&c.view, center, radius, half);
        }
        let s = &mut p.stick;
        s.home = CGPoint::new(left + 3.2 * u, b.height - 3.2 * u);
        s.radius = 1.6 * u;
        s.knob_radius = 0.7 * u;
        if s.touch.is_none() {
            s.origin = s.home;
        }
        place(&s.base, s.origin, s.radius);
        place(&s.knob, s.origin, s.knob_radius);
    });
}

fn handle(view: &TouchView, touches: &NSSet<UITouch>, phase: Phase) {
    PAD.with(|p| {
        let mut p = p.borrow_mut();
        let Some(p) = p.as_mut() else { return };
        mark();
        if phase == Phase::Began && !p.shown && IDLE.swap(false, Relaxed) {
            show_controls(p, true);
            p.shown = true;
        }
        let b = view.bounds().size;
        for t in touches.allObjects().iter() {
            let id = &*t as *const UITouch as usize;
            let pt = t.locationInView(Some(view));
            match phase {
                Phase::Began => {
                    let hit = p
                        .controls
                        .iter()
                        .map(|c| (c, ((pt.x - c.center.x).abs() - c.half).max(0.0).hypot(pt.y - c.center.y) / c.radius))
                        .filter(|(_, d)| *d < 1.3)
                        .min_by(|a, b| a.1.total_cmp(&b.1));
                    let reach = ((pt.x - p.stick.home.x).powi(2) + (pt.y - p.stick.home.y).powi(2)).sqrt() / p.stick.radius;
                    if let Some((c, _)) = hit.filter(|_| p.shown) {
                        p.held.insert(id, c.bit);
                    } else if p.shown && p.stick.touch.is_none() && reach < STICK_REACH {
                        p.stick.touch = Some(id);
                        p.stick.origin = pt;
                        place(&p.stick.base, pt, p.stick.radius);
                        move_stick(&p.stick, pt);
                    } else if !super::ios::CONTROLLER_MODE.load(Relaxed) && !super::ios::TV.load(Relaxed) {
                        let at = super::screen_point(pt.x, pt.y, b.width, b.height);
                        if super::on_screen(at) {
                            p.screen.insert(id);
                            input::screen_touch(id, Some(at));
                        }
                    }
                }
                Phase::Moved => {
                    if p.stick.touch == Some(id) {
                        move_stick(&p.stick, pt);
                    } else if p.screen.contains(&id) {
                        input::screen_touch(id, Some(super::screen_point(pt.x, pt.y, b.width, b.height)));
                    }
                }
                Phase::Ended => {
                    p.held.remove(&id);
                    if p.screen.remove(&id) {
                        input::screen_touch(id, None);
                    }
                    if p.stick.touch == Some(id) {
                        p.stick.touch = None;
                        p.stick.origin = p.stick.home;
                        place(&p.stick.base, p.stick.home, p.stick.radius);
                        move_stick(&p.stick, p.stick.home);
                    }
                }
            }
        }
        publish(p);
    });
}

fn move_stick(s: &Stick, pt: CGPoint) {
    let (mut dx, mut dy) = (pt.x - s.origin.x, pt.y - s.origin.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len > s.radius {
        dx *= s.radius / len;
        dy *= s.radius / len;
    }
    place(&s.knob, CGPoint::new(s.origin.x + dx, s.origin.y + dy), s.knob_radius);
    input::set_touch_stick((dx / s.radius) as f32, (-dy / s.radius) as f32);
}

fn publish(p: &Pad) {
    let held = p.held.values().fold(0, |m, b| m | b);
    input::TOUCH_BUTTONS.store(held, Relaxed);
    if p.stick.touch.is_none() {
        input::set_touch_stick(0.0, 0.0);
    }
    for c in &p.controls {
        let color = if held & c.bit != 0 { UIColor::colorWithWhite_alpha(1.0, PRESSED) } else { UIColor::colorWithWhite_alpha(0.0, FILL) };
        c.view.setBackgroundColor(Some(&color));
    }
}
