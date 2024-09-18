use gtk::{
    glib::{self, Object},
    subclass::prelude::ObjectSubclassIsExt,
};
use std::time::Duration;

glib::wrapper! {
    pub struct EguiArea(ObjectSubclass<imp::EguiArea>)
        @extends gtk::GLArea, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl EguiArea {
    pub fn new(ui: impl Fn(&egui::Context) + 'static) -> Self {
        let area: Self = Object::builder().build();
        area.set_ui(ui);
        area
    }

    pub fn with_max_fps(run_ui: impl Fn(&egui::Context) + 'static, max_fps: u32) -> Self {
        let area = Self::new(run_ui);
        area.set_max_fps(max_fps);
        area
    }

    pub fn set_max_fps(&self, max_fps: u32) {
        self.imp()
            .min_render_interval
            .set(Some(Duration::from_micros(
                ((1000.0 / max_fps as f64) * 1000.0) as u64,
            )));
    }

    pub fn set_ui(&self, ui: impl Fn(&egui::Context) + 'static) {
        *self.imp().run_ui.borrow_mut() = Some(Box::new(ui));
    }

    pub fn egui_ctx(&self) -> &egui::Context {
        &self.imp().egui_ctx
    }
}

mod imp {
    use egui_glow::glow;
    use glib::clone;
    use gtk::{
        gdk::GLContext,
        gio, glib,
        prelude::{Cast, GLAreaExt, NativeExt, SurfaceExt, WidgetExt, WidgetExtManual},
        subclass::{
            prelude::{
                GLAreaImpl, ObjectImpl, ObjectImplExt, ObjectSubclass, ObjectSubclassExt,
                ObjectSubclassIsExt,
            },
            widget::{WidgetImpl, WidgetImplExt},
        },
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::Arc,
        time::{Duration, Instant},
    };

    type DynGuiFn = Box<dyn Fn(&egui::Context)>;

    #[derive(Default)]
    pub struct EguiArea {
        painter: RefCell<Option<egui_glow::Painter>>,
        pub(super) egui_ctx: egui::Context,
        input_events: RefCell<Vec<egui::Event>>,
        pub(super) min_render_interval: Cell<Option<Duration>>,
        pub(super) run_ui: RefCell<Option<DynGuiFn>>,
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

            let obj = self.obj().clone();
            obj.set_can_focus(true);
            obj.set_focusable(true);

            self.register_controllers();

            let last_render = Cell::new(Instant::now());
            obj.add_tick_callback(move |area, _frame_clock| {
                let should_render = match area.imp().min_render_interval.get() {
                    Some(min_interval) => last_render.get().elapsed() > min_interval,
                    None => true,
                };

                if should_render {
                    area.queue_render();
                    last_render.set(Instant::now());
                }
                glib::ControlFlow::Continue
            });
        }
    }

    impl WidgetImpl for EguiArea {
        fn realize(&self) {
            self.parent_realize();

            self.obj().make_current();
            let gl = unsafe { glow::Context::from_loader_function(epoxy::get_proc_addr) };
            #[allow(clippy::arc_with_non_send_sync)]
            let gl = Arc::new(gl);
            *self.painter.borrow_mut() = Some(egui_glow::Painter::new(gl, "", None).unwrap());
        }

        fn unrealize(&self) {
            self.parent_unrealize();
            if let Some(mut painter) = self.painter.borrow_mut().take() {
                painter.destroy();
            }
        }
    }

    impl GLAreaImpl for EguiArea {
        fn render(&self, _context: &GLContext) -> glib::Propagation {
            let screen_size_pixels = self.native_size();
            let bg_color = self.egui_ctx.style().visuals.window_fill();

            let mut painter_guard = self.painter.borrow_mut();
            let painter = painter_guard.as_mut().unwrap();
            painter.clear(screen_size_pixels, bg_color.to_normalized_gamma_f32());

            if let Some(run_ui) = self.run_ui.borrow().as_ref() {
                let input_events: Vec<egui::Event> =
                    std::mem::take(self.input_events.borrow_mut().as_mut());

                if !input_events.is_empty() {
                    println!("input events: {input_events:?}");
                }

                let input = egui::RawInput {
                    events: input_events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        Default::default(),
                        egui::Vec2::new(self.obj().width() as f32, self.obj().width() as f32),
                    )),
                    viewports: [(
                        egui::ViewportId::ROOT,
                        egui::ViewportInfo {
                            native_pixels_per_point: Some(self.scale_factor()),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                    ..egui::RawInput::default()
                };

                let full_output = self.egui_ctx.run(input, run_ui);

                self.handle_platform_output(full_output.platform_output);

                let clipped_primitives = self
                    .egui_ctx
                    .tessellate(full_output.shapes, full_output.pixels_per_point);
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
        fn scale_factor(&self) -> f32 {
            if let Some(native) = self.obj().native() {
                if let Some(surface) = native.surface() {
                    // TODO: with gtk 4.12+ this can be float
                    return surface.scale_factor() as f32;
                }
            }
            1.0
        }

        fn native_size(&self) -> [u32; 2] {
            let scale_factor = self.scale_factor();

            let width = self.obj().width() as f32;
            let height = self.obj().height() as f32;
            [
                (width * scale_factor) as u32,
                (height * scale_factor) as u32,
            ]
        }

        fn handle_platform_output(&self, output: egui::PlatformOutput) {
            if !output.copied_text.is_empty() {
                let clipboard = self.obj().clipboard();
                clipboard.set_text(&output.copied_text);
            }

            if let Some(url) = output.open_url {
                let window = self
                    .obj()
                    .root()
                    .and_then(|root| root.downcast::<gtk::Window>().ok());
                gtk::show_uri(window.as_ref(), &url.url, 0);
            }
        }

        fn register_controllers(&self) {
            let obj = self.obj().clone();
            let current_modifiers = Rc::new(Cell::new(egui::Modifiers::default()));

            let gesture_click = gtk::GestureClick::new();
            gesture_click.connect_pressed(clone!(
                #[strong]
                current_modifiers,
                #[strong]
                obj,
                move |_gesture, _num, x, y| {
                    let mut events = obj.imp().input_events.borrow_mut();
                    events.push(egui::Event::PointerButton {
                        pos: egui::pos2(x as f32, y as f32),
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: current_modifiers.get(),
                    });
                }
            ));
            gesture_click.connect_released(clone!(
                #[strong]
                current_modifiers,
                #[strong]
                obj,
                move |_gesture, _num, x, y| {
                    let mut events = obj.imp().input_events.borrow_mut();
                    events.push(egui::Event::PointerButton {
                        pos: egui::pos2(x as f32, y as f32),
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: current_modifiers.get(),
                    });
                }
            ));

            let obj = self.obj().clone();
            let event_controller_motion = gtk::EventControllerMotion::new();
            event_controller_motion.connect_motion(move |_motion, x, y| {
                let mut events = obj.imp().input_events.borrow_mut();
                events.push(egui::Event::PointerMoved(egui::pos2(x as f32, y as f32)));
            });
            let obj = self.obj().clone();
            event_controller_motion.connect_leave(move |_motion| {
                let mut events = obj.imp().input_events.borrow_mut();
                events.push(egui::Event::PointerGone);
            });

            let obj = self.obj().clone();

            let event_controller_scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::BOTH_AXES
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            event_controller_scroll.connect_scroll(clone!(
                #[strong]
                current_modifiers,
                move |_scroll, x, y| {
                    let mut events = obj.imp().input_events.borrow_mut();

                    events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::Vec2::new(-x as f32, -y as f32),
                        modifiers: current_modifiers.get(),
                    });
                    glib::Propagation::Proceed
                }
            ));

            let obj = self.obj().clone();
            let event_controller_key = gtk::EventControllerKey::new();
            event_controller_key.connect_key_pressed(move |_controller, key, _code, modifiers| {
                let mut events = obj.imp().input_events.borrow_mut();

                if modifiers.is_empty() || modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    if let Some(char) = key.to_unicode() {
                        if !char.is_control() {
                            events.push(egui::Event::Text(char.into()));
                        }
                    }
                }

                if let Some(key) = gdk_to_egui_key(key) {
                    let modifiers = gdk_to_egui_modifiers(modifiers);

                    if is_copy_command(modifiers, key) {
                        events.push(egui::Event::Copy);
                    } else if is_cut_command(modifiers, key) {
                        events.push(egui::Event::Cut);
                    } else if is_paste_command(modifiers, key) {
                        let clipboard = obj.clipboard();
                        let obj = obj.clone();
                        clipboard.read_text_async(gio::Cancellable::NONE, move |result| {
                            if let Ok(Some(text)) = result {
                                obj.imp()
                                    .input_events
                                    .borrow_mut()
                                    .push(egui::Event::Paste(text.to_string()));
                            }
                        });
                    }

                    events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    });
                }
                glib::Propagation::Proceed
            });
            let obj = self.obj().clone();
            event_controller_key.connect_key_released(move |_controller, key, _code, modifiers| {
                let mut events = obj.imp().input_events.borrow_mut();

                if let Some(key) = gdk_to_egui_key(key) {
                    events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: gdk_to_egui_modifiers(modifiers),
                    });
                }
            });
            event_controller_key.connect_modifiers(move |_controller, new_modifiers| {
                current_modifiers.set(gdk_to_egui_modifiers(new_modifiers));
                glib::Propagation::Proceed
            });

            let obj = self.obj().clone();
            obj.add_controller(event_controller_motion);
            obj.add_controller(gesture_click);
            obj.add_controller(event_controller_scroll);
            obj.add_controller(event_controller_key);
        }
    }

    fn gdk_to_egui_key(key: gtk::gdk::Key) -> Option<egui::Key> {
        use egui::Key as EguiKey;
        use gtk::gdk::Key;
        let key = match key {
            Key::BackSpace => EguiKey::Backspace,
            Key::Down => EguiKey::ArrowDown,
            Key::Up => EguiKey::ArrowUp,
            Key::Left => EguiKey::ArrowLeft,
            Key::Right => EguiKey::ArrowRight,
            Key::KP_Enter | Key::ISO_Enter => EguiKey::Enter,
            Key::space | Key::KP_Space => EguiKey::Space,
            Key::Page_Up => EguiKey::PageUp,
            Key::Page_Down => EguiKey::PageDown,
            Key::colon => EguiKey::Colon,
            Key::comma => EguiKey::Comma,
            Key::backslash => EguiKey::Backslash,
            Key::slash => EguiKey::Slash,
            Key::vertbar => EguiKey::Pipe,
            Key::question => EguiKey::Questionmark,
            Key::bracketleft => EguiKey::OpenBracket,
            Key::braceright => EguiKey::CloseBracket,
            Key::grave => EguiKey::Backtick,
            Key::minus => EguiKey::Minus,
            Key::period => EguiKey::Period,
            Key::plus => EguiKey::Plus,
            Key::equal => EguiKey::Equals,
            Key::semicolon => EguiKey::Semicolon,
            Key::singlelowquotemark => EguiKey::Quote,
            Key::_0 | Key::KP_0 => EguiKey::Num0,
            Key::_1 | Key::KP_1 => EguiKey::Num1,
            Key::_2 | Key::KP_2 => EguiKey::Num2,
            Key::_3 | Key::KP_3 => EguiKey::Num3,
            Key::_4 | Key::KP_4 => EguiKey::Num4,
            Key::_5 | Key::KP_5 => EguiKey::Num5,
            Key::_6 | Key::KP_6 => EguiKey::Num6,
            Key::_7 | Key::KP_7 => EguiKey::Num7,
            Key::_8 | Key::KP_8 => EguiKey::Num8,
            Key::_9 | Key::KP_9 => EguiKey::Num9,
            _ => return key.name().and_then(|name| egui::Key::from_name(&name)),
        };
        Some(key)
    }

    fn gdk_to_egui_modifiers(modifiers: gtk::gdk::ModifierType) -> egui::Modifiers {
        use gtk::gdk::ModifierType;
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
}
