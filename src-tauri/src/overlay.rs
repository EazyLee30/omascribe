//! A native Wayland layer surface: captions remain independent of the WebView and window layout.
use crate::views::CaptionPreferences;
use gtk::{glib::translate::ToGlibPtr, prelude::*};
use std::{cell::RefCell, path::Path, sync::Arc, time::Instant};
type WindowPtr = *mut gtk::ffi::GtkWindow;
struct LayerApi {
    _library: libloading::Library,
    init: unsafe extern "C" fn(WindowPtr),
    layer: unsafe extern "C" fn(WindowPtr, i32),
    anchor: unsafe extern "C" fn(WindowPtr, i32, i32),
    margin: unsafe extern "C" fn(WindowPtr, i32, i32),
    keyboard: unsafe extern "C" fn(WindowPtr, i32),
    exclusive: unsafe extern "C" fn(WindowPtr, i32),
    monitor: unsafe extern "C" fn(WindowPtr, *mut gtk::gdk::ffi::GdkMonitor),
    namespace: unsafe extern "C" fn(WindowPtr, *const std::ffi::c_char),
}
impl LayerApi {
    fn load(runtime: &Path) -> Result<Arc<Self>, String> {
        // The small LGPL library is dynamically loaded, so it can be replaced independently.
        unsafe {
            let library = libloading::Library::new(
                runtime.join("platform/libgtk-layer-shell.so.0"),
            )
            .or_else(|_| libloading::Library::new("libgtk-layer-shell.so.0"))
            .map_err(|_| {
                "Desktop captions need gtk-layer-shell. Run scripts/bin/setup --overlay-only."
                    .to_string()
            })?;
            let supported: unsafe extern "C" fn() -> i32 = *library
                .get::<unsafe extern "C" fn() -> i32>(b"gtk_layer_is_supported\0")
                .map_err(|e| e.to_string())?;
            if supported() == 0 {
                return Err(
                    "This desktop does not support the Wayland layer-shell protocol.".into(),
                );
            }
            let api = Self {
                init: *library
                    .get(b"gtk_layer_init_for_window\0")
                    .map_err(|e| e.to_string())?,
                layer: *library
                    .get(b"gtk_layer_set_layer\0")
                    .map_err(|e| e.to_string())?,
                anchor: *library
                    .get(b"gtk_layer_set_anchor\0")
                    .map_err(|e| e.to_string())?,
                margin: *library
                    .get(b"gtk_layer_set_margin\0")
                    .map_err(|e| e.to_string())?,
                keyboard: *library
                    .get(b"gtk_layer_set_keyboard_mode\0")
                    .map_err(|e| e.to_string())?,
                exclusive: *library
                    .get(b"gtk_layer_set_exclusive_zone\0")
                    .map_err(|e| e.to_string())?,
                monitor: *library
                    .get(b"gtk_layer_set_monitor\0")
                    .map_err(|e| e.to_string())?,
                namespace: *library
                    .get(b"gtk_layer_set_namespace\0")
                    .map_err(|e| e.to_string())?,
                _library: library,
            };
            Ok(Arc::new(api))
        }
    }
}
struct Overlay {
    window: gtk::Window,
    label: gtk::Label,
    css: gtk::CssProvider,
    api: Arc<LayerApi>,
    last_text: String,
    changed: Instant,
}
thread_local! {static OVERLAY:RefCell<Option<Overlay>>=const{RefCell::new(None)};}
fn color(value: &str) -> (u8, u8, u8) {
    let channel = |i| u8::from_str_radix(value.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    (channel(1), channel(3), channel(5))
}
impl Overlay {
    fn new(runtime: &Path) -> Result<Self, String> {
        let api = LayerApi::load(runtime)?;
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("OmaScribe captions");
        window.set_widget_name("omascribe-caption-window");
        window.set_decorated(false);
        window.set_resizable(false);
        window.set_app_paintable(true);
        if let Some(visual) =
            gtk::prelude::WidgetExt::screen(&window).and_then(|screen| screen.rgba_visual())
        {
            window.set_visual(Some(&visual));
        }
        let pointer: WindowPtr = window.to_glib_none().0;
        unsafe {
            (api.init)(pointer);
            (api.layer)(pointer, 3);
            (api.keyboard)(pointer, 0);
            (api.exclusive)(pointer, -1);
            (api.namespace)(pointer, c"omascribe-captions".as_ptr());
        }
        let label = gtk::Label::new(None);
        label.set_widget_name("omascribe-caption-text");
        label.set_line_wrap(true);
        label.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_justify(gtk::Justification::Center);
        window.add(&label);
        let css = gtk::CssProvider::new();
        window
            .style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        label
            .style_context()
            .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        Ok(Self {
            window,
            label,
            css,
            api,
            last_text: String::new(),
            changed: Instant::now(),
        })
    }
    fn configure(&self, p: &CaptionPreferences) {
        let display = gtk::gdk::Display::default();
        let monitor = display.as_ref().and_then(|d| {
            if p.monitor >= 0 {
                d.monitor(p.monitor)
            } else {
                d.primary_monitor().or_else(|| d.monitor(0))
            }
        });
        let geometry = monitor.as_ref().map(|m| m.geometry());
        let width = geometry.map_or(1440, |g| g.width()) * p.width_percent as i32 / 100;
        let height = (p.font_size as f64 * 1.5 * p.lines as f64 + 28.) as i32;
        let pointer: WindowPtr = self.window.to_glib_none().0;
        unsafe {
            (self.api.anchor)(pointer, 2, (p.position == "top") as i32);
            (self.api.anchor)(pointer, 3, (p.position == "bottom") as i32);
            (self.api.margin)(pointer, 2, p.margin as i32);
            (self.api.margin)(pointer, 3, p.margin as i32);
            (self.api.monitor)(
                pointer,
                monitor
                    .as_ref()
                    .map_or(std::ptr::null_mut(), |m| m.to_glib_none().0),
            );
        }
        self.window.set_size_request(width, height);
        self.window.resize(width, height);
        self.label.set_lines(p.lines as i32);
        self.label.set_selectable(!p.click_through);
        self.label
            .set_xalign(if p.align == "left" { 0. } else { 0.5 });
        self.label.set_justify(if p.align == "left" {
            gtk::Justification::Left
        } else {
            gtk::Justification::Center
        });
        let (r, g, b) = color(&p.background_color);
        let css=format!("#omascribe-caption-window{{background-color:transparent;}}#omascribe-caption-text{{background-color:rgba({r},{g},{b},{});color:{};font-family:sans-serif;font-size:{}px;font-weight:600;padding:12px 22px;border-radius:{}px;text-shadow:0 1px 3px rgba(0,0,0,.5);}}",p.opacity,p.text_color,p.font_size,p.radius);
        let _ = self.css.load_from_data(css.as_bytes());
        if let Some(window) = self.window.window() {
            if p.click_through {
                window.input_shape_combine_region(&gtk::cairo::Region::create(), 0, 0);
            } else {
                window.input_shape_combine_region(
                    &gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
                        0, 0, width, height,
                    )),
                    0,
                    0,
                );
            }
        }
    }
    fn render(&mut self, text: &str, p: &CaptionPreferences) {
        if self.last_text != text {
            self.last_text = text.to_string();
            self.changed = Instant::now();
            self.label.set_text(text);
        }
        if text.is_empty()
            || (p.idle_seconds > 0 && self.changed.elapsed().as_secs() >= p.idle_seconds)
        {
            self.window.hide();
            return;
        }
        if !self.window.is_visible() {
            self.window.show_all();
            self.configure(p);
        }
    }
}
pub fn apply(
    _app: &tauri::AppHandle,
    runtime: &Path,
    p: &CaptionPreferences,
) -> Result<(), String> {
    OVERLAY.with(|state| {
        let mut state = state.borrow_mut();
        if !p.enabled {
            if let Some(overlay) = state.as_ref() {
                overlay.window.hide();
            }
            return Ok(());
        }
        if state.is_none() {
            *state = Some(Overlay::new(runtime)?);
        }
        let overlay = state.as_ref().unwrap();
        overlay.configure(p);
        Ok(())
    })
}
pub fn render(_app: &tauri::AppHandle, text: &str, p: &CaptionPreferences) {
    OVERLAY.with(|state| {
        if let Some(overlay) = state.borrow_mut().as_mut() {
            if p.enabled {
                overlay.render(text, p)
            } else {
                overlay.window.hide()
            }
        }
    });
}
