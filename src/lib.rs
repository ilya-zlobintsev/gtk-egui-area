#![doc = include_str!("../README.md")]

pub use egui;

use gtk::{glib, prelude::*};
use std::{cell::{Cell, RefCell}, rc::Rc, sync::Arc, time::{Duration, Instant}};
use gtk::subclass::prelude::*;

glib::wrapper! {
    /// This type is a thin wrapper around a GObject subclass implemented in the
    /// `imp` module below.
    pub struct EguiArea(ObjectSubclass<imp::EguiArea>)
        @extends gtk::GLArea, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl EguiArea {
    /// The `ui` closure is called every frame (from the GLArea render pass) and
    /// receives a `&mut egui::Ui` to construct the UI.
    pub fn new(ui: impl Fn(&mut egui::Ui) + 'static) -> Self {
        let area: Self = glib::Object::builder().build();
        area.set_ui(ui);
        area
    }

    /// Create with an FPS cap (frames-per-second). Setting this can reduce CPU
    /// usage when you don't need continuous high-rate rendering.
    pub fn with_max_fps(ui: impl Fn(&mut egui::Ui) + 'static, max_fps: u32) -> Self {
        let area = Self::new(ui);
        area.set_max_fps(max_fps);
        area
    }

    /// Set a maximum FPS. `None` (default) means render as often as GTK asks.
    pub fn set_max_fps(&self, max_fps: u32) {
        self.imp().min_render_interval.set(Some(Duration::from_micros(
            ((1000.0 / max_fps as f64) * 1000.0) as u64
        )));
    }

    /// Replace the UI closure that will be executed each frame.
    pub fn set_ui(&self, ui: impl Fn(&mut egui::Ui) + 'static) {
        *self.imp().run_ui.borrow_mut() = Some(Box::new(ui));
    }

    // Get a reference to the inner `egui::Context` if you need advanced control.
    pub fn egui_ctx(&self) -> &egui::Context {
        &self.imp().egui_ctx
    }
}

impl Default for EguiArea {
    fn default() -> Self {
        Self::new(|_ui| {})
    }
}

mod imp {
    use super::*;
    use egui;
    use egui_glow::glow;
    use gtk::{gdk, gio};

    type DynGuiFn = Box<dyn Fn(&mut egui::Ui)>;

    /// Implementation struct for the GObject subclass.
    #[derive(Default)]
    pub struct EguiArea {
        /// The GPU painter from `egui_glow`.
        painter: RefCell<Option<egui_glow::Painter>>,

        /// The egui context we drive every frame.
        pub(super) egui_ctx: egui::Context,

        /// Queue of input events collected from GTK between frames.
        input_events: RefCell<Vec<egui::Event>>,

        /// Optional minimum time between renders (used for FPS limiting).
        pub(super) min_render_interval: Cell<Option<Duration>>,

        /// The user-provided UI closure that runs each frame.
        pub(super) run_ui: RefCell<Option<DynGuiFn>>,

        /// GTK IM context used to support system IMEs (preedit / commit).
        im_context: RefCell<Option<gtk::IMMulticontext>>,

        /// Tracks whether the widget currently has focus (so we can call
        /// im_context.focus_in/focus_out only on changes).
        focused: Cell<bool>,

        /// Current modifier state (kept so pointer/key events can include modifiers).
        modifiers: Rc<Cell<egui::Modifiers>>,

        /// ID of the tick callback, so we can remove it on unrealize.
        tick_id: RefCell<Option<gtk::TickCallbackId>>
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EguiArea {
        const NAME: &'static str = "EguiArea";
        type Type = super::EguiArea;
        type ParentType = gtk::GLArea;
    }

    impl ObjectImpl for EguiArea {
        fn constructed(&self) {
            self.parent_constructed();
            
            // Make sure GL symbols are available.
            init_epoxy();

            let obj = self.obj().clone();
            obj.set_can_focus(true);
            obj.set_focusable(true);
            obj.set_hexpand(true);
            obj.set_vexpand(true);

            // Create and configure IM context early so event controllers can use it.
            let im = gtk::IMMulticontext::new();
            // Ask the IMContext to use preedit strings (so we receive preedit
            // contents via `preedit_string()` and `commit()` signals).
            im.set_use_preedit(true);
            // Bind the IM context to this widget so some IMs can position windows
            // relative to it.
            im.set_client_widget(Some(&obj));
            
            // Keep a reference for later platform-output handling.
            *self.im_context.borrow_mut() = Some(im.clone());

            // Register all input controllers (pointer, keyboard, scroll, gestures).
            self.register_controllers();

            // When focus changes we must notify IMContext (so e.g. candidate
            // windows appear/disappear correctly). We track focus changes in the
            // render pass (where `has_focus()` is always available), but we also
            // listen to focus events to be robust.
            let im_for_focus = self.im_context.borrow().clone();
            obj.connect_notify_local(Some("has-focus"), move |widget, _pspec| {
                if let Some(im) = im_for_focus.as_ref() {
                    if widget.has_focus() {
                        im.focus_in();
                    } else {
                        im.focus_out();
                    }
                }
            });

            // Tick callback: request redraw according to FPS cap.
            let last_render = Cell::new(Instant::now());
            let tick_id = obj.add_tick_callback(move |area, _clock| {
                let should_render = match area.imp().min_render_interval.get() {
                    Some(min_interval) => last_render.get().elapsed() > min_interval,
                    None => true
                };
                if should_render {
                    area.queue_render();
                    last_render.set(Instant::now());
                }
                glib::ControlFlow::Continue
            });
            *self.tick_id.borrow_mut() = Some(tick_id);


            // Connect IM signals to push egui Ime events.
            if let Some(im) = self.im_context.borrow().as_ref() {
                let obj = self.obj().downgrade();
                // `commit` -> final text, send as Ime::Commit
                im.connect_commit(move |_im, text| {
                    if let Some(obj) = obj.upgrade() {
                        let mut events = obj.imp().input_events.borrow_mut();
                        events.push(egui::Event::Ime(egui::ImeEvent::Commit(text.to_string())));
                    };
                });

                // `preedit-changed` -> read current preedit string and forward it.
                let obj = self.obj().downgrade();
                im.connect_preedit_changed(move |im| {
                    // preedit_string returns (text, attr_list, cursor_pos)
                    let (preedit, _attrs, _pos) = im.preedit_string();
                    if let Some(obj) = obj.upgrade() {
                        let mut events = obj.imp().input_events.borrow_mut();
                        events.push(egui::Event::Ime(egui::ImeEvent::Preedit(preedit.to_string())));
                    }
                });

                // preedit_start/end - forward Enabled/Disabled.
                let obj = self.obj().downgrade();
                im.connect_preedit_start(move |_im| {
                    if let Some(obj) = obj.upgrade() {
                        let mut events = obj.imp().input_events.borrow_mut();
                        events.push(egui::Event::Ime(egui::ImeEvent::Enabled));
                    }
                });
                let obj = self.obj().downgrade();
                im.connect_preedit_end(move |_im| {
                    if let Some(obj) = obj.upgrade() {
                        let mut events = obj.imp().input_events.borrow_mut();
                        events.push(egui::Event::Ime(egui::ImeEvent::Disabled));
                    }
                });
            }
        }

        fn dispose(&self) {
            if let Some(im) = self.im_context.borrow_mut().take() {
                im.set_client_widget(None::<&gtk::Widget>);
            }
            // *self.painter.borrow_mut() = None; // Do it in unrealize()
        }
    }

    impl WidgetImpl for EguiArea {
        fn realize(&self) {
            self.parent_realize();

            // Make GL context current and create the egui_glow painter.
            self.obj().make_current();
            let gl = unsafe { glow::Context::from_loader_function(epoxy::get_proc_addr) };
            // Wrap in Arc as egui_glow::Painter wants a shared GL context.
            let gl = Arc::new(gl);
            *self.painter.borrow_mut() = Some(
                egui_glow::Painter::new(gl, "", None, true)
                    .expect("Failed to create painter")
            );
        }

        fn unrealize(&self) {
            if let Some(id) = self.tick_id.borrow_mut().take() {
                id.remove();
            }
            if let Some(mut painter) = self.painter.borrow_mut().take() {
                painter.destroy();
            }
            self.parent_unrealize();
        }
    }

    impl GLAreaImpl for EguiArea {
        fn render(&self, _context: &gdk::GLContext) -> glib::Propagation {
            let area = self.obj();

            let screen_size_pixels = self.native_size();

            // Background color from egui style
            let bg_color = self.egui_ctx.global_style().visuals.window_fill();

            let focused_now = area.has_focus();
            if focused_now != self.focused.get() {
                // Focus changed; inform IM context once.
                self.focused.set(focused_now);
                if let Some(im) = self.im_context.borrow().as_ref() {
                    if focused_now {
                        im.focus_in();
                    } else {
                        im.focus_out();
                    }
                }
            }

            // Clear the GL canvas via painter helper
            let mut painter_guard = self.painter.borrow_mut();
            let painter = painter_guard.as_mut().unwrap();
            painter.clear(screen_size_pixels, bg_color.to_normalized_gamma_f32());

            if let Some(run_ui) = self.run_ui.borrow().as_ref() {
                let input_events: Vec<egui::Event> = std::mem::take(self.input_events.borrow_mut().as_mut());

                // Build egui RawInput: minimal, but correct. All coordinates are in
                // points. We pass the widget size in points via screen_rect.
                let input = egui::RawInput {
                    events: input_events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        Default::default(),
                        egui::Vec2::new(area.width() as f32, area.height() as f32),
                    )),
                    viewports: [(
                        egui::ViewportId::ROOT,
                        egui::ViewportInfo {
                            // Tell egui the underlying native pixel ratio for this surface.
                            native_pixels_per_point: Some(self.scale_factor()),
                            focused: Some(focused_now),
                            ..Default::default()
                        },
                    )].into_iter().collect(),
                    focused: focused_now,
                    ..egui::RawInput::default()
                };

                // Run egui UI
                let full_output = self.egui_ctx.run_ui(input, |ui| run_ui(ui));

                // Platform output
                self.handle_platform_output(full_output.platform_output);

                // Tessellate and draw.
                let clipped_primitives = self.egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
                painter.paint_and_update_textures(
                    screen_size_pixels,
                    self.egui_ctx.pixels_per_point(),
                    &clipped_primitives,
                    &full_output.textures_delta,
                );
            }

            glib::Propagation::Stop
        }
    }

    impl EguiArea {
        /// Helper: obtain the scale factor of the native surface.
        fn scale_factor(&self) -> f32 {
            if let Some(native) = self.obj().native() {
                if let Some(surface) = native.surface() {
                    return surface.scale_factor() as f32;
                }
            }
            1.0
        }

        /// Helper: compute native size in device pixels [width, height].
        fn native_size(&self) -> [u32; 2] {
            let scale_factor = self.scale_factor();
            let width = self.obj().width() as f32;
            let height = self.obj().height() as f32;
            [
                (width * scale_factor) as u32,
                (height * scale_factor) as u32,
            ]
        }

        /// Handle egui's `PlatformOutput` (non-rendering commands).
        /// 
        /// This includes: clipboard text, open_url, IME placement.
        fn handle_platform_output(&self, output: egui::PlatformOutput) {
            for cmd in output.commands {
                match cmd {
                    egui::OutputCommand::CopyText(text) => {
                        if !text.is_empty() {
                            let clipboard = self.obj().clipboard();
                            clipboard.set_text(&text);
                        }
                    }
                    egui::OutputCommand::CopyImage(_image) => {
                        eprintln!("Not Planned");
                    }
                    egui::OutputCommand::OpenUrl(open) => {
                        let window = self.obj().root()
                            .and_then(|r| r.downcast::<gtk::Window>().ok());
                        if let Some(win) = window.as_ref() {
                            let _ = gtk::show_uri(Some(win), &open.url, 0);
                        } else {
                            let _ = gtk::show_uri(None::<&gtk::Window>, &open.url, 0);
                        }
                    }
                }
            }

            // IME placement
            if let Some(ime) = output.ime {
                if let Some(im) = self.im_context.borrow().as_ref() {
                    // point -> physical pixel
                    let ppp = self.egui_ctx.pixels_per_point();
                    let cursor = ime.cursor_rect;
                    let x = (cursor.min.x * ppp).round() as i32;
                    let y = (cursor.min.y * ppp).round() as i32;
                    let w = ((cursor.max.x - cursor.min.x) * ppp).ceil() as i32;
                    let h = ((cursor.max.y - cursor.min.y) * ppp).ceil() as i32;
                    let rect = gdk::Rectangle::new(x, y, w, h);
                    im.set_cursor_location(&rect);
                }
            }
        }

        /// Register GTK event controllers (pointer, scroll, gestures, keyboard) and
        /// translate them to `egui::Event`s stored in `input_events`.
        fn register_controllers(&self) {
            let obj = self.obj().clone();

            // Hold the modifiers state so all input events include the correct modifier keys.
            // Rc<Cell<Modifiers>> allows sharing and interior mutability across closures.
            let current_modifiers = self.modifiers.clone();
            current_modifiers.set(egui::Modifiers::default());

            // Click
            let gesture_click = gtk::GestureClick::new();
            gesture_click.set_button(0);
            {
                let obj = obj.clone();
                let current_modifiers = current_modifiers.clone();
                gesture_click.connect_pressed(move |gesture, _num, x, y| {
                    obj.grab_focus();
                    let mut events = obj.imp().input_events.borrow_mut();
                    let button = match gesture.current_button() {
                        1 => egui::PointerButton::Primary,
                        3 => egui::PointerButton::Secondary,
                        _ => return,
                    };
                    events.push(egui::Event::PointerButton {
                        pos: egui::pos2(x as f32, y as f32),
                        button,
                        pressed: true,
                        modifiers: current_modifiers.get(),
                    });
                });
            }
            {
                let obj = obj.clone();
                let current_modifiers = current_modifiers.clone();
                gesture_click.connect_released(move |gesture, _num, x, y| {
                    let mut events = obj.imp().input_events.borrow_mut();
                    let button = match gesture.current_button() {
                        1 => egui::PointerButton::Primary,
                        3 => egui::PointerButton::Secondary,
                        _ => return,
                    };
                    events.push(egui::Event::PointerButton {
                        pos: egui::pos2(x as f32, y as f32),
                        button,
                        pressed: false,
                        modifiers: current_modifiers.get(),
                    });
                });
            }

            // Move
            let motion = gtk::EventControllerMotion::new();
            {
                let obj = obj.clone();
                motion.connect_motion(move |_motion, x, y| {
                    let mut events = obj.imp().input_events.borrow_mut();
                    events.push(egui::Event::PointerMoved(egui::pos2(x as f32, y as f32)));
                });
            }
            {
                let obj = obj.clone();
                motion.connect_leave(move |_motion| {
                    let mut events = obj.imp().input_events.borrow_mut();
                    events.push(egui::Event::PointerGone);
                });
            }

            // Scroll
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::BOTH_AXES 
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            {
                let obj = obj.clone();
                let current_modifiers = current_modifiers.clone();
                scroll.connect_scroll(move |_scroll, x, y| {
                    let mut events = obj.imp().input_events.borrow_mut();
                    events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::Vec2::new(-x as f32, -y as f32),
                        phase: egui::TouchPhase::Move,
                        modifiers: current_modifiers.get(),
                    });
                    glib::Propagation::Proceed
                });
            }

            // Key
            let key_controller = gtk::EventControllerKey::new();
            {
                let obj = obj.clone();
                key_controller.connect_key_pressed(move |_controller, key, _code, modifiers| {
                    let mut events = obj.imp().input_events.borrow_mut();
                    let emod = gdk_to_egui_modifiers(modifiers);

                    if key == gdk::Key::BackSpace {
                        events.push(egui::Event::Key {
                            key: egui::Key::Backspace,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: emod,
                        });
                        return glib::Propagation::Proceed;
                    }
                    
                    // text input
                    if modifiers.is_empty() || modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                        if let Some(ch) = key.to_unicode() {
                            events.push(egui::Event::Text(ch.into()));
                        }
                    }

                    // egui keys
                    if let Some(ekey) = gdk_to_egui_key(key) {
                        if is_cut_command(emod, ekey) {
                            events.push(egui::Event::Cut);
                        } else if is_copy_command(emod, ekey) {
                            events.push(egui::Event::Copy);
                        } else if is_paste_command(emod, ekey) {
                            let clipboard = obj.clipboard();
                            let obj = obj.clone();
                            clipboard.read_text_async(
                                gio::Cancellable::NONE,
                                move |result| {
                                    if let Ok(Some(text)) = result {
                                        obj.imp().input_events.borrow_mut().push(egui::Event::Paste(text.into()));
                                    }
                                }
                            );
                        }

                        events.push(egui::Event::Key {
                            key: ekey,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: emod,
                        });
                    }

                    glib::Propagation::Proceed
                });
            }
            {
                let obj = obj.clone();
                key_controller.connect_key_released(move |_controller, key, _code, modifiers| {
                    if let Some(ekey) = gdk_to_egui_key(key) {
                        let mut events = obj.imp().input_events.borrow_mut();
                        events.push(egui::Event::Key {
                            key: ekey,
                            physical_key: None,
                            pressed: false,
                            repeat: false,
                            modifiers: gdk_to_egui_modifiers(modifiers),
                        });
                    }
                });
            }
            {
                let current_modifiers = current_modifiers.clone();
                key_controller.connect_modifiers(move |_controller, new_modifiers| {
                    current_modifiers.set(gdk_to_egui_modifiers(new_modifiers));
                    glib::Propagation::Proceed
                });
            }

            obj.add_controller(gesture_click);
            obj.add_controller(motion);
            obj.add_controller(scroll);
            obj.add_controller(key_controller);
        }
    }

    // Utility
    fn gdk_to_egui_key(key: gdk::Key) -> Option<egui::Key> {
        use egui::Key as EKey;
        use gdk::Key as GKey;

        let k = match key {
            GKey::BackSpace => EKey::Backspace,
            GKey::Down => EKey::ArrowDown,
            GKey::Up => EKey::ArrowUp,
            GKey::Left => EKey::ArrowLeft,
            GKey::Right => EKey::ArrowRight,
            GKey::KP_Enter | GKey::ISO_Enter => EKey::Enter,
            GKey::space | GKey::KP_Space => EKey::Space,
            GKey::Page_Up => EKey::PageUp,
            GKey::Page_Down => EKey::PageDown,
            GKey::colon => EKey::Colon,
            GKey::comma => EKey::Comma,
            GKey::backslash => EKey::Backslash,
            GKey::slash => EKey::Slash,
            GKey::vertbar => EKey::Pipe,
            GKey::question => EKey::Questionmark,
            GKey::bracketleft => EKey::OpenBracket,
            GKey::braceright => EKey::CloseBracket,
            GKey::grave => EKey::Backtick,
            GKey::minus => EKey::Minus,
            GKey::period => EKey::Period,
            GKey::plus => EKey::Plus,
            GKey::equal => EKey::Equals,
            GKey::semicolon => EKey::Semicolon,
            GKey::singlelowquotemark => EKey::Quote,
            GKey::_0 | GKey::KP_0 => EKey::Num0,
            GKey::_1 | GKey::KP_1 => EKey::Num1,
            GKey::_2 | GKey::KP_2 => EKey::Num2,
            GKey::_3 | GKey::KP_3 => EKey::Num3,
            GKey::_4 | GKey::KP_4 => EKey::Num4,
            GKey::_5 | GKey::KP_5 => EKey::Num5,
            GKey::_6 | GKey::KP_6 => EKey::Num6,
            GKey::_7 | GKey::KP_7 => EKey::Num7,
            GKey::_8 | GKey::KP_8 => EKey::Num8,
            GKey::_9 | GKey::KP_9 => EKey::Num9,
            other => return other.name()
                .and_then(|n| egui::Key::from_name(&n)),
        };
        Some(k)
    }

    fn gdk_to_egui_modifiers(modifiers: gdk::ModifierType) -> egui::Modifiers {
        use gdk::ModifierType;
        egui::Modifiers { 
            alt: modifiers.contains(ModifierType::ALT_MASK),
            ctrl: modifiers.contains(ModifierType::CONTROL_MASK),
            shift: modifiers.contains(ModifierType::SHIFT_MASK),
            mac_cmd: modifiers.contains(ModifierType::META_MASK),
            #[cfg(target_os = "macos")]
            command: modifiers.contains(ModifierType::META_MASK),
            #[cfg(not(target_os = "macos"))]
            command: modifiers.contains(ModifierType::CONTROL_MASK),
        }
    }

    fn is_cut_command(modifiers: egui::Modifiers, key: egui::Key) -> bool {
        key == egui::Key::Cut
            || (modifiers.command && key == egui::Key::X)
            || (cfg!(target_os = "windows") && modifiers.shift && key == egui::Key::Delete)
    }

    fn is_copy_command(modifiers: egui::Modifiers, key: egui::Key) -> bool {
        key == egui::Key::Copy
            || (modifiers.command && key == egui::Key::C)
            || (cfg!(target_os = "windows") && modifiers.ctrl && key == egui::Key::Insert)
    }

    fn is_paste_command(modifiers: egui::Modifiers, key: egui::Key) -> bool {
        key == egui::Key::Paste
            || (modifiers.command && key == egui::Key::V)
            || (cfg!(target_os = "windows") && modifiers.shift && key == egui::Key::Insert)
    }

    fn init_epoxy() {
        static EPOXY_INIT: std::sync::Once = std::sync::Once::new();
        EPOXY_INIT.call_once(|| {
            #[cfg(target_os = "macos")]
            let library = unsafe { libloading::os::unix::Library::new("libepoxy.0.dylib").unwrap() };
            #[cfg(all(unix, not(target_os = "macos")))]
            let library = unsafe { libloading::os::unix::Library::new("libepoxy.so.0").unwrap() };
            #[cfg(windows)]
            let library = libloading::os::windows::Library::open_already_loaded("libepoxy-0.dll").or_else(|_| libloading::os::windows::Library::open_already_loaded("epoxy-0.dll")).unwrap();

            epoxy::load_with(|name| unsafe {
                library.get::<_>(name.as_bytes()).map(|sym| *sym).unwrap_or(std::ptr::null())
            });
        });
    }
}