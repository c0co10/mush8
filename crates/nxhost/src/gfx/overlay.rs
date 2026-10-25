// Mush8 - overlay.rs
// Constantin Clerc (c22dev) - 0.1
use objc2::MainThreadMarker;
use std::sync::atomic::Ordering::Relaxed;

#[cfg(target_os = "macos")]
pub use ui::set_game_window;

fn on_main(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(move || f(MainThreadMarker::new().expect("main queue runs on the main thread")));
}

const ROW: f64 = 44.0;
const WIDTH: f64 = 560.0;

#[cfg(target_os = "macos")]
mod ui {
    use super::{ROW, WIDTH};
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSBackingStoreType, NSBitmapImageFileType, NSBox, NSBoxType, NSColor, NSFont, NSTextAlignment, NSTextField, NSTitlePosition, NSWindow, NSWindowOrderingMode, NSWindowStyleMask};
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    static WINDOW: AtomicUsize = AtomicUsize::new(0);

    pub fn set_game_window(w: &NSWindow) {
        WINDOW.store(w as *const NSWindow as usize, Relaxed);
    }

    pub fn available() -> bool {
        WINDOW.load(Relaxed) != 0
    }

    struct Panel {
        window: Retained<NSWindow>,
        rows: Vec<Retained<NSTextField>>,
        footer: Retained<NSTextField>,
    }

    thread_local! {
        static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
    }

    fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool, frame: NSRect) -> Retained<NSTextField> {
        let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        let font = if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
        l.setFont(Some(&font));
        l.setTextColor(Some(&NSColor::whiteColor()));
        l.setAlignment(NSTextAlignment::Center);
        l.setFrame(frame);
        l
    }

    pub fn open(mtm: MainThreadMarker, rows: usize) {
        let w = WINDOW.load(Relaxed);
        if w == 0 {
            return;
        }
        let game: &NSWindow = unsafe { &*(w as *const NSWindow) };
        let height = 130.0 + rows as f64 * ROW;
        let content = game.contentView().map_or(game.frame(), |v| game.convertRectToScreen(v.frame()));
        let origin = NSPoint::new(content.origin.x + (content.size.width - WIDTH) / 2.0, content.origin.y + (content.size.height - height) / 2.0);
        let rect = NSRect::new(origin, NSSize::new(WIDTH, height));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, NSWindowStyleMask::Borderless, NSBackingStoreType::Buffered, false)
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setIgnoresMouseEvents(true);
        window.setHasShadow(true);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), rect.size);
        let panel = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
        panel.setBoxType(NSBoxType::Custom);
        panel.setTitlePosition(NSTitlePosition::NoTitle);
        panel.setFillColor(&NSColor::colorWithWhite_alpha(0.08, 0.88));
        panel.setBorderColor(&NSColor::colorWithWhite_alpha(1.0, 0.25));
        panel.setCornerRadius(18.0);
        window.setContentView(Some(&panel));
        let line = |y: f64| NSRect::new(NSPoint::new(20.0, y), NSSize::new(WIDTH - 40.0, ROW - 10.0));
        panel.addSubview(&label(mtm, "Controllers", 26.0, true, line(height - 56.0)));
        let rows: Vec<_> = (0..rows)
            .map(|i| {
                let l = label(mtm, "", 18.0, false, line(height - 100.0 - i as f64 * ROW));
                panel.addSubview(&l);
                l
            })
            .collect();
        let footer = label(mtm, "", 15.0, false, line(16.0));
        panel.addSubview(&footer);
        unsafe { game.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
        PANEL.with(|p| *p.borrow_mut() = Some(Panel { window, rows, footer }));
    }

    pub fn update(rows: Vec<String>, footer: String) {
        PANEL.with(|p| {
            if let Some(p) = &*p.borrow() {
                for (l, t) in p.rows.iter().zip(&rows) {
                    l.setStringValue(&NSString::from_str(t));
                }
                p.footer.setStringValue(&NSString::from_str(&footer));
            }
        });
    }

    pub fn save_png(path: &str) {
        PANEL.with(|p| {
            let Some(p) = &*p.borrow() else { return };
            let Some(view) = p.window.contentView() else { return };
            let rect = view.bounds();
            let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(rect) else { return };
            view.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
            if let Some(data) = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) } {
                let _ = std::fs::write(path, data.to_vec());
                eprintln!("[overlay] controller screen saved to {path}");
            }
        });
    }

    pub fn close() {
        PANEL.with(|p| {
            if let Some(p) = p.borrow_mut().take() {
                let game = WINDOW.load(Relaxed);
                if game != 0 {
                    unsafe { &*(game as *const NSWindow) }.removeChildWindow(&p.window);
                }
                p.window.orderOut(None);
            }
        });
    }
}

#[cfg(target_os = "ios")]
mod ui {
    use super::{ROW, WIDTH};
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::NSString;
    use objc2_ui_kit::{NSTextAlignment, UIColor, UIFont, UILabel, UIView, UIViewAutoresizing};
    use std::cell::RefCell;

    pub fn available() -> bool {
        true
    }

    struct Panel {
        view: Retained<UIView>,
        rows: Vec<Retained<UILabel>>,
        footer: Retained<UILabel>,
    }

    thread_local! {
        static PANEL: RefCell<Option<Panel>> = const { RefCell::new(None) };
    }

    fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool, frame: CGRect) -> Retained<UILabel> {
        let l = UILabel::new(mtm);
        l.setText(Some(&NSString::from_str(text)));
        let font = if bold { UIFont::boldSystemFontOfSize(size) } else { UIFont::systemFontOfSize(size) };
        unsafe { l.setFont(Some(&font)) };
        unsafe { l.setTextColor(Some(&UIColor::whiteColor())) };
        l.setTextAlignment(NSTextAlignment::Center);
        l.setAdjustsFontSizeToFitWidth(true);
        l.setFrame(frame);
        l
    }

    pub fn open(mtm: MainThreadMarker, rows: usize) {
        let Some(root) = super::super::ios::root_view() else { return };
        let bounds = root.bounds();
        let width = WIDTH.min(bounds.size.width - 32.0);
        let row = ROW.min((bounds.size.height - 32.0 - 110.0) / rows.max(1) as f64);
        let height = 110.0 + rows as f64 * row;
        let rect = CGRect::new(CGPoint::new((bounds.size.width - width) / 2.0, (bounds.size.height - height) / 2.0), CGSize::new(width, height));
        let view = UIView::initWithFrame(UIView::alloc(mtm), rect);
        view.setBackgroundColor(Some(&UIColor::colorWithWhite_alpha(0.08, 0.88)));
        view.setUserInteractionEnabled(false);
        view.setAutoresizingMask(UIViewAutoresizing::FlexibleLeftMargin | UIViewAutoresizing::FlexibleRightMargin | UIViewAutoresizing::FlexibleTopMargin | UIViewAutoresizing::FlexibleBottomMargin);
        let layer = view.layer();
        layer.setCornerRadius(18.0);
        layer.setBorderWidth(1.0);
        layer.setBorderColor(Some(&*unsafe { UIColor::colorWithWhite_alpha(1.0, 0.25).CGColor() }));
        let line = |y: f64| CGRect::new(CGPoint::new(20.0, y), CGSize::new(width - 40.0, row - 8.0));
        view.addSubview(&label(mtm, "Controllers", 24.0, true, line(14.0)));
        let rows: Vec<_> = (0..rows)
            .map(|i| {
                let l = label(mtm, "", 17.0, false, line(60.0 + i as f64 * row));
                view.addSubview(&l);
                l
            })
            .collect();
        let footer = label(mtm, "", 14.0, false, line(height - row - 4.0));
        view.addSubview(&footer);
        root.addSubview(&view);
        PANEL.with(|p| *p.borrow_mut() = Some(Panel { view, rows, footer }));
    }

    pub fn update(rows: Vec<String>, footer: String) {
        PANEL.with(|p| {
            if let Some(p) = &*p.borrow() {
                for (l, t) in p.rows.iter().zip(&rows) {
                    l.setText(Some(&NSString::from_str(t)));
                }
                p.footer.setText(Some(&NSString::from_str(&footer)));
            }
        });
    }

    pub fn save_png(_path: &str) {}

    pub fn close() {
        PANEL.with(|p| {
            if let Some(p) = p.borrow_mut().take() {
                p.view.removeFromSuperview();
            }
        });
    }
}

const CONFIRM: u64 = 1 | 1 << 10;
const CANCEL: u64 = 1 << 1;

pub fn controller_screen(min: usize, max: usize) -> Option<usize> {
    let connected = || crate::input::SLOT_NAMES.lock().unwrap().iter().filter(|n| n.is_some()).count();
    if !ui::available() || std::env::var_os("NX_AUTOINPUT").is_some() {
        return Some(connected());
    }
    let max = max.clamp(1, crate::input::MAX_PLAYERS);
    on_main(move |mtm| ui::open(mtm, max));
    let held = || crate::services::hid::BUTTONS.iter().map(|b| b.load(Relaxed)).collect::<Vec<_>>();
    let mut prev = held();
    let mut saved = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(30));
        let names = crate::input::SLOT_NAMES.lock().unwrap().clone();
        let n = connected();
        let rows: Vec<String> = (0..max)
            .map(|i| match &names[i] {
                Some(name) => format!("Player {}  ·  {name}", i + 1),
                None => format!("Player {}  ·  connect a controller", i + 1),
            })
            .collect();
        let keyboard = names[0].as_deref() == Some("Keyboard");
        let footer = match (n < min, keyboard) {
            (true, false) => format!("{n} of {min} players connected · A or + to continue anyway · B to go back"),
            (true, true) => format!("{n} of {min} players · Return to continue anyway · Backspace to go back"),
            (false, false) => "A or + to continue · B to go back".to_string(),
            (false, true) => "Return to continue · Backspace to go back".to_string(),
        };
        on_main(move |_| ui::update(rows, footer));
        if let Some(path) = std::env::var("NX_OVERLAY_PNG").ok().filter(|_| !std::mem::replace(&mut saved, true)) {
            on_main(move |_| ui::save_png(&path));
        }
        let now = held();
        let pressed = |mask: u64| now.iter().zip(&prev).any(|(b, p)| b & mask != 0 && p & mask == 0);
        let (confirm, cancel) = (pressed(CONFIRM), pressed(CANCEL));
        prev = now;
        if confirm || cancel {
            on_main(|_| ui::close());
            return confirm.then_some(n);
        }
    }
}
