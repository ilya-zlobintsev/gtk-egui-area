use gtk::{prelude::*, glib, Application, ApplicationWindow};
use gtk_egui_area::EguiArea;
use std::cell::RefCell;

fn main() {
    unsafe {
        std::env::set_var("GDK_BACKEND", "wayland,x11");
    }

    let app = Application::builder().build();

    app.connect_activate(build_ui);

    app.run();
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::new(app);
    window.set_default_width(1200);
    window.set_default_height(900);

    let demo_windows = RefCell::new(egui_demo_lib::DemoWindows::default());
    let egui_area = EguiArea::new(move |ctx| {
        demo_windows.borrow_mut().ui(ctx);
    });

    let frame = gtk::Frame::new(Some("EGUI"));
    frame.set_label_align(0.5);
    frame.set_margin_top(10);
    frame.set_margin_bottom(10);
    frame.set_margin_start(10);
    frame.set_margin_end(10);
    frame.set_child(Some(&egui_area));

    window.set_child(Some(&frame));

    // recommended
    let app_clone = app.clone();
    window.connect_close_request(move |w| {
        w.hide(); // stop gtk to send draw events, prevent egui to use destroyed gl context
        let app_clone = app_clone.clone();
        glib::MainContext::default().spawn_local(async move {
            app_clone.quit();
        });
        glib::Propagation::Stop
    });

    window.present();
}
