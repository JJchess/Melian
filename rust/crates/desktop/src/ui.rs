use crate::{
    accessibility::{self, Item},
    render::{rect, Renderer},
    WM_ACTION, WM_COLLAPSE, WM_FOCUS_ITEM,
};
use anyhow::Result;
use guard_platform::{host::UiEvent, integration, launch, Host};
use guard_protocol::{log, wide, State};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicIsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::UnboundedReceiver;
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::{
    core::w,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{
            Com::{CoInitializeEx, COINIT_APARTMENTTHREADED},
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            Accessibility::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*,
            WindowsAndMessaging::*,
        },
    },
};
static WINDOW: AtomicIsize = AtomicIsize::new(0);
const WM_MOUSELEAVE: u32 = 0x02a3;
const STATUS_TIMER: usize = 1;
const ANIMATION_TIMER: usize = 2;
const TRAY: u32 = WM_APP + 10;
#[derive(Clone, Copy, PartialEq)]
enum View {
    Menu,
    Guide,
    Details,
}
#[derive(Clone)]
struct Hit {
    id: u32,
    area: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
    name: String,
    checked: Option<bool>,
}
struct Motion {
    start: Instant,
    from: (f32, f32),
    to: (f32, f32),
    duration: Duration,
}
struct App {
    frame_count: u64,
    hwnd: HWND,
    host: Arc<Host>,
    events: UnboundedReceiver<UiEvent>,
    renderer: Option<Renderer>,
    access: accessibility::Shared,
    state: State,
    signature: String,
    expanded: bool,
    view: View,
    width: f32,
    height: f32,
    dpi: f32,
    anchor: (f64, f64),
    motion: Option<Motion>,
    hits: Vec<Hit>,
    hover: u32,
    focus: u32,
    pressed: u32,
    drag_start: Option<POINT>,
    drag_anchor: (f64, f64),
    dragged: bool,
    animations: bool,
    seen_healthy: bool,
    fault: Option<(String, Instant)>,
    notified: HashSet<String>,
    peek_until: Option<Instant>,
    feedback: Option<(String, Instant)>,
    hook: HHOOK,
    closing: bool,
}
pub fn run(host: Arc<Host>, events: UnboundedReceiver<UiEvent>) -> Result<()> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let module = GetModuleHandleW(None)?;
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            hCursor: cursor,
            lpszClassName: w!("ClaudeGuard.Rust.Island"),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            anyhow::bail!("无法注册原生窗口");
        }
        let dpi = GetDpiForSystem() as f32;
        let scale = dpi / 96.;
        let settings = host.engine.settings();
        let mut monitor = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(
            MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
            &mut monitor,
        )
        .ok()?;
        let anchor = (
            settings
                .island_left
                .map(|x| (x + 66.) * scale as f64)
                .unwrap_or((monitor.rcWork.left + monitor.rcWork.right) as f64 / 2.),
            settings
                .island_top
                .map(|y| y * scale as f64)
                .unwrap_or(monitor.rcWork.top as f64 + 10. * scale as f64),
        );
        let access = Arc::new(Mutex::new(accessibility::Data {
            hwnd: 0,
            bounds: UiaRect::default(),
            items: vec![],
            focused: 0,
            status: "正在检查连接".into(),
        }));
        let mut app = Box::new(App {
            frame_count: 0,
            hwnd: HWND::default(),
            state: host.state(),
            host,
            events,
            renderer: None,
            access,
            signature: String::new(),
            expanded: false,
            view: View::Menu,
            width: 132.,
            height: 34.,
            dpi,
            anchor,
            motion: None,
            hits: vec![],
            hover: u32::MAX,
            focus: 0,
            pressed: u32::MAX,
            drag_start: None,
            drag_anchor: anchor,
            dragged: false,
            animations: animations_enabled(),
            seen_healthy: false,
            fault: None,
            notified: HashSet::new(),
            peek_until: None,
            feedback: None,
            hook: HHOOK::default(),
            closing: false,
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            w!("ClaudeGuard.Rust.Island"),
            w!("Claude 网络守卫"),
            WS_POPUP,
            anchor.0 as i32 - 70,
            anchor.1 as i32,
            (140. * scale) as i32,
            (42. * scale) as i32,
            None,
            None,
            module,
            Some((&mut *app as *mut App).cast()),
        )?;
        app.hwnd = hwnd;
        app.dpi = GetDpiForWindow(hwnd) as f32;
        app.clamp();
        app.access.lock().unwrap().hwnd = hwnd.0 as usize;
        app.renderer = Some(Renderer::new(
            hwnd,
            (140. * app.scale()).ceil() as u32,
            (42. * app.scale()).ceil() as u32,
            app.dpi,
        )?);
        WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
        app.hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0)?;
        app.tray(true);
        app.position();
        app.draw()?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        if app.host.engine.settings().proxy_port != 0 {
            log(if std::env::args().any(|a| a == "--autostart") {
                "Rust 桌面已就绪：登录启动"
            } else {
                "Rust 桌面已就绪：手动启动"
            });
        }
        SetTimer(hwnd, STATUS_TIMER, 250, None);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        WINDOW.store(0, Ordering::Relaxed);
        let _ = UnhookWindowsHookEx(app.hook);
        app.tray(false);
        Ok(())
    }
}
impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.hwnd).as_bool() {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                let _ = DestroyWindow(self.hwnd);
            }
            if !self.hook.is_invalid() {
                let _ = UnhookWindowsHookEx(self.hook);
            }
            self.tray(false);
        }
    }
}
fn animations_enabled() -> bool {
    unsafe {
        let mut enabled = BOOL(1);
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        enabled.as_bool()
    }
}
impl App {
    fn scale(&self) -> f32 {
        self.dpi / 96.
    }
    fn desired_height(&self) -> f32 {
        if !self.expanded {
            return 34.;
        }
        match self.view {
            View::Guide => 328.,
            View::Details => 316.,
            View::Menu => {
                if self.problem() {
                    340.
                } else {
                    230.
                }
            }
        }
    }
    fn problem(&self) -> bool {
        !self.state.allowed || self.feedback.is_some()
    }
    fn action_label(&self) -> &'static str {
        match self.state.suggested_action.as_str() {
            "startClash" => "启动 Clash",
            "restoreProxy" => "恢复代理",
            "openClash" => "打开 Clash",
            _ => "重新检查",
        }
    }
    fn animate(&mut self) {
        unsafe {
            let to = if self.expanded {
                (248., self.desired_height())
            } else if self.pressed == 0 {
                (128., 32.)
            } else if self.hover == 0 {
                (136., 35.)
            } else {
                (132., 34.)
            };
            self.motion = Some(Motion {
                start: Instant::now(),
                from: (self.width, self.height),
                to,
                duration: Duration::from_millis(if self.expanded { 220 } else { 160 }),
            });
            if !self.animations {
                self.width = to.0;
                self.height = to.1;
                self.motion = None;
                self.position();
                let _ = self.draw();
            } else {
                SetTimer(self.hwnd, ANIMATION_TIMER, 16, None);
            }
        }
    }
    fn open(&mut self, interactive: bool) {
        unsafe {
            self.expanded = true;
            self.view = View::Menu;
            self.focus = 0;
            self.animate();
            if interactive {
                self.peek_until = None;
                let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32 & !WS_EX_NOACTIVATE.0;
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, style as isize);
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(self.hwnd);
            }
        }
    }
    fn collapse(&mut self) {
        unsafe {
            if !self.expanded {
                return;
            }
            self.expanded = false;
            self.focus = 0;
            self.hover = u32::MAX;
            self.peek_until = None;
            SetWindowLongPtrW(
                self.hwnd,
                GWL_EXSTYLE,
                (GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32 | WS_EX_NOACTIVATE.0) as isize,
            );
            self.animate();
        }
    }
    fn clamp(&mut self) {
        unsafe {
            let mut monitor = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let point = POINT {
                x: self.anchor.0 as i32,
                y: self.anchor.1 as i32,
            };
            if GetMonitorInfoW(
                MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .as_bool()
            {
                let half = 128. * self.scale() as f64;
                self.anchor.0 = self.anchor.0.clamp(
                    monitor.rcWork.left as f64 + half,
                    monitor.rcWork.right as f64 - half,
                );
                self.anchor.1 = self.anchor.1.clamp(
                    monitor.rcWork.top as f64,
                    (monitor.rcWork.bottom as f64 - 350. * self.scale() as f64)
                        .max(monitor.rcWork.top as f64),
                );
            }
        }
    }
    fn position(&mut self) {
        unsafe {
            let scale = self.scale();
            let w = ((self.width + 8.) * scale).ceil() as i32;
            let h = ((self.height + 8.) * scale).ceil() as i32;
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                (self.anchor.0 - w as f64 / 2.).round() as i32,
                self.anchor.1.round() as i32,
                w,
                h,
                SWP_NOACTIVATE,
            );
        }
    }
    fn save_position(&self) {
        let scale = unsafe { GetDpiForSystem() } as f64 / 96.;
        let _ = self.host.engine.update_settings(|s| {
            s.island_left = Some(self.anchor.0 / scale - 66.);
            s.island_top = Some(self.anchor.1 / scale);
        });
    }
    fn step(&mut self) {
        unsafe {
            if let Some(m) = &self.motion {
                let t = (m.start.elapsed().as_secs_f32() / m.duration.as_secs_f32()).min(1.);
                let ease = 1. - (1. - t).powi(3);
                self.width = m.from.0 + (m.to.0 - m.from.0) * ease;
                self.height = m.from.1 + (m.to.1 - m.from.1) * ease;
                if t >= 1. {
                    self.motion = None;
                    let _ = KillTimer(self.hwnd, ANIMATION_TIMER);
                }
                self.position();
                if self.draw().is_err() {
                    self.recreate_renderer();
                }
            }
        }
    }
    fn recreate_renderer(&mut self) {
        self.renderer = None;
        match Renderer::new(
            self.hwnd,
            ((self.width + 8.) * self.scale()).ceil() as u32,
            ((self.height + 8.) * self.scale()).ceil() as u32,
            self.dpi,
        ) {
            Ok(r) => {
                self.renderer = Some(r);
                let _ = self.draw();
            }
            Err(_) => log("图形设备暂时不可用"),
        }
    }
    fn tick(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                UiEvent::Show => self.open(true),
                UiEvent::Exit => self.exit(),
            }
        }
        if self.closing {
            return;
        }
        let next = self.host.state();
        let signature = format!(
            "{}|{}|{}|{}|{}|{}|{}",
            next.status,
            next.reason_code,
            next.reason,
            next.node,
            next.country,
            next.browsers
                .iter()
                .map(|(k, b)| format!("{k}:{}:{}:{}", b.connected, b.protected, b.reason))
                .collect::<Vec<_>>()
                .join(","),
            if self.view == View::Details {
                next.verified_at.map(|t| t.timestamp()).unwrap_or(0)
            } else {
                0
            }
        );
        let was_problem = self.problem();
        self.state = next;
        if self.state.allowed {
            self.seen_healthy = true;
            self.fault = None;
        } else if self.seen_healthy && self.state.status == "blocked" {
            let code = self.state.reason_code.clone();
            if self.fault.as_ref().is_none_or(|(old, _)| old != &code) {
                self.fault = Some((code.clone(), Instant::now()));
            }
            if self
                .fault
                .as_ref()
                .is_some_and(|(_, t)| t.elapsed() >= Duration::from_secs(2))
                && !self.notified.contains(&code)
            {
                self.notified.insert(code);
                if !self.expanded {
                    self.open(false);
                    self.peek_until = Some(Instant::now() + Duration::from_secs(6));
                }
                if self.host.engine.settings().sound_enabled {
                    unsafe {
                        let _ = MessageBeep(MB_ICONWARNING);
                    }
                }
            }
        }
        if self.state.status == "checking" {
            self.fault = None;
        }
        if self.peek_until.is_some_and(|t| Instant::now() >= t) {
            self.collapse();
        }
        if self
            .feedback
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(8))
        {
            self.feedback = None;
            self.signature.clear();
        }
        if signature != self.signature {
            self.signature = signature;
            if self.expanded && (self.problem() != was_problem || self.view == View::Details) {
                self.animate();
            }
            if self.motion.is_none() {
                if self.renderer.is_none() {
                    self.recreate_renderer();
                } else {
                    let _ = self.draw();
                }
            }
        }
    }
    fn row(&mut self, id: u32, name: &str, y: f32, checked: Option<bool>) {
        self.hits.push(Hit {
            id,
            name: name.into(),
            area: rect(16., y, 216., 36.),
            checked,
        });
    }
    fn draw(&mut self) -> Result<()> {
        self.frame_count += 1;
        self.hits.clear();
        let mut error_y = 0.;
        if self.expanded {
            match self.view {
                View::Menu => {
                    error_y = if self.problem() { 110. } else { 0. };
                    if self.problem() {
                        self.row(5, self.action_label(), 100., None);
                        self.hits.push(Hit {
                            id: 6,
                            name: "查看详情".into(),
                            area: rect(161., 72., 67., 24.),
                            checked: None,
                        });
                    }
                    self.row(1, "Claude Code", 52. + error_y, None);
                    self.row(2, "Claude 网页", 90. + error_y, None);
                    self.row(
                        3,
                        "开机自启",
                        138. + error_y,
                        Some(self.host.engine.settings().auto_start),
                    );
                    self.row(4, "退出", 176. + error_y, None);
                }
                View::Guide => {
                    self.row(7, "Edge 接入", 151., None);
                    self.row(8, "Chrome 接入", 190., None);
                    self.row(9, "打开扩展文件夹", 229., None);
                    self.row(10, "返回", 281., None);
                }
                View::Details => {
                    self.row(10, "返回", 266., None);
                }
            }
        }
        let problem = self.problem();
        let Some(r) = self.renderer.as_mut() else {
            return Ok(());
        };
        let scale = self.dpi / 96.;
        r.resize(
            ((self.width + 8.) * scale).ceil() as u32,
            ((self.height + 8.) * scale).ceil() as u32,
            self.dpi,
        )?;
        r.begin();
        let body = rect(4., 4., self.width, self.height);
        let expanded_fraction = ((self.width - 132.) / 116.).clamp(0., 1.);
        // A single surface changes shape. The edge remains quiet on both light and dark desktops.
        r.rounded(
            rect(3., 3., self.width + 2., self.height + 2.),
            if expanded_fraction < 0.05 { 18. } else { 25. },
            0xffffff,
            0.10,
        );
        if expanded_fraction < 0.05 {
            r.rounded(body, 17., 0x09090b, 1.);
        } else {
            r.continuous(body, 17. + 9. * expanded_fraction, 0x09090b)?;
        }
        let header_x = if expanded_fraction > 0. {
            24.
        } else {
            (self.width - 76.) / 2. + 4.
        };
        let dot_color = match self.state.status.as_str() {
            "allowed" => 0x7be0ad,
            "checking" => 0xf4c878,
            _ => 0xf08585,
        };
        r.dot(header_x + 3., 21., 3., dot_color, 1.);
        r.text(
            "Claude",
            rect(header_x + 16., 8., 92., 26.),
            0xf5f5f7,
            1.,
            2,
        );
        if self.expanded || expanded_fraction > 0.1 {
            let a = expanded_fraction;
            r.line(
                (self.width - 27., 23.),
                (self.width - 23., 19.),
                0x78787e,
                a,
                1.,
            );
            r.line(
                (self.width - 23., 19.),
                (self.width - 19., 23.),
                0x78787e,
                a,
                1.,
            );
            match self.view {
                View::Menu => {
                    if problem {
                        let reason = self
                            .feedback
                            .as_ref()
                            .map(|(s, _)| s.as_str())
                            .unwrap_or(&self.state.reason);
                        r.text(reason, rect(24., 40., 202., 34.), 0xb5b5bd, a, 1);
                        r.text("查看详情", rect(164., 76., 64., 17.), 0x77777f, a, 1);
                        r.rounded(rect(23., 101., 210., 32.), 10., 0x1e1e22, a);
                    }
                    r.line(
                        (24., 131. + error_y),
                        (232., 131. + error_y),
                        0xffffff,
                        0.07 * a,
                        1.,
                    );
                }
                View::Guide => {
                    r.text("接入浏览器", rect(24., 45., 200., 27.), 0xf4f4f5, a, 2);
                    r.text("在扩展页开启「开发者模式」。\n选择「加载解压缩的扩展」，\n加载下方文件夹，再重启浏览器。",rect(24.,79.,208.,64.),0xa2a2ad,a,1);
                }
                View::Details => {
                    let node = if self.state.node.is_empty() {
                        "—"
                    } else {
                        &self.state.node
                    };
                    let country = if self.state.country.is_empty() {
                        "待确认"
                    } else if self.state.country == "US" {
                        "美国"
                    } else {
                        &self.state.country
                    };
                    let time = self
                        .state
                        .verified_at
                        .map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_else(|| "—".into());
                    for (index, (label, value)) in [
                        ("节点", node.to_owned()),
                        ("IP 属地", country.to_owned()),
                        ("验证时间", time),
                        ("Edge", browser_label(&self.state, "edge")),
                        ("Chrome", browser_label(&self.state, "chrome")),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let y = 52. + index as f32 * 39.;
                        r.text(label, rect(24., y, 72., 26.), 0x777780, a, 1);
                        r.text(&value, rect(100., y, 132., 32.), 0xc8c8d0, a, 1);
                    }
                }
            }
            for hit in &self.hits {
                let selected = self.hover == hit.id || self.focus == hit.id;
                if selected {
                    let inset = if self.pressed == hit.id { 2. } else { -1. };
                    r.rounded(
                        rect(
                            hit.area.left + inset,
                            hit.area.top + inset,
                            hit.area.right - hit.area.left - 2. * inset,
                            36. - 2. * inset,
                        ),
                        10.,
                        0xffffff,
                        if self.pressed == hit.id {
                            0.10 * a
                        } else {
                            0.055 * a
                        },
                    );
                }
                let y = hit.area.top;
                if hit.id <= 4 {
                    r.icon(
                        hit.id,
                        28.,
                        y + 11.,
                        if hit.id == 4 { 0x81818b } else { 0xb4b4bf },
                        a,
                    );
                    r.text(
                        &hit.name,
                        rect(56., y, 145., 36.),
                        if hit.id == 4 { 0x9b9ba5 } else { 0xe9e9ef },
                        a,
                        0,
                    );
                } else if hit.id == 6 {
                } else {
                    r.text(
                        &hit.name,
                        rect(29., y, 192., 36.),
                        0xd8d8df,
                        a,
                        if hit.id == 5 { 1 } else { 0 },
                    );
                }
                if let Some(checked) = hit.checked {
                    r.rounded(
                        rect(198., y + 10., 27., 16.),
                        8.,
                        if checked { 0x7be0ad } else { 0x3c3c43 },
                        a,
                    );
                    r.dot(
                        if checked { 217. } else { 206. },
                        y + 18.,
                        5.5,
                        if checked { 0x101812 } else { 0xb0b0b8 },
                        a,
                    );
                }
            }
        }
        r.end()?;
        self.update_access();
        Ok(())
    }
    fn update_access(&self) {
        unsafe {
            let mut bounds = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut bounds);
            let scale = self.scale() as f64;
            let mut data = self.access.lock().unwrap();
            data.bounds = UiaRect {
                left: bounds.left as f64,
                top: bounds.top as f64,
                width: (bounds.right - bounds.left) as f64,
                height: (bounds.bottom - bounds.top) as f64,
            };
            data.items = self
                .hits
                .iter()
                .map(|h| Item {
                    id: h.id,
                    name: h.name.clone(),
                    checked: h.checked,
                    rect: UiaRect {
                        left: bounds.left as f64 + h.area.left as f64 * scale,
                        top: bounds.top as f64 + h.area.top as f64 * scale,
                        width: (h.area.right - h.area.left) as f64 * scale,
                        height: (h.area.bottom - h.area.top) as f64 * scale,
                    },
                })
                .collect();
            data.focused = self.focus;
            data.status = self.state.reason.clone();
        }
    }
    fn hit(&self, lparam: LPARAM) -> u32 {
        let x = (lparam.0 as u16 as i16) as f32 / self.scale();
        let y = ((lparam.0 >> 16) as u16 as i16) as f32 / self.scale();
        if y < 40. {
            return 0;
        }
        self.hits
            .iter()
            .find(|h| x >= h.area.left && x < h.area.right && y >= h.area.top && y < h.area.bottom)
            .map(|h| h.id)
            .unwrap_or(u32::MAX)
    }
    fn feedback(&mut self, text: String) {
        self.feedback = Some((text, Instant::now()));
        self.expanded = true;
        self.view = View::Menu;
        self.animate();
    }
    fn action(&mut self, id: u32) {
        self.peek_until = None;
        if self.host.engine.settings().proxy_port == 0 && matches!(id, 1 | 3 | 5 | 7 | 8 | 9) {
            self.feedback("界面预览模式".into());
            return;
        }
        match id {
            0 => {
                if self.expanded {
                    self.collapse()
                } else {
                    self.open(true)
                }
            }
            1 => {
                if self.state.fresh_allowed() {
                    if let Err(e) = launch::launch_terminal(None, &[]) {
                        self.feedback(e.to_string());
                    } else {
                        self.collapse();
                    }
                } else {
                    self.open(true)
                }
            }
            2 => {
                if !self.state.fresh_allowed() {
                    self.open(true);
                } else if self
                    .state
                    .browsers
                    .values()
                    .any(|b| b.connected && b.protected)
                {
                    if let Err(e) = self.host.open_web() {
                        self.feedback(e.to_string());
                    } else {
                        self.collapse();
                    }
                } else {
                    self.view = View::Guide;
                    self.expanded = true;
                    self.animate();
                }
            }
            3 => {
                let enabled = !self.host.engine.settings().auto_start;
                let result = integration::set_auto_start(enabled).and_then(|_| {
                    self.host
                        .engine
                        .update_settings(|s| s.auto_start = enabled)
                        .inspect_err(|_| {
                            let _ = integration::set_auto_start(!enabled);
                        })
                });
                if let Err(e) = result {
                    self.feedback(e.to_string());
                }
                let _ = self.draw();
            }
            4 => self.exit(),
            5 => match self.state.suggested_action.as_str() {
                "startClash" | "openClash" => {
                    if let Err(e) = integration::open_clash() {
                        self.feedback(e.to_string());
                    }
                }
                "restoreProxy" => {
                    let engine = self.host.engine.clone();
                    tokio::spawn(async move {
                        let _ = engine.switch_preferred(None).await;
                    });
                }
                _ => {
                    if self.state.reason_code == "portConflict" {
                        let _ = self.host.engine.start();
                    } else {
                        self.host.engine.invalidate();
                    }
                    self.feedback = None;
                }
            },
            6 => {
                self.view = View::Details;
                self.animate();
            }
            7 | 8 => {
                if let Err(e) =
                    integration::extension_guide(if id == 7 { "edge" } else { "chrome" })
                {
                    self.feedback(e.to_string());
                }
            }
            9 => {
                if let Err(e) = integration::open_extension_folder() {
                    self.feedback(e.to_string());
                }
            }
            10 => {
                self.view = View::Menu;
                self.animate();
            }
            _ => {}
        }
    }
    fn exit(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.host.engine.shutdown();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
    fn tray(&self, add: bool) {
        unsafe {
            let mut data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: TRAY,
                hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
                ..Default::default()
            };
            let tip = wide("Claude 网络守卫");
            data.szTip[..tip.len()].copy_from_slice(&tip);
            let _ = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_DELETE }, &data);
        }
    }
    fn key(&mut self, key: usize) {
        unsafe {
            match key {
                0x1b => self.collapse(),
                0x0d | 0x20 => {
                    let id = self.focus;
                    self.action(id);
                }
                0x09 | 0x26 | 0x28 => {
                    if !self.expanded {
                        self.open(true);
                    }
                    let ids: Vec<_> = self.hits.iter().map(|h| h.id).collect();
                    if !ids.is_empty() {
                        let prev =
                            key == 0x26 || (key == 0x09 && GetKeyState(VK_SHIFT.0 as i32) < 0);
                        let index = ids.iter().position(|id| *id == self.focus);
                        let index = match index {
                            Some(i) => {
                                if prev {
                                    (i + ids.len() - 1) % ids.len()
                                } else {
                                    (i + 1) % ids.len()
                                }
                            }
                            None => {
                                if prev {
                                    ids.len() - 1
                                } else {
                                    0
                                }
                            }
                        };
                        self.focus = ids[index];
                        let _ = self.draw();
                    }
                }
                _ => {}
            }
        }
    }
}
fn browser_label(state: &State, key: &str) -> String {
    state
        .browsers
        .get(key)
        .map(|b| {
            if b.connected && b.protected {
                "已接入"
            } else if b.connected {
                "代理冲突"
            } else {
                "未接入"
            }
            .into()
        })
        .unwrap_or_else(|| "未接入".into())
}
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
        )
    {
        let hwnd = HWND(WINDOW.load(Ordering::Relaxed) as *mut _);
        if !hwnd.is_invalid() {
            let data = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            if data.pt.x < rect.left
                || data.pt.x >= rect.right
                || data.pt.y < rect.top
                || data.pt.y >= rect.bottom
            {
                let _ = PostMessageW(hwnd, WM_COLLAPSE, WPARAM(0), LPARAM(0));
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        return LRESULT(1);
    }
    if msg == WM_ERASEBKGND {
        return LRESULT(1);
    }
    if msg == WM_PAINT {
        let mut paint = PAINTSTRUCT::default();
        let _ = BeginPaint(hwnd, &mut paint);
        let _ = EndPaint(hwnd, &paint);
        return LRESULT(0);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    if msg == WM_APP + 4 {
        return LRESULT((*ptr).frame_count as isize);
    }
    match msg {
        WM_TIMER | WM_ACTION | WM_FOCUS_ITEM | WM_COLLAPSE | WM_LBUTTONDOWN | WM_LBUTTONUP
        | WM_MOUSEMOVE | WM_MOUSELEAVE | WM_KEYDOWN | WM_DPICHANGED | WM_POWERBROADCAST
        | WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_GETOBJECT | WM_CLOSE | WM_DESTROY
        | WM_KILLFOCUS | WM_MOUSEACTIVATE | TRAY => {}
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    let app = &mut *ptr;
    match msg {
        WM_TIMER => {
            if wparam.0 == STATUS_TIMER {
                app.tick();
            } else if wparam.0 == ANIMATION_TIMER {
                app.step();
            }
        }
        WM_ACTION => app.action(wparam.0 as u32),
        WM_FOCUS_ITEM => {
            if !app.expanded {
                app.open(true);
            }
            app.focus = wparam.0 as u32;
            let _ = app.draw();
        }
        WM_COLLAPSE | WM_KILLFOCUS => app.collapse(),
        WM_MOUSEACTIVATE => {
            return LRESULT(if app.expanded {
                MA_ACTIVATE as isize
            } else {
                MA_NOACTIVATE as isize
            })
        }
        WM_LBUTTONDOWN => {
            app.pressed = app.hit(lparam);
            app.focus = 0;
            if app.pressed == 0 {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                app.drag_start = Some(p);
                app.drag_anchor = app.anchor;
                app.dragged = false;
            }
            SetCapture(hwnd);
            if !app.expanded {
                app.animate();
            } else {
                let _ = app.draw();
            }
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            let id = app.hit(lparam);
            if app.dragged {
                app.clamp();
                app.position();
                app.save_position();
            } else if id == app.pressed {
                app.action(id);
            }
            app.drag_start = None;
            app.dragged = false;
            app.pressed = u32::MAX;
            if !app.expanded {
                app.animate();
            } else {
                let _ = app.draw();
            }
        }
        WM_MOUSEMOVE => {
            if let Some(start) = app.drag_start {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                if app.dragged || (p.x - start.x).abs() + (p.y - start.y).abs() > 4 {
                    app.dragged = true;
                    app.anchor = (
                        app.drag_anchor.0 + (p.x - start.x) as f64,
                        app.drag_anchor.1 + (p.y - start.y) as f64,
                    );
                    app.position();
                }
            } else {
                let hit = app.hit(lparam);
                if hit != app.hover {
                    app.hover = hit;
                    app.focus = 0;
                    if !app.expanded {
                        app.animate();
                    } else {
                        let _ = app.draw();
                    }
                }
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tracking);
            }
        }
        WM_MOUSELEAVE => {
            app.hover = u32::MAX;
            if !app.expanded {
                app.animate();
            } else {
                let _ = app.draw();
            }
        }
        WM_KEYDOWN => app.key(wparam.0),
        WM_DPICHANGED => {
            app.dpi = (wparam.0 as u16) as f32;
            app.clamp();
            app.position();
            let _ = app.draw();
        }
        WM_DISPLAYCHANGE => {
            app.clamp();
            app.position();
            app.save_position();
        }
        WM_SETTINGCHANGE => {
            app.animations = animations_enabled();
        }
        WM_POWERBROADCAST => {
            app.host.engine.invalidate();
            return LRESULT(1);
        }
        WM_GETOBJECT => {
            if lparam.0 as i32 == UiaRootObjectId {
                return UiaReturnRawElementProvider(
                    hwnd,
                    wparam,
                    lparam,
                    &accessibility::Provider::simple(app.access.clone(), 0),
                );
            }
        }
        TRAY => {
            if matches!(lparam.0 as u32, WM_LBUTTONUP | WM_RBUTTONUP) {
                if app.expanded {
                    app.collapse();
                } else {
                    app.open(true);
                }
            }
        }
        WM_CLOSE => app.collapse(),
        WM_DESTROY => {
            let _ = KillTimer(hwnd, STATUS_TIMER);
            let _ = KillTimer(hwnd, ANIMATION_TIMER);
            PostQuitMessage(0);
        }
        _ => {}
    }
    LRESULT(0)
}
