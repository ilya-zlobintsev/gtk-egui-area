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
    use gtk::{
        gdk::GLContext,
        glib,
        prelude::{GLAreaExt, NativeExt, SurfaceExt, WidgetExt, WidgetExtManual},
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

            let gesture_click = gtk::GestureClick::new();
            gesture_click.connect_pressed(move |_gesture, _num, x, y| {
                let mut events = obj.imp().input_events.borrow_mut();
                events.push(egui::Event::PointerButton {
                    pos: egui::pos2(x as f32, y as f32),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                });
            });
            let obj = self.obj().clone();
            gesture_click.connect_released(move |_gesture, _num, x, y| {
                let mut events = obj.imp().input_events.borrow_mut();
                events.push(egui::Event::PointerButton {
                    pos: egui::pos2(x as f32, y as f32),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                });
            });

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
            event_controller_scroll.connect_scroll(move |_scroll, x, y| {
                let mut events = obj.imp().input_events.borrow_mut();

                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::Vec2::new(-x as f32, -y as f32),
                    modifiers: egui::Modifiers::default(),
                });
                glib::Propagation::Proceed
            });

            let obj = self.obj().clone();
            let event_controller_key = gtk::EventControllerKey::new();
            event_controller_key.connect_key_pressed(move |_controller, key, _code, _modifier| {
                let mut events = obj.imp().input_events.borrow_mut();

                if let Some(name) = key.name() {
                    if let Some(key) = egui::Key::from_name(&name) {
                        events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::default(),
                        });
                    }
                }
                glib::Propagation::Proceed
            });
            let obj = self.obj().clone();
            event_controller_key.connect_key_released(move |_controller, key, _code, _modifier| {
                let mut events = obj.imp().input_events.borrow_mut();

                if let Some(name) = key.name() {
                    if let Some(key) = egui::Key::from_name(&name) {
                        events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: false,
                            repeat: false,
                            modifiers: egui::Modifiers::default(),
                        });
                    }
                }
            });

            let obj = self.obj().clone();
            obj.add_controller(event_controller_motion);
            obj.add_controller(gesture_click);
            obj.add_controller(event_controller_scroll);
            obj.add_controller(event_controller_key);

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
            let screen_size = self.native_size();
            let bg_color = self.egui_ctx.style().visuals.window_fill();

            let mut painter_guard = self.painter.borrow_mut();
            let painter = painter_guard.as_mut().unwrap();
            painter.clear(screen_size, bg_color.to_normalized_gamma_f32());

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
                        egui::Vec2::new(screen_size[0] as f32, screen_size[1] as f32),
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

                let clipped_primitives = self
                    .egui_ctx
                    .tessellate(full_output.shapes, full_output.pixels_per_point);
                painter.paint_and_update_textures(
                    screen_size,
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
    }
}
