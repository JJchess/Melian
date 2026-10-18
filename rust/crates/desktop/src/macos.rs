//! Native AppKit capsule. Network work stays on Tokio; AppKit stays on the main thread.
use anyhow::{Context, Result};
use guard_core::Engine;
use guard_platform::{host::UiEvent, integration, ipc, Host};
use guard_protocol::{Request, Settings, State};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::*;
use std::{
    cell::RefCell,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedReceiver;

struct ViewState {
    host: Arc<Host>,
    events: UnboundedReceiver<UiEvent>,
    state: State,
    expanded: bool,
    details: bool,
    codex_details: bool,
    codex_page: usize,
    codex_preview: Option<guard_protocol::codex::CodexState>,
    codex_notifications: guard_platform::codex::presentation::NotificationCursor,
    feedback: Option<String>,
    alerts: crate::alerts::Alerts,
    collapse_at: Option<Instant>,
    buttons: Vec<Retained<NSButton>>,
    fingerprint: String,
    mouse_down: Option<NSPoint>,
    dragged: bool,
    focus: usize,
    smoke: bool,
    started: Instant,
    preview_output: Option<String>,
}
struct ViewIvars {
    state: RefCell<ViewState>,
}
define_class!(
    #[unsafe(super = NSPanel)]
    #[thread_kind = MainThreadOnly]
    struct IslandPanel;
    unsafe impl NSObjectProtocol for IslandPanel {}
    impl IslandPanel {
        #[unsafe(method(canBecomeKeyWindow))]fn can_become_key(&self)->bool{true}
        #[unsafe(method(canBecomeMainWindow))]fn can_become_main(&self)->bool{false}
        #[unsafe(method(animationResizeTime:))]fn resize_time(&self,frame:NSRect)->f64{if frame.size.height>self.frame().size.height{0.22}else{0.16}}
    }
);
define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    struct IslandView;
    unsafe impl NSObjectProtocol for IslandView {}
    impl IslandView {
        #[unsafe(method(isFlipped))]fn is_flipped(&self)->bool{true}
        #[unsafe(method(acceptsFirstResponder))]fn accepts_first(&self)->bool{true}
        #[unsafe(method(acceptsFirstMouse:))]fn first_mouse(&self,_event:Option<&NSEvent>)->bool{true}
        #[unsafe(method(drawRect:))]fn draw(&self,_rect:NSRect){unsafe{self.paint();}}
        #[unsafe(method(mouseDown:))]fn mouse_down(&self,event:&NSEvent){let mut s=self.ivars().state.borrow_mut();s.mouse_down=Some(event.locationInWindow());s.dragged=false;}
        #[unsafe(method(mouseDragged:))]fn mouse_dragged(&self,event:&NSEvent){
            {let mut s=self.ivars().state.borrow_mut();s.dragged=true;}
            if let Some(window)=self.window(){window.performWindowDragWithEvent(event);let frame=window.frame();let host=self.ivars().state.borrow().host.clone();let _=host.engine.update_settings(|s|{s.island_left=Some(frame.origin.x+frame.size.width/2.);s.island_top=Some(frame.origin.y+frame.size.height);});}
        }
        #[unsafe(method(mouseUp:))]fn mouse_up(&self,_event:&NSEvent){let dragged=self.ivars().state.borrow().dragged;if !dragged{let expanded=!self.ivars().state.borrow().expanded;self.expand(expanded,true);}}
        #[unsafe(method(keyDown:))]fn key_down(&self,event:&NSEvent){
            match event.keyCode(){53=>self.expand(false,true),36|49=>{let index=self.ivars().state.borrow().focus;self.action(index);},125|124|48=>self.focus(1),126|123=>self.focus(-1),_=>{}}
        }
        #[unsafe(method(performAction:))]fn perform_action(&self,sender:&NSButton){self.action(sender.tag() as usize);}
        #[unsafe(method(toggle:))]fn toggle(&self,_sender:Option<&AnyObject>){let expanded=!self.ivars().state.borrow().expanded;self.expand(expanded,true);}
        #[unsafe(method(tick:))]fn tick(&self,_timer:&NSTimer){self.poll();}
        #[unsafe(method(wake:))]fn wake(&self,_notification:&NSNotification){let s=self.ivars().state.borrow();s.host.engine.invalidate();s.host.codex.refresh();}
    }
);
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
fn color(r: f64, g: f64, b: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.)
}
unsafe fn text(value: &str, x: f64, y: f64, w: f64, h: f64, size: f64, tint: &NSColor) {
    let font = NSFont::systemFontOfSize(size);
    let values: [&AnyObject; 2] = [&font, tint];
    let attributes = NSDictionary::from_slices(
        &[NSFontAttributeName, NSForegroundColorAttributeName],
        &values,
    );
    let string = NSAttributedString::initWithString_attributes(
        NSAttributedString::alloc(),
        &NSString::from_str(value),
        Some(&attributes),
    );
    string.drawInRect(rect(x, y, w, h));
}
impl IslandView {
    fn new(mtm: MainThreadMarker, state: ViewState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            state: RefCell::new(state),
        });
        unsafe { msg_send![super(this),initWithFrame:rect(0.,0.,132.,34.)] }
    }
    unsafe fn paint(&self) {
        let s = self.ivars().state.borrow();
        let bounds = self.bounds();
        color(0.055, 0.057, 0.063).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 17., 17.).fill();
        let blocked = s.state.status == "blocked";
        let tint = if s.state.allowed {
            color(0.5, 0.86, 0.69)
        } else if blocked {
            color(1., 0.43, 0.42)
        } else {
            color(0.97, 0.76, 0.39)
        };
        tint.setFill();
        NSBezierPath::bezierPathWithOvalInRect(rect(15., 13., 7., 7.)).fill();
        let heading = if blocked {
            tint
        } else {
            color(0.95, 0.95, 0.95)
        };
        text(
            if blocked {
                "Melian · 连接受阻"
            } else {
                "Melian"
            },
            31.,
            7.,
            bounds.size.width - 45.,
            22.,
            13.,
            &heading,
        );
        if s.state.codex.as_ref().is_some_and(|c| c.unread) && !s.expanded {
            color(0.61, 0.69, 1.).setFill();
            NSBezierPath::bezierPathWithOvalInRect(rect(bounds.size.width - 13., 8., 5., 5.))
                .fill();
        }
        if s.expanded && s.codex_details {
            let d = guard_platform::codex::presentation::details(
                s.state.codex.as_ref(),
                s.codex_page,
                chrono::Utc::now(),
            );
            let primary = color(0.90, 0.90, 0.94);
            let secondary = color(0.65, 0.65, 0.70);
            let title = s
                .state
                .codex
                .as_ref()
                .filter(|c| c.unread && s.collapse_at.is_some())
                .map(|c| c.notification.as_str())
                .filter(|t| !t.is_empty())
                .unwrap_or("Codex");
            text(title, 20., 43., 208., 24., 14., &primary);
            text(&d.quota, 20., 74., 208., 22., 12., &primary);
            text(&d.reset, 20., 98., 208., 32., 11., &secondary);
            if d.pages == 1 {
                text(&d.checked, 20., 132., 208., 24., 11., &secondary);
            }
            text(
                &d.announcement,
                20.,
                173.,
                208.,
                22.,
                12.,
                &color(0.79, 0.81, 1.),
            );
            text(&d.summary, 20., 198., 208., 42., 11., &secondary);
            text(&d.expected, 20., 241., 208., 30., 11., &secondary);
        } else if s.expanded {
            let reason = s.feedback.as_deref().unwrap_or(&s.state.reason);
            let show_reason = !s.state.allowed || s.feedback.is_some();
            if show_reason {
                text(reason, 17., 43., 214., 42., 12., &color(0.72, 0.73, 0.76));
            }
            if s.details {
                let verified = s
                    .state
                    .verified_at
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_else(|| "未验证".into());
                text(
                    &format!(
                        "节点  {}\nIP 属地  {}\n验证时间  {}\n当前保护  Claude Code 专用入口",
                        s.state.node, s.state.country, verified
                    ),
                    17.,
                    128.,
                    214.,
                    115.,
                    11.,
                    &color(0.72, 0.73, 0.76),
                );
            }
        }
    }
    fn size(&self) -> NSSize {
        let s = self.ivars().state.borrow();
        if s.expanded {
            NSSize::new(
                248.,
                if s.codex_details {
                    424.
                } else if s.details {
                    444.
                } else if !s.state.allowed || s.feedback.is_some() {
                    349.
                } else {
                    253.
                },
            )
        } else {
            NSSize::new(
                if s.state.status == "blocked" {
                    184.
                } else {
                    132.
                },
                34.,
            )
        }
    }
    fn expand(&self, expanded: bool, interactive: bool) {
        {
            let mut s = self.ivars().state.borrow_mut();
            s.expanded = expanded;
            s.details = false;
            s.codex_details = false;
            if !expanded {
                s.feedback = None;
            }
            s.collapse_at = None;
        }
        self.layout(true);
        if interactive && expanded {
            if let Some(window) = self.window() {
                window.makeKeyAndOrderFront(None);
                window.makeFirstResponder(Some(self));
            }
        } else if !expanded {
            if let Some(window) = self.window() {
                window.resignKeyWindow();
            }
        }
    }
    fn layout(&self, animate: bool) {
        unsafe {
            let size = self.size();
            if let Some(window) = self.window() {
                let current = window.frame();
                let mut frame = rect(
                    current.origin.x + (current.size.width - size.width) / 2.,
                    current.origin.y + current.size.height - size.height,
                    size.width,
                    size.height,
                );
                if let Some(screen) = window.screen().or_else(|| NSScreen::mainScreen(self.mtm())) {
                    let work = screen.visibleFrame();
                    frame.origin.x = frame.origin.x.clamp(
                        work.origin.x,
                        (work.origin.x + work.size.width - size.width).max(work.origin.x),
                    );
                    frame.origin.y = frame.origin.y.clamp(
                        work.origin.y,
                        (work.origin.y + work.size.height - size.height).max(work.origin.y),
                    );
                }
                window.setFrame_display_animate(
                    frame,
                    true,
                    animate
                        && !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion(),
                );
            }
            let mut s = self.ivars().state.borrow_mut();
            for button in s.buttons.drain(..) {
                button.removeFromSuperview();
            }
            if !s.expanded {
                self.setNeedsDisplay(true);
                return;
            }
            let show_reason = !s.state.allowed || s.feedback.is_some();
            let mut entries: Vec<(usize, String, f64)> = Vec::new();
            if s.codex_details {
                let d = guard_platform::codex::presentation::details(
                    s.state.codex.as_ref(),
                    s.codex_page,
                    chrono::Utc::now(),
                );
                if d.pages > 1 {
                    entries.push((
                        10,
                        format!("额度 {}/{}  ›", s.codex_page % d.pages + 1, d.pages),
                        132.,
                    ));
                }
                entries.push((8, "来源：codex-reset.com".into(), 272.));
                if d.post.is_some() {
                    entries.push((9, "查看 X 原帖 ↗".into(), 302.));
                }
                entries.push((7, format!("刷新 · {}", d.checked), 340.));
                self.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                    "{}。{}。{}。{}。{}",
                    d.quota, d.reset, d.checked, d.summary, d.expected
                ))));
                entries.push((11, "返回".into(), 378.));
            } else {
                self.setAccessibilityLabel(Some(&NSString::from_str("Melian 菜单")));
                if show_reason {
                    let label = match s.state.suggested_action.as_str() {
                        "startClash" => "打开代理客户端",
                        "switchPreferred" => "恢复代理",
                        "openClash" => "打开代理客户端",
                        _ => "重新检查",
                    };
                    entries.push((4, label.into(), 85.));
                    entries.push((
                        5,
                        if s.details {
                            "收起详情"
                        } else {
                            "查看详情"
                        }
                        .into(),
                        113.,
                    ));
                }
                let start = if s.details {
                    247.
                } else if show_reason {
                    152.
                } else {
                    52.
                };
                for (position, (index, title)) in [
                    (0, "Claude Code".into()),
                    (1, "Claude 网页".into()),
                    (
                        6,
                        guard_platform::codex::presentation::menu(s.state.codex.as_ref()),
                    ),
                    (
                        2,
                        if integration::auto_start() {
                            "开机自启     ✓".into()
                        } else {
                            "开机自启".into()
                        },
                    ),
                    (3, "退出".into()),
                ]
                .into_iter()
                .enumerate()
                {
                    entries.push((index, title, start + position as f64 * 37.));
                }
            }
            for (id, label, y) in entries {
                let button = NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&label),
                    Some(self),
                    Some(sel!(performAction:)),
                    self.mtm(),
                );
                button.setTag(id as isize);
                button.setBordered(false);
                button.setAlignment(NSTextAlignment::Left);
                button.setFont(Some(&NSFont::systemFontOfSize(if id >= 4 && id != 6 {
                    11.
                } else {
                    13.
                })));
                button.setContentTintColor(Some(&color(0.91, 0.91, 0.92)));
                button.setFrame(rect(
                    17.,
                    y,
                    214.,
                    if id >= 4 && id != 6 { 26. } else { 34. },
                ));
                button.setAccessibilityLabel(Some(&NSString::from_str(&label)));
                self.addSubview(&button);
                s.buttons.push(button);
            }
            self.setNeedsDisplay(true);
        }
    }
    fn focus(&self, delta: i32) {
        let mut s = self.ivars().state.borrow_mut();
        if s.buttons.is_empty() {
            return;
        }
        let position = s
            .buttons
            .iter()
            .position(|b| b.tag() == s.focus as isize)
            .unwrap_or(0);
        let next = (position as i32 + delta).rem_euclid(s.buttons.len() as i32) as usize;
        s.focus = s.buttons[next].tag() as usize;
        if let Some(button) = s.buttons.iter().find(|b| b.tag() == s.focus as isize) {
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(button));
            }
        }
    }
    fn action(&self, index: usize) {
        self.ivars().state.borrow_mut().collapse_at = None;
        if (6..=11).contains(&index) {
            {
                let mut s = self.ivars().state.borrow_mut();
                s.host.codex.mark_read();
                match index {
                    6 => {
                        s.codex_details = true;
                        s.details = false;
                        s.codex_page = 0;
                        s.host.codex.refresh();
                    }
                    7 => s.host.codex.refresh(),
                    8 => {
                        let _ = guard_platform::codex::open_link(
                            guard_platform::codex::announcements::SOURCE,
                        );
                    }
                    9 => {
                        let d = guard_platform::codex::presentation::details(
                            s.state.codex.as_ref(),
                            s.codex_page,
                            chrono::Utc::now(),
                        );
                        if let Some(url) = d.post {
                            let _ = guard_platform::codex::open_link(&url);
                        }
                    }
                    10 => s.codex_page += 1,
                    11 => s.codex_details = false,
                    _ => {}
                }
                s.state = s.host.state();
            }
            self.layout(true);
            return;
        }
        if index == 5 {
            let details = !self.ivars().state.borrow().details;
            self.ivars().state.borrow_mut().details = details;
            self.layout(true);
            return;
        }
        let s = self.ivars().state.borrow();
        let host = s.host.clone();
        let action = match index {
            0 => "launch",
            1 => "openWeb",
            2 => "toggleAutoStart",
            3 => "exit",
            4 => s.state.suggested_action.as_str(),
            _ => return,
        }
        .to_string();
        drop(s);
        tokio::spawn(async move {
            let response = host.handle(Request::action(&action)).await;
            if let Some(error) = response.error {
                let _ = host.ui.send(UiEvent::Feedback(error));
            }
        });
    }
    fn poll(&self) {
        let now = Instant::now();
        let mut show = false;
        let mut exit = false;
        let mut changed = false;
        let mut codex_peek = false;
        {
            let mut s = self.ivars().state.borrow_mut();
            while let Ok(event) = s.events.try_recv() {
                match event {
                    UiEvent::Show => show = true,
                    UiEvent::Exit => exit = true,
                    UiEvent::Feedback(message) => {
                        s.feedback = Some(message);
                        show = true;
                    }
                }
            }
            let mut state = s.host.state();
            if let Some(fixture) = &s.codex_preview {
                state.codex = Some(fixture.clone());
            }
            let expanded = s.expanded;
            if s.codex_notifications
                .should_peek(state.codex.as_ref(), state.allowed, expanded)
            {
                codex_peek = true;
                show = true;
                s.collapse_at = Some(now + Duration::from_secs(6));
            }
            if !state.allowed && s.codex_details && s.collapse_at.is_some() {
                s.codex_details = false;
                changed = true;
            }
            if s.alerts.update(
                state.allowed,
                state.status == "blocked",
                &state.reason_code,
                now,
            ) {
                show = true;
                s.collapse_at = Some(now + Duration::from_secs(6));
                if s.host.engine.settings().sound_enabled {
                    NSBeep();
                }
            }
            let mut key = format!(
                "{}|{}|{}|{}|{}",
                state.status,
                state.reason,
                state.node,
                integration::auto_start(),
                s.feedback.as_deref().unwrap_or("")
            );
            key.push_str(&serde_json::to_string(&state.codex).unwrap_or_default());
            if s.expanded && s.codex_details {
                key.push_str(&(chrono::Utc::now().timestamp() / 60).to_string());
            }
            if key != s.fingerprint {
                s.fingerprint = key;
                changed = true;
            }
            s.state = state;
            if s.collapse_at.is_some_and(|t| t <= now) {
                s.expanded = false;
                s.details = false;
                s.codex_details = false;
                s.collapse_at = None;
                changed = true;
            }
            if s.smoke && s.started.elapsed() > Duration::from_secs(2) {
                eprintln!("AppKit smoke: capture and stop");
                exit = true;
            }
        }
        if exit {
            unsafe {
                if let Some(output) = self.ivars().state.borrow().preview_output.clone() {
                    if let Some(bitmap) = self.bitmapImageRepForCachingDisplayInRect(self.bounds())
                    {
                        self.cacheDisplayInRect_toBitmapImageRep(self.bounds(), &bitmap);
                        if let Some(data) = bitmap.representationUsingType_properties(
                            NSBitmapImageFileType::PNG,
                            &NSDictionary::new(),
                        ) {
                            let _ = data.writeToFile_atomically(&NSString::from_str(&output), true);
                        }
                    }
                }
                self.ivars().state.borrow().host.engine.shutdown();
                let application = NSApplication::sharedApplication(self.mtm());
                application.stop(None);
                // stop: from a timer needs an event to wake nextEventMatchingMask:.
                if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                    NSEventType::ApplicationDefined, NSPoint::new(0., 0.),
                    NSEventModifierFlags::empty(), 0., 0, None, 0, 0, 0,
                ) {
                    application.postEvent_atStart(&event, true);
                }
            }
            return;
        }
        if show {
            let deadline = self.ivars().state.borrow().collapse_at;
            self.expand(true, false);
            self.ivars().state.borrow_mut().collapse_at = deadline;
            if codex_peek {
                self.ivars().state.borrow_mut().codex_details = true;
                self.layout(true);
            }
        } else if changed {
            self.layout(false);
        }
        if self.ivars().state.borrow().expanded && NSEvent::pressedMouseButtons() != 0 {
            if let Some(window) = self.window() {
                let p = NSEvent::mouseLocation();
                let frame = window.frame();
                if p.x < frame.origin.x
                    || p.x > frame.origin.x + frame.size.width
                    || p.y < frame.origin.y
                    || p.y > frame.origin.y + frame.size.height
                {
                    self.expand(false, true);
                }
            }
        }
    }
}
pub fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--install-hooks") {
        return integration::install_hooks();
    }
    if args.iter().any(|a| a == "--uninstall-hooks") {
        return integration::uninstall_hooks();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let _enter = runtime.enter();
    let smoke = args.iter().any(|a| a == "--smoke-test");
    let preview = smoke || args.iter().any(|a| a == "--preview");
    if args.iter().any(|a| a == "--health-check") {
        let response = runtime.block_on(ipc::send(Request::action("status")))?;
        anyhow::ensure!(response.ok, "守卫未就绪");
        return Ok(());
    }
    if !preview && runtime.block_on(ipc::send(Request::action("show"))).is_ok() {
        return Ok(());
    }
    let server = if preview {
        None
    } else {
        Some(ipc::Server::bind()?)
    };
    let settings = Settings::load();
    let engine = Engine::new(settings.clone())?;
    let (tx, events) = tokio::sync::mpsc::unbounded_channel();
    let host = Host::new(engine.clone(), tx);
    if let Some(server) = server {
        host.start(server);
        let _ = engine.start();
        guard_platform::network::start(engine.clone());
    }
    let mtm = MainThreadMarker::new().context("AppKit 必须在主线程启动")?;
    let app = NSApplication::sharedApplication(mtm);
    if smoke {
        eprintln!("AppKit smoke: application created");
    }
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let view = IslandView::new(
        mtm,
        ViewState {
            host: host.clone(),
            events,
            state: engine.state(),
            expanded: false,
            details: false,
            codex_details: false,
            codex_page: 0,
            codex_preview: if preview && args.iter().any(|a| a == "--preview-codex") {
                Some(guard_platform::codex::presentation::preview())
            } else {
                None
            },
            codex_notifications: Default::default(),
            feedback: None,
            alerts: Default::default(),
            collapse_at: None,
            buttons: vec![],
            fingerprint: String::new(),
            mouse_down: None,
            dragged: false,
            focus: 0,
            smoke,
            started: Instant::now(),
            preview_output: args
                .windows(2)
                .find(|a| a[0] == "--preview-output")
                .map(|a| a[1].clone()),
        },
    );
    unsafe {
        let screen = NSScreen::mainScreen(mtm).context("无法读取显示器")?;
        let work = screen.visibleFrame();
        let center = settings
            .island_left
            .unwrap_or(work.origin.x + work.size.width / 2.);
        let top = settings
            .island_top
            .unwrap_or(work.origin.y + work.size.height - 12.);
        let panel: Retained<IslandPanel> = msg_send![IslandPanel::alloc(mtm),initWithContentRect:rect(center-66.,top-34.,132.,34.),styleMask:NSWindowStyleMask::Borderless|NSWindowStyleMask::NonactivatingPanel,backing:NSBackingStoreType::Buffered,defer:false];
        panel.setReleasedWhenClosed(false);
        panel.setTitle(ns_string!("Melian"));
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setHidesOnDeactivate(false);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setContentView(Some(&view));
        panel.orderFrontRegardless();
        if smoke {
            eprintln!("AppKit smoke: panel visible");
        }
        view.layout(false);
        view.setAccessibilityLabel(Some(ns_string!("Melian 网络守卫，点击展开，拖动调整位置")));
        let status =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = status.button(mtm) {
            button.setTitle(ns_string!("Melian"));
            button.setTarget(Some(&view));
            button.setAction(Some(sel!(toggle:)));
            button.setAccessibilityLabel(Some(ns_string!("展开 Melian")));
        }
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        center.addObserver_selector_name_object(
            &view,
            sel!(wake:),
            Some(NSWorkspaceDidWakeNotification),
            None,
        );
        let timer = NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            0.25,
            &view,
            sel!(tick:),
            None,
            true,
        );
        if smoke {
            eprintln!("AppKit smoke: expanding");
            view.expand(true, false);
            if args.iter().any(|a| a == "--preview-codex") {
                view.ivars().state.borrow_mut().codex_details = true;
                view.layout(false);
            }
            eprintln!("AppKit smoke: entering event loop");
        }
        app.run();
        if smoke {
            eprintln!("AppKit smoke: event loop stopped");
        }
        timer.invalidate();
        center.removeObserver(&view);
        NSStatusBar::systemStatusBar().removeStatusItem(&status);
        panel.close();
    }
    engine.shutdown();
    drop(_enter);
    runtime.shutdown_timeout(Duration::from_secs(2));
    Ok(())
}
